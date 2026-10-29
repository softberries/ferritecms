//! `Idempotency-Key` for the upload routes `POST /media` (07 §1; M2a-2b P10, F5–F7)
//! and `POST /import/wordpress` (M3-2 P25), which share these rules.
//!
//! A client that may retry an upload sends `Idempotency-Key: <key>` (1..=255 bytes of
//! `0x21..=0x7E`; anything else, or the header twice, is `400 malformed-header`). The
//! key is scoped to the caller: the store keeps the SHA-256 of `user || 0x00 || key`
//! ([`key_hash`]), so one user can neither replay nor probe another's key. The first
//! request with a key claims it ([`begin`]) after the caller is authorized and before
//! any of the body is read:
//!
//! * a key whose request finished replays the stored status, JSON body, `Location` and
//!   `ETag` with `Idempotent-Replayed: true`; the body of the retry is not read (a
//!   retry with another file is not detected, P10);
//! * a key whose request is still running is `409 conflict` with `Retry-After: 5`; a
//!   pending key older than [`IDEMPOTENCY_STALE`] (1 h, database clock) is taken over,
//!   and the original request's late finish or release is then a no-op (F5);
//! * a key used for another method or path (the query included) is `422 validation`
//!   (`errors[0].field` = `Idempotency-Key`).
//!
//! The handler records a `2xx` outcome with [`Guard::finish`]; any other outcome frees
//! the key with [`Guard::release`], and a request dropped before either (the client went
//! away, a panic) frees it from the guard's drop, so a retry can run. Rows are deleted
//! [`IDEMPOTENCY_RETENTION`] (24 h) after the claim by `sessions.prune`.
//!
//! Three cases can still give one key a second item (07 §1 lists them): a failed
//! `finish` (the key stays pending, so after 1 h a retry takes it over), an upload that
//! outlives [`IDEMPOTENCY_STALE`] (taken over while it streams; it still records its
//! item), and a client drop while the item commits (the key is freed as for a failure).

use std::sync::Arc;

use axum::{
    Json,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
pub use ferrite_domain::{IDEMPOTENCY_RETENTION, IDEMPOTENCY_STALE};
use ferrite_domain::{IdemBegin, IdempotencyKey, Timestamp, UserId};
use ferrite_store::Store;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{ApiState, problem::Problem};

/// The request header carrying the key.
pub const IDEMPOTENCY_HEADER: &str = "Idempotency-Key";
/// Set to `true` on a replayed response.
pub const REPLAYED_HEADER: &str = "Idempotent-Replayed";
/// The longest key in bytes.
pub const MAX_KEY_BYTES: usize = 255;
/// `Retry-After` of a `409` for a key whose request is still running.
pub const IN_PROGRESS_RETRY_AFTER: u64 = 5;

/// The response headers a finished request stores and its replay restores (F6).
const STORED_HEADERS: [header::HeaderName; 2] = [header::LOCATION, header::ETAG];

fn malformed(detail: &'static str) -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "malformed-header",
        "Malformed header",
        detail,
    )
    .with_error(
        Some(IDEMPOTENCY_HEADER),
        "1 to 255 visible ASCII characters (0x21-0x7E), sent once",
    )
}

/// The request's key: `None` without the header; `400 malformed-header` when it is
/// sent more than once, empty, over [`MAX_KEY_BYTES`] or holds a byte outside
/// `0x21..=0x7E`.
pub fn parse_key(headers: &HeaderMap) -> Result<Option<String>, Problem> {
    let mut values = headers.get_all(IDEMPOTENCY_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(malformed("send Idempotency-Key once"));
    }
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_KEY_BYTES {
        return Err(malformed("Idempotency-Key must be 1 to 255 bytes"));
    }
    if !bytes.iter().all(|b| (0x21..=0x7E).contains(b)) {
        return Err(malformed(
            "Idempotency-Key must be visible ASCII characters (no spaces)",
        ));
    }
    // Visible ASCII is UTF-8.
    std::str::from_utf8(bytes)
        .map(|k| Some(k.to_owned()))
        .map_err(|_| malformed("Idempotency-Key must be visible ASCII characters"))
}

