//! The one rule for a homepage URL in a theme or plugin manifest (M2b-3): what the
//! admin may render as a link.

/// Longest homepage URL accepted.
pub const MAX_WEB_URL: usize = 2048;

/// Whether `url` is `http://` or `https://` followed by a non-empty host part (not
/// starting with `/`), with no whitespace or control characters, at most
/// [`MAX_WEB_URL`] bytes. The scheme is matched in lowercase only.
pub fn is_web_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    url.len() <= MAX_WEB_URL
        && rest.is_some_and(|r| {
            !r.is_empty()
                && !r.starts_with('/')
                && !r.chars().any(|c| c.is_whitespace() || c.is_control())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_and_https_with_a_host() {
        for ok in ["https://example.com", "http://a.b/c?d#e", "https://x"] {
            assert!(is_web_url(ok), "{ok}");
        }
        let long = format!("https://{}", "a".repeat(MAX_WEB_URL));
        for bad in [
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "ftp://example.com",
            "HTTPS://example.com",
            "https://",
            "https:///path",
            "https://a b",
            "https://a\nb",
            "//example.com",
            "",
            long.as_str(),
        ] {
            assert!(!is_web_url(bad), "{bad:?}");
        }
    }
}
