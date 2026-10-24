//! The theme comment form: `POST /_comment` (M2a-2b P12, P13; 06 §4).
//!
//! The second submit path to the node's one [`CommentService`] (the API's
//! `POST /api/v1/content/{type}/{id}/comments` is the other). Mounted as an axum route
//! in front of the public fallback, so it never passes through redirects or the page
//! cache, and every answer carries `Cache-Control: no-store`. Checks, in order:
//!
//! 1. **Origin (the CSRF defence).** The `Origin` header, else the `Referer`'s origin,
//!    must be the site's (`site.url`); anything else, a duplicate header or neither is
//!    `403`. A theme form cannot carry a per-visitor token (cached pages are shared), so
//!    the origin check is what stops other sites posting in a visitor's name, and since
//!    every form comment is **anonymous** (the session cookie is never read here, P12)
//!    a forged post gains no account's rights anyway.
//! 2. `Content-Type` must be `application/x-www-form-urlencoded` (parameters allowed),
//!    else `415`; a declared `Content-Length` over [`MAX_FORM_BODY`] (64 KiB) is `413`
//!    before anything is read, and so is a streamed body that grows past it.
//! 3. No [`CommentService`] (an app built without
//!    [`with_comments`](super::AppState::with_comments)) or no client address
//!    ([`ClientIp`]): `503`, since the per-IP limit could only count one shared bucket
//!    (M2a-2a R13).
//! 4. The fields `post`, `parent`, `author`, `email`, `url`, `comment` and `website`
//!    (the honeypot, [`HONEYPOT_FIELD`]); others are ignored. A repeated field or bytes
//!    that are not UTF-8 are `400`. No `post` is a `422` page; one that is not an id
//!    is `404`, as for an unknown item; a `parent` that is not an id is a `422` page.
//! 5. [`CommentService::submit`] as [`Commenter::Anonymous`] for kind `post`: the
//!    per-IP limits (3 a minute, 20 an hour, in the database), `comments.enabled` and
//!    the item (public `post`), the fields, the honeypot, moderation.
//!
//! Answers: approved → `303 See Other` to `<permalink>#comment-<id>`; pending, or
//! discarded by the honeypot → `303` to `<permalink>#comment-pending`, the same answer
//! byte for byte (the theme shows its "awaiting moderation" notice under `:target`). The
//! permalink comes from [`SiteReader`](ferrite_core::SiteReader), never from the
//! request, so the redirect cannot be pointed elsewhere. Validation → `422` and the
//! limit → `429` with `Retry-After`, each a small built-in HTML page with the message
//! (escaped) and a link back to the post; an unknown, non-public or closed item →
//! `404` plain text; a store failure → `500` plain text.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Response, StatusCode, header},
};
use ferrite_core::{
    CoreError,
    comments::{COMMENT_KIND, CommentInput, CommentService, Commenter, HONEYPOT_FIELD, Submitted},
};
use ferrite_domain::{CommentId, CommentStatus, ContentId};
use html_escape::{encode_double_quoted_attribute, encode_text};
use percent_encoding::percent_decode_str;

use super::{
    AppState,
    client::ClientIp,
    response::{HTML, noindex, text},
};
use crate::api::auth::same_origin;

/// The largest form body (as the API submit's, P13).
pub const MAX_FORM_BODY: usize = 64 * 1024;
const FORM_TYPE: &str = "application/x-www-form-urlencoded";
/// The fragment of a held (or discarded) comment's redirect.
const PENDING_FRAGMENT: &str = "comment-pending";

/// The fields the form sends.
#[derive(Debug, Default, PartialEq, Eq)]
struct Form {
    post: Option<String>,
    parent: Option<String>,
    author: String,
    email: String,
    url: String,
    comment: String,
    website: String,
}

/// Why a form body could not be read (`400`).
#[derive(Debug, PartialEq, Eq)]
enum FormError {
    NotUtf8,
    Repeated,
}

/// `+` is a space, then percent-decoding; the result must be UTF-8.
fn decode(raw: &str) -> Result<String, FormError> {
    let spaced = raw.replace('+', " ");
    percent_decode_str(&spaced)
        .decode_utf8()
        .map(|s| s.into_owned())
        .map_err(|_| FormError::NotUtf8)
}