/// The stored form of `user`'s `key`: the SHA-256 hex of `user || 0x00 || key` (the
/// user id in its hyphenated form).
pub fn key_hash(user: UserId, key: &str) -> String {
    let mut h = Sha256::new();
    h.update(user.to_string().as_bytes());
    h.update([0u8]);
    h.update(key.as_bytes());
    ferrite_domain::to_hex(&h.finalize())
}

/// The claim on a key, held by the request that runs it. Consume it with
/// [`Guard::finish`] or [`Guard::release`]; dropped unconsumed, it releases the key in
/// a spawned task.
pub struct Guard {
    owner: Option<Owner>,
}

struct Owner {
    store: Arc<dyn Store>,
    user: UserId,
    key_hash: String,
    token: Timestamp,
}

impl Owner {
    async fn release(self) {
        match self
            .store
            .read()
            .idempotency()
            .release(self.user, &self.key_hash, self.token)
            .await
        {
            Ok(true) => {}
            // Taken over since (F5): the new owner's row stays.
            Ok(false) => tracing::warn!("idempotency key was taken over before its release"),
            Err(e) => tracing::error!(error = %e, "cannot release an idempotency key"),
        }
    }
}

impl Guard {
    /// Records a `2xx` outcome: `status`, the JSON `body` and the response's `Location`
    /// and `ETag`, so retries replay it. The write runs in its own task, so it completes
    /// even when the request is dropped meanwhile. A failure is logged: the key then
    /// stays pending (retries get `409` until it is stale), and the response still goes
    /// out.
    pub async fn finish(mut self, status: StatusCode, headers: &HeaderMap, body: &Value) {
        let Some(owner) = self.owner.take() else {
            return;
        };
        let mut stored = Map::new();
        for name in STORED_HEADERS {
            if let Some(v) = headers.get(&name).and_then(|v| v.to_str().ok()) {
                stored.insert(name.as_str().to_owned(), Value::String(v.to_owned()));
            }
        }
        let response = json!({ "body": body, "headers": stored });
        let status = status.as_u16();
        let task = tokio::spawn(async move {
            owner
                .store
                .read()
                .idempotency()
                .finish(owner.user, &owner.key_hash, owner.token, status, &response)
                .await
        });
        match task.await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                tracing::warn!("idempotency key was taken over before the upload finished");
            }
            Ok(Err(e)) => tracing::error!(error = %e, "cannot record an idempotency key"),
            Err(e) => tracing::error!(error = %e, "idempotency finish task failed"),
        }
    }

    /// Frees the key so a retry can run (a failed request). Like `finish`, the write
    /// runs in its own task.
    pub async fn release(mut self) {
        if let Some(owner) = self.owner.take()
            && let Err(e) = tokio::spawn(owner.release()).await
        {
            tracing::error!(error = %e, "idempotency release task failed");
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let Some(owner) = self.owner.take() else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(owner.release());
            }
            Err(_) => {
                tracing::warn!("idempotency key dropped outside a runtime; it is freed once stale")
            }
        }
    }
}

