//! The session cookie, written and read by hand (P7, P30: no `axum-extra`).
//!
//! `<name>=<token>; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000`, plus `; Secure`
//! when `site.url` is `https` ([`ApiState::cookie_secure`](super::ApiState)). The name
//! follows the same flag (M2a-2b P23): [`HOST_SESSION_COOKIE`] (`__Host-ferrite_session`,
//! which browsers accept only with `Secure`, `Path=/` and no `Domain`) on an `https`
//! site, [`SESSION_COOKIE`] (`ferrite_session`) on an `http` one. The flag comes from
//! `site.url`, not from the request, so a TLS-terminating proxy changes nothing. Only the
//! site's name is read: a cookie under the other name is ignored (no transition, so
//! sessions from before the rename log in once more). `Max-Age` is the absolute session
//! lifetime (30 days); the idle timeout is enforced by the server. Tokens are base64url,
//! so they never need quoting or escaping.

use axum::http::{HeaderMap, HeaderValue, header};
use ferrite_domain::SESSION_ABSOLUTE;

use super::openapi::{HOST_SESSION_COOKIE, SESSION_COOKIE};

/// The session cookie's name on a site whose `site.url` is `https` (`secure`) or not.
pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        HOST_SESSION_COOKIE
    } else {
        SESSION_COOKIE
    }
}

/// `Set-Cookie` for a new session.
pub fn session_cookie(token: &str, secure: bool) -> HeaderValue {
    cookie(token, SESSION_ABSOLUTE.as_secs(), secure)
}

/// `Set-Cookie` that removes the session cookie (`Max-Age=0`).
pub fn clear_cookie(secure: bool) -> HeaderValue {
    cookie("", 0, secure)
}

fn cookie(value: &str, max_age: u64, secure: bool) -> HeaderValue {
    let name = session_cookie_name(secure);
    let flag = if secure { "; Secure" } else { "" };
    let text = format!("{name}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{flag}");
    // Our tokens are base64url: always a valid header value. Anything else (never the
    // case) clears the cookie instead of failing the response.
    HeaderValue::from_str(&text).unwrap_or_else(|_| {
        tracing::error!("session cookie value is not a valid header value");
        HeaderValue::from_static(fallback_clear(secure))
    })
}

/// The clearing cookie as a literal (`from_static` needs one). It must match
/// [`clear_cookie`] and the name constants; `fallback_matches_clear_cookie` checks it.
fn fallback_clear(secure: bool) -> &'static str {
    if secure {
        "__Host-ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure"
    } else {
        "ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
    }
}

/// The value of the first session cookie (the name for `secure`, see the module docs)
/// in any `Cookie` header (`None` when absent or empty). Header lines that are not
/// visible ASCII are skipped.
pub fn read_session_cookie(headers: &HeaderMap, secure: bool) -> Option<&str> {
    let wanted = session_cookie_name(secure);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|line| line.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name.trim() == wanted).then(|| value.trim().trim_matches('"'))
        })
        .filter(|v| !v.is_empty())
}

/// The whole-site preview cookie's name (M6-3, design 15 §5.3): `__Host-ferrite_preview`
/// on an `https` site, `ferrite_preview` on an `http` one.
pub fn preview_cookie_name(secure: bool) -> &'static str {
    if secure {
        "__Host-ferrite_preview"
    } else {
        "ferrite_preview"
    }
}

/// `Set-Cookie` of a preview token living `max_age` seconds (`0` clears it). Preview
/// tokens are `ferp_` + base64url: always a valid header value.
pub fn preview_cookie(token: &str, max_age: u64, secure: bool) -> HeaderValue {
    let name = preview_cookie_name(secure);
    let flag = if secure { "; Secure" } else { "" };
    let text = format!("{name}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{flag}");
    HeaderValue::from_str(&text).unwrap_or_else(|_| {
        HeaderValue::from_static(if secure {
            "__Host-ferrite_preview=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure"
        } else {
            "ferrite_preview=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
        })
    })
}

/// The value of the preview cookie, if any.
pub fn read_preview_cookie(headers: &HeaderMap, secure: bool) -> Option<&str> {
    let wanted = preview_cookie_name(secure);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|line| line.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name.trim() == wanted).then(|| value.trim().trim_matches('"'))
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
            "__Host-ferrite_session=abc; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000; Secure"
        );
        assert_eq!(
            clear_cookie(true),
            "__Host-ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure"
        );
        assert_eq!(
            clear_cookie(false),
            "ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
        );
    }

    fn read_session_cookie_http(h: &HeaderMap) -> Option<&str> {
        read_session_cookie(h, false)
    }

    /// Review m3: the literal fallback names the same cookie as the constants.
    #[test]
    fn fallback_matches_clear_cookie() {
        for secure in [true, false] {
            assert_eq!(clear_cookie(secure), fallback_clear(secure));
            assert!(
                fallback_clear(secure).starts_with(&format!("{}=;", session_cookie_name(secure)))
            );
        }
        assert!(fallback_clear(true).starts_with(&format!("{HOST_SESSION_COOKIE}=")));
        assert!(fallback_clear(false).starts_with(&format!("{SESSION_COOKIE}=")));
    }

    /// P23: an `https` site reads only `__Host-ferrite_session`, an `http` site only
    /// `ferrite_session`.
    #[test]
    fn name_follows_the_site_scheme() {
        let both = headers(&[b"ferrite_session=old; __Host-ferrite_session=new"]);
        assert_eq!(read_session_cookie(&both, true), Some("new"));
        assert_eq!(read_session_cookie(&both, false), Some("old"));
        let old = headers(&[b"ferrite_session=old"]);
        assert_eq!(read_session_cookie(&old, true), None);
        let new = headers(&[b"__Host-ferrite_session=new"]);
        assert_eq!(read_session_cookie(&new, false), None);
        assert_eq!(session_cookie_name(true), "__Host-ferrite_session");
        assert_eq!(session_cookie_name(false), "ferrite_session");
    }

    #[test]
    fn reads_the_first_session_cookie() {
        assert_eq!(
            read_session_cookie_http(&headers(&[b"ferrite_session=t1"])),
            Some("t1")
        );
        assert_eq!(
            read_session_cookie_http(&headers(&[b"a=1; ferrite_session=t2 ; b=2"])),
            Some("t2")
        );
        assert_eq!(
            read_session_cookie_http(&headers(&[
                b"a=1",
                b"ferrite_session=t3",
                b"ferrite_session=t4"
            ])),
            Some("t3")
        );
        assert_eq!(
            read_session_cookie_http(&headers(&[b"ferrite_session=t5; ferrite_session=t6"])),
            Some("t5")
        );
        assert_eq!(
            read_session_cookie_http(&headers(&[b"ferrite_session=\"t7\""])),
            Some("t7")
        );
        // Not ours, empty, malformed, non-ASCII lines.
        assert_eq!(
            read_session_cookie_http(&headers(&[b"xferrite_session=t"])),
            None
        );
        assert_eq!(
            read_session_cookie_http(&headers(&[b"ferrite_session="])),
            None
        );
        assert_eq!(
            read_session_cookie_http(&headers(&[b"ferrite_session"])),
            None
        );
        assert_eq!(
            read_session_cookie_http(&headers(&[
                b"\xff=1; ferrite_session=x",
                b"ferrite_session=t8"
            ])),
            Some("t8")
        );
        assert_eq!(read_session_cookie_http(&HeaderMap::new()), None);
    }
}
