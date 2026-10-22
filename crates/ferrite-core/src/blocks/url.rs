//! URL allow-listing for link hrefs and image sources.

use uuid::Uuid;

/// Splits off a scheme the way browsers do: ASCII whitespace/control chars are ignored
/// inside it, and it must come before any `/`, `?` or `#`.
fn scheme(s: &str) -> Option<String> {
    let mut scheme = String::new();
    for c in s.chars() {
        match c {
            ':' => return Some(scheme.to_ascii_lowercase()),
            '/' | '?' | '#' => return None,
            c if c.is_ascii_whitespace() || c.is_ascii_control() => {}
            c => scheme.push(c),
        }
    }
    None
}

/// Absolute path on this host: `/x`, not `//x` and not `/\x` (both protocol-relative).
fn is_local_path(s: &str) -> bool {
    s.starts_with('/') && !s.starts_with("//") && !s.starts_with("/\\")
}

/// Returns the trimmed href when it is safe to emit, else `None`.
pub(super) fn safe_href(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() || s.chars().any(|c| c.is_control()) || s.contains('\\') {
        return None;
    }
    if s.starts_with('#') || is_local_path(s) {
        return Some(s.to_owned());
    }
    match scheme(s) {
        Some(sch) if matches!(sch.as_str(), "http" | "https" | "mailto") => Some(s.to_owned()),
        Some(_) => None,
        None if s.starts_with("//") => None,
        None => Some(s.to_owned()),
    }
}

pub(super) enum ImageSrc {
    Url(String),
    Media(String),
}

/// Classifies an image `src`; `None` means drop the image.
pub(super) fn image_src(raw: &str) -> Option<ImageSrc> {
    let s = raw.trim();
    if s.is_empty() || s.chars().any(|c| c.is_control()) || s.contains('\\') {
        return None;
    }
    if let Some(id) = s.strip_prefix("ferrite:media/") {
        let id = Uuid::parse_str(id).ok()?;
        return Some(ImageSrc::Media(id.hyphenated().to_string()));
    }
    if is_local_path(s) {
        return Some(ImageSrc::Url(s.to_owned()));
    }
    match scheme(s) {
        Some(sch) if matches!(sch.as_str(), "http" | "https") => Some(ImageSrc::Url(s.to_owned())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrefs() {
        for ok in [
            "http://a.b/",
            "HTTPS://a.b",
            "mailto:a@b.c",
            "/x/y",
            "#top",
            "page/2",
            " /x ",
        ] {
            assert!(safe_href(ok).is_some(), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "java\nscript:alert(1)",
            "java\tscript:alert(1)",
            " javascript:alert(1)",
            "data:text/html,x",
            "vbscript:x",
            "//evil.com",
            "/\\evil.com",
            "\\\\evil.com",
            "",
            "ftp://x",
            "file:///etc/passwd",
        ] {
            assert!(safe_href(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn images() {
        let id = "0190f1c0-0000-7000-8000-000000000001";
        assert!(
            matches!(image_src(&format!("ferrite:media/{id}")), Some(ImageSrc::Media(m)) if m == id)
        );
        assert!(image_src("ferrite:media/not-a-uuid").is_none());
        assert!(image_src("data:image/png;base64,AAAA").is_none());
        assert!(image_src("//evil.com/x.png").is_none());
        assert!(image_src("javascript:x").is_none());
        assert!(image_src("mailto:a@b.c").is_none());
        assert!(image_src("x.png").is_none());
        assert!(image_src("/media/x.png").is_some());
        assert!(image_src("https://a.b/x.png").is_some());
    }
}
