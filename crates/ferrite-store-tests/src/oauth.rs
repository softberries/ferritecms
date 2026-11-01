//! Contract tests for the OAuth tables of `0013_oauth` (M5 task 1): clients, pending
//! authorization requests, authorization codes and grants, and the `oauth_grant_id` link
//! from `api_tokens`.

use ferrite_domain::{
    ApiToken, ApiTokenId, ClientKind, OAuthClient, OAuthClientId, OAuthCode, OAuthGrant,
    OAuthGrantId, OAuthRequest, OAuthRequestId, Timestamp, User,
};
use ferrite_store::{MAX_PRUNE_BATCH, StoreError};
use serde_json::json;

use crate::{
    TestDb,
    export::{dump, row, spec},
    fixtures::fixture_user,
};

const MINUTE: i64 = 60 * 1_000_000;
const DAY: i64 = 24 * 3600 * 1_000_000;

fn plus(t: Timestamp, micros: i64) -> Timestamp {
    Timestamp::from_micros(t.as_micros() + micros).unwrap()
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) -> bool {
    matches!(r, Err(StoreError::Invalid(_)))
}

pub(crate) fn client(id: &str, kind: ClientKind, now: Timestamp) -> OAuthClient {
    OAuthClient {
        id: OAuthClientId::new(id),
        kind,
        name: format!("Client {id}"),
        redirect_uris: vec![
            "https://app.example/cb".into(),
            "http://127.0.0.1/cb".into(),
        ],
        client_uri: Some("https://app.example".into()),
        logo_uri: None,
        created_at: now,
        fetched_at: (kind == ClientKind::Metadata).then_some(now),
        last_used_at: None,
    }
}

fn request(client: &OAuthClient, now: Timestamp, ttl: i64) -> OAuthRequest {
    OAuthRequest {
        id: OAuthRequestId::new(),
        client_id: client.id.clone(),
        redirect_uri: "https://app.example/cb".into(),
        scopes: vec!["content:read".into(), "media:write".into()],
        state: Some("st ate/ü".into()),
        code_challenge: "c".repeat(43),
        resource: Some("https://site.example/mcp".into()),
        created_at: now,
        expires_at: plus(now, ttl),
    }
}

fn code(client: &OAuthClient, user: &User, hash: &str, now: Timestamp, ttl: i64) -> OAuthCode {
    OAuthCode {
        code_hash: hash.into(),
        client_id: client.id.clone(),
        user_id: user.id,
        redirect_uri: "https://app.example/cb".into(),
        scopes: vec!["content:read".into()],
        code_challenge: "d".repeat(43),
        resource: None,
        created_at: now,
        expires_at: plus(now, ttl),
        grant_id: None,
    }
}

fn grant(client: &OAuthClient, user: &User, now: Timestamp, ttl: i64) -> OAuthGrant {
    OAuthGrant {
        id: OAuthGrantId::new(),
        client_id: client.id.clone(),
        user_id: user.id,
        scopes: vec!["content:read".into(), "media:write".into()],
        created_at: now,
        last_used_at: now,
        expires_at: plus(now, ttl),
    }
}

fn oauth_token(user: &User, grant: OAuthGrantId, hash: &str, now: Timestamp) -> ApiToken {
    ApiToken {
        id: ApiTokenId::new(),
        user_id: user.id,
        name: "Client".into(),
        token_hash: hash.into(),
        scopes: vec!["content:read".into()],
        created_at: now,
        last_used_at: None,
        expires_at: Some(plus(now, 60 * MINUTE)),
        oauth_grant_id: Some(grant),
    }
}

fn personal_token(user: &User, hash: &str, now: Timestamp) -> ApiToken {
    ApiToken {
        oauth_grant_id: None,
        ..oauth_token(user, OAuthGrantId::new(), hash, now)
    }
}

