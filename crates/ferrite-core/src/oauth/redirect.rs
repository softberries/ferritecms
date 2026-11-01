//! Redirect URIs (design 12 §3.2): which ones a client may register, and when a requested
//! one matches a registered one.
//!
//! * `https` with a host, or `http` on a loopback host (`localhost`, `127.0.0.1`,
//!   `[::1]`, RFC 8252 §7.3 native apps). Custom schemes (`cursor://…`, `vscode://…`,
//!   RFC 8252 §7.1) are allowed, never `javascript:`, `data:`, `file:` and the other
//!   schemes a browser runs or reads locally.
//! * No fragment and no user information.
//! * A request matches a registered URI exactly; only a loopback URI's port may differ
//!   (RFC 8252 §7.3: the client picks a free port at run time).

use url::{Host, Url};

/// Longest redirect URI accepted, in bytes.
pub(crate) const MAX_REDIRECT_URI: usize = 2000;

/// Schemes never accepted as a redirect target.
const REFUSED_SCHEMES: [&str; 6] = ["javascript", "data", "file", "vbscript", "blob", "about"];

/// Checks a URI a client registers; the error says why it is refused.
pub(crate) fn check_registered(uri: &str) -> Result<Url, String> {
    if uri.is_empty() || uri.len() > MAX_REDIRECT_URI {
        return Err(format!(
            "a redirect URI must be 1 to {MAX_REDIRECT_URI} bytes"
        ));
    }
    let url = Url::parse(uri).map_err(|_| format!("{uri:?} is not a URI"))?;
    if url.fragment().is_some() || uri.contains('#') {
        return Err(format!("{uri:?} has a fragment"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{uri:?} has user information"));
    }
    match url.scheme() {
        "https" if url.host().is_some() => Ok(url),
        "https" => Err(format!("{uri:?} has no host")),
        "http" if is_loopback(&url) => Ok(url),
        "http" => Err(format!(
            "{uri:?}: http is only allowed on localhost, 127.0.0.1 or [::1]"
        )),
        scheme if REFUSED_SCHEMES.contains(&scheme) => {
            Err(format!("the {scheme}: scheme is not allowed"))
        }
        _ if url.cannot_be_a_base() => Err(format!("{uri:?} is not a redirect URI")),
        _ => Ok(url),
    }
}

/// Whether `given` is one of `registered`: the same text, or the same loopback `http`
/// URI on another port.
pub(crate) fn matches(registered: &[String], given: &str) -> bool {
    if given.is_empty() || given.len() > MAX_REDIRECT_URI {
        return false;
    }
    if registered.iter().any(|r| r == given) {
        return true;
    }
    let Ok(given) = Url::parse(given) else {
        return false;
    };
    if given.scheme() != "http" || !is_loopback(&given) || given.fragment().is_some() {
        return false;
    }
    registered
        .iter()
        .filter_map(|r| Url::parse(r).ok())
        .any(|r| {
            r.scheme() == "http"
                && is_loopback(&r)
                && r.host() == given.host()
                && r.path() == given.path()
                && r.query() == given.query()
                && r.username() == given.username()
                && r.password() == given.password()
                && r.fragment().is_none()
        })
}

/// `localhost`, `127.0.0.1` or `[::1]`.
fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => d == "localhost",
        Some(Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
        Some(Host::Ipv6(ip)) => ip == std::net::Ipv6Addr::LOCALHOST,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(uris: &[&str]) -> Vec<String> {
        uris.iter().map(|u| (*u).to_owned()).collect()
    }

    #[test]
    fn loopback_redirect_port_may_differ() {
        let registered = reg(&["http://127.0.0.1:33418/callback", "http://[::1]/cb"]);
        assert!(matches(&registered, "http://127.0.0.1:33418/callback"));
        assert!(matches(&registered, "http://127.0.0.1:50999/callback"));
        assert!(matches(&registered, "http://127.0.0.1/callback"));
        assert!(matches(&registered, "http://[::1]:8123/cb"));
        assert!(!matches(&registered, "http://127.0.0.1:50999/other"));
        assert!(!matches(&registered, "http://127.0.0.1:50999/callback?x=1"));
        assert!(!matches(&registered, "http://localhost:33418/callback"));
        assert!(!matches(&registered, "https://127.0.0.1:33418/callback"));
        assert!(!matches(&registered, "http://127.0.0.1:50999/callback#f"));
        // Other hosts match exactly only.
        let https = reg(&["https://app.example/cb"]);
        assert!(matches(&https, "https://app.example/cb"));
        assert!(!matches(&https, "https://app.example:8443/cb"));
        assert!(!matches(&https, "https://app.example/cb/"));
        assert!(!matches(&https, ""));
    }

    #[test]
    fn registered_uris_are_checked() {
        for ok in [
            "https://app.example/cb",
            "https://app.example/cb?x=1",
            "http://localhost/cb",
            "http://127.0.0.1:8080/cb",
            "http://[::1]:8080/cb",
            "cursor://anysphere.cursor-retrieval/oauth/callback",
            "vscode://vscode.github-authentication/did-authenticate",
            "com.example.app:/oauth2redirect",
        ] {
            assert!(check_registered(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
            "http://example.com/cb",
            "http://127.0.0.2/cb",
            "https://app.example/cb#frag",
            "https://app.example/cb#",
            "https://user:pw@app.example/cb",
            "mailto:a@example.com",
            "not a uri",
            &format!("https://app.example/{}", "a".repeat(MAX_REDIRECT_URI)),
        ] {
            assert!(check_registered(bad).is_err(), "{bad}");
        }
    }
}
