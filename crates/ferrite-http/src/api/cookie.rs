//! The session cookie, written and read by hand (P7, P30: no `axum-extra`).
//!
//! `ferrite_session=<token>; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000`, plus
//! `; Secure` when `site.url` is `https` ([`ApiState::cookie_secure`](super::ApiState)).
//! `Max-Age` is the absolute session lifetime (30 days); the idle timeout is enforced
//! by the server. Tokens are base64url, so they never need quoting or escaping.

use axum::http::{HeaderMap, HeaderValue, header};
use ferrite_domain::SESSION_ABSOLUTE;

use super::openapi::SESSION_COOKIE;

/// `Set-Cookie` for a new session.
pub fn session_cookie(token: &str, secure: bool) -> HeaderValue {
    cookie(token, SESSION_ABSOLUTE.as_secs(), secure)
}

/// `Set-Cookie` that removes the session cookie (`Max-Age=0`).
pub fn clear_cookie(secure: bool) -> HeaderValue {
    cookie("", 0, secure)
}

fn cookie(value: &str, max_age: u64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    let text = format!(
        "{SESSION_COOKIE}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}"
    );
    // Our tokens are base64url: always a valid header value. Anything else (never the
    // case) clears the cookie instead of failing the response.
    HeaderValue::from_str(&text).unwrap_or_else(|_| {
        tracing::error!("session cookie value is not a valid header value");
        HeaderValue::from_static("ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
    })
}

/// The value of the first `ferrite_session` cookie in any `Cookie` header (`None` when
/// absent or empty). Header lines that are not visible ASCII are skipped.
pub fn read_session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|line| line.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name.trim() == SESSION_COOKIE).then(|| value.trim().trim_matches('"'))
        })
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(lines: &[&[u8]]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for l in lines {
            h.append(header::COOKIE, HeaderValue::from_bytes(l).unwrap());
        }
        h
    }

    #[test]
    fn cookie_attributes() {
        assert_eq!(
            session_cookie("abc", false),
            "ferrite_session=abc; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000"
        );
        assert_eq!(
            session_cookie("abc", true),
            "ferrite_session=abc; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000; Secure"
        );
        assert_eq!(
            clear_cookie(true),
            "ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure"
        );
        assert_eq!(
            clear_cookie(false),
            "ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
        );
    }

    #[test]
    fn reads_the_first_session_cookie() {
        assert_eq!(
            read_session_cookie(&headers(&[b"ferrite_session=t1"])),
            Some("t1")
        );
        assert_eq!(
            read_session_cookie(&headers(&[b"a=1; ferrite_session=t2 ; b=2"])),
            Some("t2")
        );
        assert_eq!(
            read_session_cookie(&headers(&[
                b"a=1",
                b"ferrite_session=t3",
                b"ferrite_session=t4"
            ])),
            Some("t3")
        );
        assert_eq!(
            read_session_cookie(&headers(&[b"ferrite_session=t5; ferrite_session=t6"])),
            Some("t5")
        );
        assert_eq!(
            read_session_cookie(&headers(&[b"ferrite_session=\"t7\""])),
            Some("t7")
        );
        // Not ours, empty, malformed, non-ASCII lines.
        assert_eq!(
            read_session_cookie(&headers(&[b"xferrite_session=t"])),
            None
        );
        assert_eq!(read_session_cookie(&headers(&[b"ferrite_session="])), None);
        assert_eq!(read_session_cookie(&headers(&[b"ferrite_session"])), None);
        assert_eq!(
            read_session_cookie(&headers(&[
                b"\xff=1; ferrite_session=x",
                b"ferrite_session=t8"
            ])),
            Some("t8")
        );
        assert_eq!(read_session_cookie(&HeaderMap::new()), None);
    }
}