/// Clients: upsert replaces the descriptive fields but keeps `created_at` and
/// `last_used_at`; `touch_client` moves `last_used_at` forward only; NUL text is refused.
pub async fn oauth_client_upsert_get_touch(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let now = Timestamp::now();

    let mut c = client("https://app.example/client.json", ClientKind::Metadata, now);
    assert_eq!(oauth.get_client(&c.id).await.unwrap(), None);
    oauth.upsert_client(&c).await.unwrap();
    assert_eq!(oauth.get_client(&c.id).await.unwrap(), Some(c.clone()));

    let used = plus(now, MINUTE);
    assert!(oauth.touch_client(&c.id, used).await.unwrap());
    assert!(oauth.touch_client(&c.id, now).await.unwrap());
    assert!(
        !oauth
            .touch_client(&OAuthClientId::new("missing"), used)
            .await
            .unwrap()
    );

    // A refetched metadata document: new name, URIs and fetched_at; created_at and
    // last_used_at stay.
    let mut again = c.clone();
    again.name = "Renamed".into();
    again.redirect_uris = vec!["https://other.example/cb".into()];
    again.client_uri = None;
    again.logo_uri = Some("https://app.example/logo.png".into());
    again.created_at = plus(now, DAY);
    again.fetched_at = Some(plus(now, 2 * MINUTE));
    oauth.upsert_client(&again).await.unwrap();
    c.name = again.name.clone();
    c.redirect_uris = again.redirect_uris.clone();
    c.client_uri = None;
    c.logo_uri = again.logo_uri.clone();
    c.fetched_at = again.fetched_at;
    c.last_used_at = Some(used);
    assert_eq!(oauth.get_client(&c.id).await.unwrap(), Some(c));

    let registered = client("ferc_abcdefghijklmnopqrstuv", ClientKind::Registered, now);
    oauth.upsert_client(&registered).await.unwrap();
    assert_eq!(
        oauth.get_client(&registered.id).await.unwrap(),
        Some(registered)
    );

    let mut bad = client("nul", ClientKind::Registered, now);
    bad.name = "a\0b".into();
    assert!(is_invalid(oauth.upsert_client(&bad).await));
}

/// A pending request is taken once, only while unexpired; an unknown client is a foreign
/// key error.
pub async fn oauth_request_is_taken_once(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let now = Timestamp::now();
    let c = client("ferc_request_client_000001", ClientKind::Registered, now);
    oauth.upsert_client(&c).await.unwrap();

    let req = request(&c, now, 10 * MINUTE);
    oauth.insert_request(&req).await.unwrap();
    assert_eq!(
        oauth.get_request(req.id, now).await.unwrap(),
        Some(req.clone())
    );
    // Reading does not consume it.
    assert_eq!(
        oauth.get_request(req.id, now).await.unwrap(),
        Some(req.clone())
    );
    assert_eq!(
        oauth.take_request(req.id, now).await.unwrap(),
        Some(req.clone())
    );
    assert_eq!(oauth.take_request(req.id, now).await.unwrap(), None);
    assert_eq!(oauth.get_request(req.id, now).await.unwrap(), None);

    // `expires_at` itself is expired.
    let late = request(&c, now, 10 * MINUTE);
    oauth.insert_request(&late).await.unwrap();
    assert_eq!(
        oauth.get_request(late.id, late.expires_at).await.unwrap(),
        None
    );
    assert_eq!(
        oauth.take_request(late.id, late.expires_at).await.unwrap(),
        None
    );
    // An expired request that was not taken stays for the prune, not for another take.
    assert_eq!(
        oauth
            .take_request(late.id, plus(late.expires_at, -1))
            .await
            .unwrap(),
        Some(late)
    );

    // Optional parts may be absent.
    let mut bare = request(&c, now, MINUTE);
    bare.state = None;
    bare.resource = None;
    oauth.insert_request(&bare).await.unwrap();
    assert_eq!(oauth.take_request(bare.id, now).await.unwrap(), Some(bare));

    let mut orphan = request(&c, now, MINUTE);
    orphan.client_id = OAuthClientId::new("unknown");
    assert!(matches!(
        oauth.insert_request(&orphan).await,
        Err(StoreError::ForeignKey { .. })
    ));
}