/// Claims `user`'s `key` for `method path` (`path` with its query). `Ok` when this
/// request runs; otherwise the response to send: the replay, `409` or `422` (see the
/// module docs), or the store's problem.
pub async fn begin(
    api: &ApiState,
    user: UserId,
    key: &str,
    method: &Method,
    path: &str,
) -> Result<Guard, Box<Response>> {
    let claim = IdempotencyKey {
        user,
        key_hash: key_hash(user, key),
        method: method.as_str().to_owned(),
        path: path.to_owned(),
    };
    let begun = api
        .store
        .read()
        .idempotency()
        .begin(&claim, IDEMPOTENCY_STALE)
        .await
        .map_err(|e| Box::new(Problem::from(e).into_response()))?;
    let answer = match begun {
        IdemBegin::Started { token } => {
            return Ok(Guard {
                owner: Some(Owner {
                    store: api.store.clone(),
                    user,
                    key_hash: claim.key_hash,
                    token,
                }),
            });
        }
        IdemBegin::Replay { status, response } => replay(status, response),
        IdemBegin::InProgress => Problem::new(
            StatusCode::CONFLICT,
            "conflict",
            "Conflict",
            "a request with this Idempotency-Key is in progress",
        )
        .with_retry_after(IN_PROGRESS_RETRY_AFTER)
        .into_response(),
        IdemBegin::Mismatch => Problem::validation("the key was used for another request")
            .with_error(Some(IDEMPOTENCY_HEADER), "used for another request")
            .into_response(),
    };
    Err(Box::new(answer))
}

/// The stored outcome as a response, marked replayed.
fn replay(status: u16, mut response: Value) -> Response {
    let Ok(status) = StatusCode::from_u16(status) else {
        tracing::error!(status, "stored idempotency status is not valid");
        return Problem::internal().into_response();
    };
    let body = response
        .get_mut("body")
        .map(Value::take)
        .unwrap_or_default();
    let mut res = (status, Json(body)).into_response();
    let headers = res.headers_mut();
    for name in STORED_HEADERS {
        if let Some(v) = response["headers"][name.as_str()]
            .as_str()
            .and_then(|v| HeaderValue::from_str(v).ok())
        {
            headers.insert(name, v);
        }
    }
    headers.insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(values: &[&[u8]]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(IDEMPOTENCY_HEADER, HeaderValue::from_bytes(v).unwrap());
        }
        h
    }

    #[test]
    fn keys_are_bounded() {
        assert_eq!(parse_key(&HeaderMap::new()).unwrap(), None);
        assert_eq!(
            parse_key(&with(&[b"abc-1"])).unwrap().as_deref(),
            Some("abc-1")
        );
        let max = vec![b'~'; MAX_KEY_BYTES];
        assert!(parse_key(&with(&[&max])).unwrap().is_some());
        let long = vec![b'a'; MAX_KEY_BYTES + 1];
        for bad in [
            &b""[..],
            &long,
            b"a b",
            b"a\tb",
            &[b'a', 0x80],
            &[b'a', 0xFF],
        ] {
            let p = parse_key(&with(&[bad])).unwrap_err();
            assert_eq!(p.slug, "malformed-header", "{bad:?}");
            assert_eq!(p.status, StatusCode::BAD_REQUEST);
        }
        assert_eq!(
            parse_key(&with(&[b"a", b"a"])).unwrap_err().slug,
            "malformed-header"
        );
    }

    #[test]
    fn hash_is_per_user() {
        let (a, b) = (UserId::new(), UserId::new());
        let h = key_hash(a, "k");
        assert_eq!(h.len(), 64);
        assert!(
            h.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(h, key_hash(a, "k"));
        assert_ne!(h, key_hash(b, "k"));
        assert_ne!(h, key_hash(a, "k2"));
        let expected = {
            let mut s = Sha256::new();
            s.update(format!("{a}\0k").as_bytes());
            ferrite_domain::to_hex(&s.finalize())
        };
        assert_eq!(h, expected);
    }

    #[test]
    fn replay_restores_only_the_stored_headers() {
        let res = replay(
            201,
            json!({
                "body": {"id": 1},
                "headers": {"location": "/api/v1/media/x", "etag": "\"1\"", "set-cookie": "a=b"},
            }),
        );
        assert_eq!(res.status(), StatusCode::CREATED);
        let h = res.headers();
        assert_eq!(h[header::LOCATION], "/api/v1/media/x");
        assert_eq!(h[header::ETAG], "\"1\"");
        assert_eq!(h[REPLAYED_HEADER], "true");
        assert!(h.get(header::SET_COOKIE).is_none());
    }
}
