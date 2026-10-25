//! The URL policy shared by every HTML sanitiser (M2a-2b P21): content blocks' `html`
//! nodes (`ferrite-core`) and plugin `html` results (`ferrite-plugin-host`) apply the
//! same rules, so a link that one drops the other drops too.
//!
//! Absolute URLs need a scheme in [`SAFE_URL_SCHEMES`]; relative ones must pass
//! [`relative_url_allowed`].

/// The schemes an absolute link or image URL may use in sanitised HTML. `ftp:`, `tel:`,
/// `data:`, `javascript:` and every other scheme are dropped.
pub const SAFE_URL_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Whether a relative URL (one without a scheme) may stay in sanitised HTML: root-relative
/// (`/x`), fragment (`#a`), query (`?q`) and path-relative (`x/y`) text. Refused:
/// protocol-relative `//host…`, anything with a backslash (browsers read `\` as `/`, so
/// `\\host` and `/\host` are protocol-relative too) and anything with a control
/// character. The empty string is refused as well.
pub fn relative_url_allowed(url: &str) -> bool {
    !url.is_empty() && !url.starts_with("//") && !url.chars().any(|c| c == '\\' || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_urls() {
        for ok in [
            "/x",
            "/x/y?q=1#f",
            "#a",
            "?q",
            "x/y",
            "page.html",
            "./x",
            "../x",
        ] {
            assert!(relative_url_allowed(ok), "{ok:?} refused");
        }
        for bad in [
            "",
            "//evil.example/x",
            "\\\\evil.example/x",
            "/\\evil.example/x",
            "x\\y",
            "/x\ny",
            "/x\ty",
            "/x\u{0}",
            "/x\u{7f}",
            "/x\u{85}",
        ] {
            assert!(!relative_url_allowed(bad), "{bad:?} allowed");
        }
        assert_eq!(SAFE_URL_SCHEMES, ["http", "https", "mailto"]);
    }
}