/// The known fields of an `application/x-www-form-urlencoded` body.
fn parse_form(body: &[u8]) -> Result<Form, FormError> {
    let body = std::str::from_utf8(body).map_err(|_| FormError::NotUtf8)?;
    let mut form = Form::default();
    let mut seen: Vec<String> = Vec::new();
    for pair in body.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(k)?;
        let value = decode(v)?;
        let slot = match key.as_str() {
            "post" => {
                form.post = Some(value);
                "post"
            }
            "parent" => {
                form.parent = Some(value);
                "parent"
            }
            "author" => {
                form.author = value;
                "author"
            }
            "email" => {
                form.email = value;
                "email"
            }
            "url" => {
                form.url = value;
                "url"
            }
            "comment" => {
                form.comment = value;
                "comment"
            }
            k if k == HONEYPOT_FIELD => {
                form.website = value;
                HONEYPOT_FIELD
            }
            _ => continue,
        };
        if seen.iter().any(|s| s == slot) {
            return Err(FormError::Repeated);
        }
        seen.push(slot.to_owned());
    }
    Ok(form)
}

/// Whether `value` is `application/x-www-form-urlencoded` (any parameters).
fn is_form_type(value: Option<&HeaderValue>) -> bool {
    value
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case(FORM_TYPE))
}

/// The built-in error page: the escaped `message` and a link to `back`.
fn error_page(status: StatusCode, title: &str, message: &str, back: &str) -> Response<Body> {
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n</head>\n<body>\n\
         <main>\n<h1>{title}</h1>\n<p>{message}</p>\n<p><a href=\"{back}\">Back to the post</a></p>\n\
         </main>\n</body>\n</html>\n",
        title = encode_text(title),
        message = encode_text(message),
        back = encode_double_quoted_attribute(back),
    );
    let mut res = Response::new(Body::from(html));
    *res.status_mut() = status;
    let headers = res.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(HTML));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    noindex(&mut res);
    res
}

fn not_posted(message: &str, back: &str) -> Response<Body> {
    error_page(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Your comment was not posted",
        message,
        back,
    )
}

fn see_other(location: &str) -> Response<Body> {
    let location = HeaderValue::from_str(location).unwrap_or_else(|_| {
        tracing::warn!("comment redirect target is not a header value; sent to /");
        HeaderValue::from_static("/")
    });
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::SEE_OTHER;
    let headers = res.headers_mut();
    headers.insert(header::LOCATION, location);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// The public URL of `post` (root-relative, from the permalinks), else `/`: where
/// redirects and "back" links go. Never anything the request named.
async fn permalink(state: &AppState, post: ContentId) -> String {
    match state.reader.by_id(post).await {
        Ok(read) => read.value.map_or_else(|| "/".to_owned(), |p| p.url),
        Err(e) => {
            tracing::warn!(error = %e, "comment form permalink lookup failed");
            "/".to_owned()
        }
    }
}

/// The handler behind `POST /_comment`; see the module docs.
pub(crate) async fn handle(State(state): State<Arc<AppState>>, req: Request) -> Response<Body> {
    let site_origin = state
        .origin
        .as_deref()
        .is_some_and(|origin| same_origin(req.headers(), origin));
    if !site_origin {
        return text(StatusCode::FORBIDDEN, "forbidden\n");
    }
    if !is_form_type(req.headers().get(header::CONTENT_TYPE)) {
        return text(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported media type\n",
        );
    }
    let too_large = || text(StatusCode::PAYLOAD_TOO_LARGE, "payload too large\n");
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > u64::try_from(MAX_FORM_BODY).unwrap_or(u64::MAX)) {
        return too_large();
    }
    let Some(service) = state.comments.clone() else {
        tracing::warn!("comment form without a comment service; refused");
        return text(StatusCode::SERVICE_UNAVAILABLE, "service unavailable\n");
    };
    let Some(ip) = req.extensions().get::<ClientIp>().map(|c| c.0) else {
        tracing::warn!("comment form without a client address; refused");
        return text(StatusCode::SERVICE_UNAVAILABLE, "service unavailable\n");
    };
    // Over the limit, or a body that broke off: nothing is read further.
    let Ok(body) = axum::body::to_bytes(req.into_body(), MAX_FORM_BODY).await else {
        return too_large();
    };
    let form = match parse_form(&body) {
        Ok(f) => f,
        Err(_) => return text(StatusCode::BAD_REQUEST, "bad request\n"),
    };
    submit(&state, &service, form, ip).await
}