/// A code is claimed once, only while unexpired, and stays readable afterwards with the
/// grant it made, so a replay can be detected.
pub async fn oauth_code_expires(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let c = client("ferc_code_client_00000001", ClientKind::Registered, now);
    oauth.upsert_client(&c).await.unwrap();

    let fresh = code(&c, &user, "code-fresh", now, MINUTE);
    oauth.insert_code(&fresh).await.unwrap();
    let dup = code(&c, &user, "code-fresh", now, MINUTE);
    assert!(matches!(
        oauth.insert_code(&dup).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    assert_eq!(
        oauth.code_by_hash_any("code-fresh").await.unwrap(),
        Some(fresh.clone())
    );
    assert_eq!(oauth.code_by_hash_any("nope").await.unwrap(), None);

    let grant_id = OAuthGrantId::new();
    let mut claimed = fresh.clone();
    claimed.grant_id = Some(grant_id);
    assert_eq!(
        oauth.take_code("code-fresh", now, grant_id).await.unwrap(),
        Some(claimed.clone())
    );
    // Used: no second claim, but it is still there for reuse detection.
    assert_eq!(
        oauth
            .take_code("code-fresh", now, OAuthGrantId::new())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        oauth.code_by_hash_any("code-fresh").await.unwrap(),
        Some(claimed)
    );

    // Expired (`expires_at` itself counts as expired): not claimable, still readable.
    let old = code(&c, &user, "code-old", now, MINUTE);
    oauth.insert_code(&old).await.unwrap();
    assert_eq!(
        oauth
            .take_code("code-old", old.expires_at, OAuthGrantId::new())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        oauth.code_by_hash_any("code-old").await.unwrap(),
        Some(old.clone())
    );
    assert_eq!(
        oauth
            .take_code("code-old", plus(now, 1), OAuthGrantId::new())
            .await
            .unwrap()
            .map(|c| c.code_hash),
        Some("code-old".into())
    );
}

/// Insert, look up by the current and by the previous refresh hash, rotate (a
/// compare-and-swap on the current hash), delete.
pub async fn grant_rotation_keeps_previous_hash(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let c = client("ferc_grant_client_0000001", ClientKind::Registered, now);
    oauth.upsert_client(&c).await.unwrap();

    let mut g = grant(&c, &user, now, 30 * DAY);
    oauth.insert_grant(&g, "refresh-1").await.unwrap();
    // A refresh hash is unique.
    let clash = grant(&c, &user, now, 30 * DAY);
    assert!(matches!(
        oauth.insert_grant(&clash, "refresh-1").await,
        Err(StoreError::UniqueViolation { .. })
    ));
    assert_eq!(
        oauth.grant_by_refresh("refresh-1").await.unwrap(),
        Some(g.clone())
    );
    assert_eq!(oauth.grant_by_previous("refresh-1").await.unwrap(), None);
    assert_eq!(oauth.grant_by_refresh("nope").await.unwrap(), None);
    assert_eq!(oauth.get_grant(g.id).await.unwrap(), Some(g.clone()));
    assert_eq!(oauth.get_grant(OAuthGrantId::new()).await.unwrap(), None);

    let later = plus(now, 5 * MINUTE);
    let expires = plus(later, 30 * DAY);
    assert!(
        oauth
            .rotate_grant(g.id, "refresh-2", "refresh-1", expires, later)
            .await
            .unwrap()
    );
    g.last_used_at = later;
    g.expires_at = expires;
    assert_eq!(
        oauth.grant_by_refresh("refresh-2").await.unwrap(),
        Some(g.clone())
    );
    assert_eq!(
        oauth.grant_by_previous("refresh-1").await.unwrap(),
        Some(g.clone())
    );
    assert_eq!(oauth.grant_by_refresh("refresh-1").await.unwrap(), None);
    assert_eq!(oauth.grant_by_previous("refresh-2").await.unwrap(), None);

    // The old hash no longer matches: a second rotation from it changes nothing.
    assert!(
        !oauth
            .rotate_grant(g.id, "refresh-x", "refresh-1", expires, later)
            .await
            .unwrap()
    );
    assert_eq!(oauth.grant_by_refresh("refresh-x").await.unwrap(), None);
    // Rotating again moves the previous hash on.
    assert!(
        oauth
            .rotate_grant(g.id, "refresh-3", "refresh-2", expires, later)
            .await
            .unwrap()
    );
    assert_eq!(
        oauth.grant_by_previous("refresh-2").await.unwrap(),
        Some(g.clone())
    );
    assert_eq!(oauth.grant_by_previous("refresh-1").await.unwrap(), None);
    assert!(
        !oauth
            .rotate_grant(OAuthGrantId::new(), "r", "refresh-3", expires, later)
            .await
            .unwrap()
    );

    // The user's and the global listings.
    let second = {
        let mut s = grant(&c, &user, plus(now, MINUTE), 30 * DAY);
        s.scopes = Vec::new();
        s
    };
    oauth.insert_grant(&second, "refresh-other").await.unwrap();
    let other_user = fixture_user(store).await;
    let third = grant(&c, &other_user, plus(now, 2 * MINUTE), 30 * DAY);
    oauth.insert_grant(&third, "refresh-third").await.unwrap();
    assert_eq!(
        oauth.grants_for_user(user.id).await.unwrap(),
        vec![second.clone(), g.clone()]
    );
    let all = oauth.all_grants(2, None).await.unwrap();
    assert_eq!(all.items, vec![third.clone(), second.clone()]);
    let cursor = all.next_cursor.expect("a second page");
    let rest = oauth.all_grants(2, Some(&cursor)).await.unwrap();
    assert_eq!(rest.items, vec![g.clone()]);
    assert_eq!(rest.next_cursor, None);
    assert!(is_invalid(oauth.all_grants(0, None).await));
    assert!(is_invalid(oauth.all_grants(10, Some("garbage")).await));

    assert!(oauth.delete_grant(g.id).await.unwrap());
    assert!(!oauth.delete_grant(g.id).await.unwrap());
    assert_eq!(oauth.grant_by_refresh("refresh-3").await.unwrap(), None);
    assert_eq!(oauth.grant_by_previous("refresh-2").await.unwrap(), None);
}

/// Deleting a grant deletes its access tokens (the foreign key); `delete_for_grant` deletes
/// them and keeps the grant; personal tokens are never touched.
pub async fn deleting_grant_deletes_its_tokens(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let tokens = store.read().api_tokens();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let c = client("ferc_token_client_0000001", ClientKind::Registered, now);
    oauth.upsert_client(&c).await.unwrap();

    let g = grant(&c, &user, now, 30 * DAY);
    let keep = grant(&c, &user, now, 30 * DAY);
    oauth.insert_grant(&g, "r-g").await.unwrap();
    oauth.insert_grant(&keep, "r-keep").await.unwrap();
    for (grant, hash) in [(g.id, "t1"), (g.id, "t2"), (keep.id, "t-keep")] {
        tokens
            .insert(&oauth_token(&user, grant, hash, now))
            .await
            .unwrap();
    }
    tokens
        .insert(&personal_token(&user, "t-personal", now))
        .await
        .unwrap();
    assert_eq!(
        tokens
            .get_by_hash("t1")
            .await
            .unwrap()
            .unwrap()
            .oauth_grant_id,
        Some(g.id)
    );
    assert_eq!(
        tokens
            .get_by_hash("t-personal")
            .await
            .unwrap()
            .unwrap()
            .oauth_grant_id,
        None
    );

    // An unknown grant is a foreign key error.
    assert!(matches!(
        tokens
            .insert(&oauth_token(&user, OAuthGrantId::new(), "t-orphan", now))
            .await,
        Err(StoreError::ForeignKey { .. })
    ));

    assert_eq!(tokens.delete_for_grant(g.id).await.unwrap(), 2);
    assert_eq!(tokens.delete_for_grant(g.id).await.unwrap(), 0);
    assert_eq!(tokens.get_by_hash("t1").await.unwrap(), None);
    assert!(oauth.grant_by_refresh("r-g").await.unwrap().is_some());

    tokens
        .insert(&oauth_token(&user, g.id, "t3", now))
        .await
        .unwrap();
    assert!(oauth.delete_grant(g.id).await.unwrap());
    assert_eq!(tokens.get_by_hash("t3").await.unwrap(), None);
    assert!(tokens.get_by_hash("t-keep").await.unwrap().is_some());
    assert!(tokens.get_by_hash("t-personal").await.unwrap().is_some());
}

/// The 50-token limit counts and the Account list show personal tokens only.
pub async fn personal_token_counts_ignore_oauth_tokens(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let tokens = store.read().api_tokens();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let c = client("ferc_count_client_0000001", ClientKind::Registered, now);
    oauth.upsert_client(&c).await.unwrap();
    let g = grant(&c, &user, now, 30 * DAY);
    oauth.insert_grant(&g, "r").await.unwrap();

    let personal = personal_token(&user, "p1", now);
    tokens.insert(&personal).await.unwrap();
    tokens
        .insert(&oauth_token(&user, g.id, "o1", plus(now, 1)))
        .await
        .unwrap();
    tokens
        .insert(&oauth_token(&user, g.id, "o2", plus(now, 2)))
        .await
        .unwrap();

    assert_eq!(tokens.count_for_user(user.id).await.unwrap(), 1);
    assert_eq!(
        tokens.list_for_user(user.id).await.unwrap(),
        vec![personal.clone()]
    );
    // Lookup by hash still finds an OAuth token (that is how it authenticates).
    assert!(tokens.get_by_hash("o1").await.unwrap().is_some());
    // Disabling a user deletes everything they hold.
    assert_eq!(tokens.delete_for_user(user.id).await.unwrap(), 3);
}

/// `prune_expired` deletes expired requests, codes and grants (a grant's tokens with it)
/// in batches of `limit`, and keeps the live ones.
pub async fn prune_removes_expired_oauth_rows(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let tokens = store.read().api_tokens();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let c = client("ferc_prune_client_0000001", ClientKind::Registered, now);
    oauth.upsert_client(&c).await.unwrap();

    let old = plus(now, -DAY);
    let dead_requests: Vec<_> = (0..3).map(|_| request(&c, old, MINUTE)).collect();
    for r in &dead_requests {
        oauth.insert_request(r).await.unwrap();
    }
    let live_request = request(&c, now, 10 * MINUTE);
    oauth.insert_request(&live_request).await.unwrap();
    // `expires_at == now` is expired.
    let edge_request = request(&c, plus(now, -MINUTE), MINUTE);
    oauth.insert_request(&edge_request).await.unwrap();

    for i in 0..2 {
        oauth
            .insert_code(&code(&c, &user, &format!("dead-{i}"), old, MINUTE))
            .await
            .unwrap();
    }
    oauth
        .insert_code(&code(&c, &user, "live", now, MINUTE))
        .await
        .unwrap();

    let dead_grant = grant(&c, &user, old, 10 * MINUTE);
    oauth.insert_grant(&dead_grant, "r-dead").await.unwrap();
    tokens
        .insert(&oauth_token(&user, dead_grant.id, "t-dead", old))
        .await
        .unwrap();
    let live_grant = grant(&c, &user, now, 30 * DAY);
    oauth.insert_grant(&live_grant, "r-live").await.unwrap();
    tokens
        .insert(&oauth_token(&user, live_grant.id, "t-live", now))
        .await
        .unwrap();

    for bad in [0, MAX_PRUNE_BATCH + 1] {
        assert!(is_invalid(oauth.prune_expired(now, bad).await));
    }
    let first = oauth.prune_expired(now, 2).await.unwrap();
    assert_eq!((first.requests, first.codes, first.grants), (2, 2, 1));
    let second = oauth.prune_expired(now, 2).await.unwrap();
    assert_eq!((second.requests, second.codes, second.grants), (2, 0, 0));
    let third = oauth.prune_expired(now, 2).await.unwrap();
    assert_eq!((third.requests, third.codes, third.grants), (0, 0, 0));

    assert!(
        oauth
            .get_request(live_request.id, now)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        oauth
            .take_request(edge_request.id, plus(now, -1))
            .await
            .unwrap(),
        None
    );
    assert!(oauth.code_by_hash_any("live").await.unwrap().is_some());
    assert_eq!(oauth.code_by_hash_any("dead-0").await.unwrap(), None);
    assert_eq!(oauth.grant_by_refresh("r-dead").await.unwrap(), None);
    assert_eq!(tokens.get_by_hash("t-dead").await.unwrap(), None);
    assert!(oauth.grant_by_refresh("r-live").await.unwrap().is_some());
    assert!(tokens.get_by_hash("t-live").await.unwrap().is_some());
}

/// Deleting a user deletes their grants (and so their OAuth tokens) and codes; other
/// users' rows stay.
pub async fn deleting_user_cascades_grants(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let tokens = store.read().api_tokens();
    let user = fixture_user(store).await;
    let other = fixture_user(store).await;
    let now = Timestamp::now();
    let c = client("ferc_user_client_00000001", ClientKind::Registered, now);
    oauth.upsert_client(&c).await.unwrap();

    let mine = grant(&c, &user, now, 30 * DAY);
    let theirs = grant(&c, &other, now, 30 * DAY);
    oauth.insert_grant(&mine, "r-mine").await.unwrap();
    oauth.insert_grant(&theirs, "r-theirs").await.unwrap();
    tokens
        .insert(&oauth_token(&user, mine.id, "t-mine", now))
        .await
        .unwrap();
    oauth
        .insert_code(&code(&c, &user, "c-mine", now, MINUTE))
        .await
        .unwrap();
    oauth
        .insert_code(&code(&c, &other, "c-theirs", now, MINUTE))
        .await
        .unwrap();

    store.read().users().delete(user.id).await.unwrap();
    assert_eq!(oauth.grant_by_refresh("r-mine").await.unwrap(), None);
    assert_eq!(oauth.grants_for_user(user.id).await.unwrap(), vec![]);
    assert_eq!(tokens.get_by_hash("t-mine").await.unwrap(), None);
    assert_eq!(oauth.code_by_hash_any("c-mine").await.unwrap(), None);
    assert!(oauth.grant_by_refresh("r-theirs").await.unwrap().is_some());
    assert!(oauth.code_by_hash_any("c-theirs").await.unwrap().is_some());
}

/// The export of `api_tokens` leaves out OAuth access tokens (`Export::RowsWithout`): their
/// grants are not exported, so such rows could not be loaded.
pub async fn export_leaves_out_oauth_tokens(db: &TestDb) {
    let store = db.store();
    let oauth = store.read().oauth();
    let tokens = store.read().api_tokens();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let c = client("ferc_export_client_000001", ClientKind::Registered, now);
    oauth.upsert_client(&c).await.unwrap();
    let g = grant(&c, &user, now, 30 * DAY);
    oauth.insert_grant(&g, "r-export").await.unwrap();
    let access = oauth_token(&user, g.id, "t-export-oauth", now);
    let personal = personal_token(&user, "t-export-personal", now);
    tokens.insert(&access).await.unwrap();
    tokens.insert(&personal).await.unwrap();

    let mut reader = store.export_reader().await.unwrap();
    let rows = dump(reader.as_mut(), spec("api_tokens"), 10).await;
    reader.finish().await.unwrap();
    let ids: Vec<_> = rows.iter().map(|r| r["id"].clone()).collect();
    assert_eq!(ids, vec![json!(personal.id.to_string())], "{rows:?}");
    assert_eq!(rows[0]["oauth_grant_id"], serde_json::Value::Null);
}

/// The loader refuses an `api_tokens` row with `oauth_grant_id` set (`Invalid`, nothing
/// written); a row with the column `null` or absent is accepted.
pub async fn import_refuses_oauth_token_rows(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let base = |id: u128| {
        json!({
            "id": uuid::Uuid::from_u128(id).to_string(),
            "user_id": user.id.to_string(),
            "name": "t", "token_hash": format!("{id:064x}"), "scopes": [],
            "created_at": "2026-01-02T01:02:03.000004Z",
        })
    };
    let mut oauth_row = base(1);
    oauth_row["oauth_grant_id"] = json!(uuid::Uuid::from_u128(9).to_string());
    let mut writer = store.import_writer().await.unwrap();
    assert!(matches!(
        writer.rows(spec("api_tokens"), vec![row(oauth_row)]).await,
        Err(StoreError::Invalid(m)) if m.contains("oauth_grant_id")
    ));
    drop(writer);

    // Null and absent are fine (an archive from before 0013 has no such column).
    let mut with_null = base(2);
    with_null["oauth_grant_id"] = serde_json::Value::Null;
    let mut writer = store.import_writer().await.unwrap();
    writer
        .rows(spec("api_tokens"), vec![row(with_null), row(base(3))])
        .await
        .unwrap();
    drop(writer);
}
