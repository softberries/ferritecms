//! Endpoint URL rules (design 18 §6.1), checked on create and update.

use std::net::IpAddr;

use url::{Host, Url};

use crate::CoreError;

/// Longest endpoint URL, in bytes.
pub const MAX_URL: usize = 2048;

/// What endpoint URLs may point at. `allow_ip` is the transport's address rule (the
/// plugin egress' public-address predicate, or with `local_targets` the one that also
/// allows loopback and private ranges), so core and the transport share one rule.
#[derive(Debug, Clone, Copy)]
pub struct UrlPolicy {
    /// `[webhooks] local_targets`: `http` allowed too.
    pub local_targets: bool,
    pub allow_ip: fn(IpAddr) -> bool,
}

/// Checks `raw`: absolute, `https` (or `http` with `local_targets`), a host, no user
/// information, no fragment, at most [`MAX_URL`] bytes, an IP-literal host allowed by
/// `policy.allow_ip`. A query string and any port are fine. Else `InvalidField("url")`.
pub fn check_url(raw: &str, policy: &UrlPolicy) -> Result<Url, CoreError> {
    let bad = |m: &str| CoreError::invalid_field("url", m.to_owned());
    if raw.len() > MAX_URL {
        return Err(bad("at most 2048 bytes"));
    }
    if raw.trim() != raw || raw.is_empty() {
        return Err(bad("enter an absolute https:// URL"));
    }
    let url = Url::parse(raw).map_err(|_| bad("enter an absolute https:// URL"))?;
    match url.scheme() {
        "https" => {}
        "http" if policy.local_targets => {}
        _ => return Err(bad("the URL must use https")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("the URL must not hold a user name or password"));
    }
    if url.fragment().is_some() {
        return Err(bad("the URL must not have a #fragment"));
    }
    let ip = match url.host() {
        None => return Err(bad("the URL needs a host")),
        Some(Host::Domain("")) => return Err(bad("the URL needs a host")),
        Some(Host::Domain(_)) => None,
        Some(Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
        Some(Host::Ipv6(v6)) => Some(IpAddr::V6(v6)),
    };
    if let Some(ip) = ip
        && !(policy.allow_ip)(ip)
    {
        return Err(bad("this address is not allowed"));
    }
    Ok(url)
}

/// The host of a URL, for audit details and logs (never the path or query, which can
/// hold a token).
pub fn host_of(raw: &str) -> String {
    Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn public(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => !(v4.is_private() || v4.is_loopback() || v4.is_link_local()),
            IpAddr::V6(v6) => !v6.is_loopback(),
        }
    }

    fn local(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => !v4.is_link_local(),
            IpAddr::V6(_) => true,
        }
    }

    #[test]
    fn url_rules() {
        let strict = UrlPolicy {
            local_targets: false,
            allow_ip: public,
        };
        let lax = UrlPolicy {
            local_targets: true,
            allow_ip: local,
        };
        let ok = |u: &str, p: &UrlPolicy| check_url(u, p).is_ok();
        assert!(ok("https://hooks.example.com/x?token=abc", &strict));
        assert!(ok("https://hooks.example.com:8443/", &strict));
        assert!(ok("https://203.0.113.9/hook", &strict));
        for bad in [
            "http://hooks.example.com/",
            "ftp://hooks.example.com/",
            "https://user:pw@hooks.example.com/",
            "https://user@hooks.example.com/",
            "https://hooks.example.com/#frag",
            "/relative",
            "https://",
            "https://10.0.0.1/",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://169.254.169.254/latest",
            " https://hooks.example.com/",
        ] {
            assert!(
                matches!(
                    check_url(bad, &strict),
                    Err(CoreError::InvalidField { field: "url", .. })
                ),
                "{bad}"
            );
        }
        let long = format!("https://hooks.example.com/{}", "a".repeat(2049));
        assert!(check_url(&long, &strict).is_err());
        let exact = format!("https://h.example/{}", "a".repeat(MAX_URL - 18));
        assert_eq!(exact.len(), MAX_URL);
        assert!(ok(&exact, &strict));
        // Local targets: http and loopback, never the metadata range.
        assert!(ok("http://127.0.0.1:9000/hook", &lax));
        assert!(ok("http://localhost:9000/hook", &lax));
        assert!(!ok("http://169.254.169.254/", &lax));
        assert_eq!(
            host_of("https://hooks.example.com/x?token=abc"),
            "hooks.example.com"
        );
    }
}