async fn submit(
    state: &AppState,
    service: &CommentService,
    form: Form,
    ip: std::net::IpAddr,
) -> Response<Body> {
    let not_found = || text(StatusCode::NOT_FOUND, "not found\n");
    let Some(post) = form
        .post
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    else {
        return not_posted("The form names no post.", "/");
    };
    let Ok(post) = post.parse::<ContentId>() else {
        return not_found();
    };
    let parent = match form.parent.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(p) => match p.parse::<CommentId>() {
            Ok(id) => Some(id),
            Err(_) => {
                return not_posted(
                    "the reply must be to an approved comment of the same post",
                    &permalink(state, post).await,
                );
            }
        },
    };
    let input = CommentInput {
        item: post,
        kind: COMMENT_KIND.to_owned(),
        parent,
        author_name: form.author,
        author_email: form.email,
        author_url: form.url,
        body: form.comment,
        honeypot: form.website,
    };
    match service.submit(input, Commenter::Anonymous, ip).await {
        // A discard answers exactly as a held comment (F18).
        Ok(Submitted::Discarded(_)) => pending(state, post).await,
        Ok(Submitted::Accepted(c)) if c.status == CommentStatus::Approved => see_other(&format!(
            "{}#comment-{}",
            permalink(state, post).await,
            c.id
        )),
        Ok(Submitted::Accepted(_)) => pending(state, post).await,
        Err(CoreError::NotFound) => not_found(),
        Err(
            CoreError::InvalidField { message, .. }
            | CoreError::InvalidKey { message, .. }
            | CoreError::Invalid(message),
        ) => not_posted(&message, &permalink(state, post).await),
        Err(CoreError::RateLimited { retry_after_secs }) => {
            let mut res = error_page(
                StatusCode::TOO_MANY_REQUESTS,
                "Slow down",
                &format!(
                    "You are posting comments too quickly. Try again in {retry_after_secs} seconds."
                ),
                &permalink(state, post).await,
            );
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
            res
        }
        Err(e) => {
            tracing::error!(error = %e, "comment form submit failed");
            text(StatusCode::INTERNAL_SERVER_ERROR, "internal server error\n")
        }
    }
}

async fn pending(state: &AppState, post: ContentId) -> Response<Body> {
    see_other(&format!(
        "{}#{PENDING_FRAGMENT}",
        permalink(state, post).await
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_are_parsed_strictly() {
        let f = parse_form(b"post=1&author=Ann+B&comment=a%26b%0Ac&website=&submit=Send").unwrap();
        assert_eq!(f.post.as_deref(), Some("1"));
        assert_eq!(f.author, "Ann B");
        assert_eq!(f.comment, "a&b\nc");
        assert_eq!(f.website, "");
        assert_eq!(f.parent, None);
        assert_eq!(parse_form(b"").unwrap(), Form::default());
        assert_eq!(parse_form(b"a=%FF"), Err(FormError::NotUtf8));
        assert_eq!(parse_form(b"comment=%FF"), Err(FormError::NotUtf8));
        assert_eq!(parse_form(b"\xff=1"), Err(FormError::NotUtf8));
        assert_eq!(parse_form(b"url=a&url=b"), Err(FormError::Repeated));
        assert_eq!(parse_form(b"x=1&x=2").unwrap(), Form::default());
        assert_eq!(parse_form(b"comment=%3Cb%3E").unwrap().comment, "<b>");
    }

    #[test]
    fn form_type_allows_parameters_only() {
        let v = |s: &'static str| HeaderValue::from_static(s);
        assert!(is_form_type(Some(&v("application/x-www-form-urlencoded"))));
        assert!(is_form_type(Some(&v(
            "Application/X-WWW-Form-Urlencoded ; charset=UTF-8"
        ))));
        assert!(!is_form_type(None));
        assert!(!is_form_type(Some(&v("multipart/form-data; boundary=x"))));
        assert!(!is_form_type(Some(&v(
            "application/x-www-form-urlencodedx"
        ))));
        assert!(!is_form_type(Some(&v("text/plain"))));
    }

    #[tokio::test]
    async fn error_pages_escape_message_and_link() {
        let res = error_page(
            StatusCode::UNPROCESSABLE_ENTITY,
            "T<",
            "<script>alert(1)</script>",
            "/a\"onmouseover=\"x",
        );
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(res.headers()["x-robots-tag"], "noindex");
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            body.contains("<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>"),
            "{body}"
        );
        assert!(body.contains("<title>T&lt;</title>"), "{body}");
        assert!(
            body.contains("href=\"/a&quot;onmouseover=&quot;x\""),
            "{body}"
        );
        assert!(!body.contains("<script>"));
    }
}
