//! URL allow-listing for link hrefs and image sources, and video ids of embeds.

use ferrite_domain::html::SAFE_URL_SCHEMES;
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

/// Returns the trimmed href when it is safe to emit, else `None`: `http`, `https`, `mailto`,
/// `#fragment`, `/path` or a relative path.
pub fn safe_href(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() || s.chars().any(|c| c.is_control()) || s.contains('\\') {
        return None;
    }
    if s.starts_with('#') || is_local_path(s) {
        return Some(s.to_owned());
    }
    match scheme(s) {
        Some(sch) if SAFE_URL_SCHEMES.contains(&sch.as_str()) => Some(s.to_owned()),
        Some(_) => None,
        // Leading `//` is protocol-relative; leading `&` could be an entity-encoded scheme.
        None if s.starts_with("//") || s.starts_with('&') => None,
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

/// The video id of an `embed` node's `url` for `provider` (`youtube` or `vimeo`, P11),
/// or `None` when the URL is not one of that provider's video URLs. Only `http(s)` URLs
/// on the provider's own hosts, without user info, count:
///
/// - YouTube (`youtube.com`, `www.`, `m.`, `youtube-nocookie.com`, `youtu.be`):
///   `/watch?v=ID`, `youtu.be/ID`, `/embed/ID` and `/shorts/ID`, where `ID` is exactly 11
///   characters of `[A-Za-z0-9_-]`;
/// - Vimeo (`vimeo.com`, `www.`, `player.vimeo.com`): the first all-digit path segment
///   (`/76979871`, `/channels/staffpicks/76979871`, `/video/76979871`).
///
/// The admin's twin is `admin/src/editor/embed.ts` `parseEmbed`; both are pinned to the
/// cases in `tests/fixtures/editor-schema.json`.
pub fn embed_id(provider: &str, raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.username() != "" || url.password().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let mut segments = url.path_segments()?.filter(|s| !s.is_empty());
    match provider {
        "youtube" => {
            let id = match host {
                "youtu.be" => segments.next()?.to_owned(),
                "youtube.com"
                | "www.youtube.com"
                | "m.youtube.com"
                | "youtube-nocookie.com"
                | "www.youtube-nocookie.com" => match (segments.next()?, segments.next()) {
                    ("watch", None) => url
                        .query_pairs()
                        .find(|(k, _)| k == "v")
                        .map(|(_, v)| v.into_owned())?,
                    ("embed" | "shorts", Some(id)) => id.to_owned(),
                    _ => return None,
                },
                _ => return None,
            };
            let ok = id.len() == 11
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
            ok.then_some(id)
        }
        "vimeo" => {
            if !matches!(host, "vimeo.com" | "www.vimeo.com" | "player.vimeo.com") {
                return None;
            }
            segments
                .find(|s| (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
                .map(str::to_owned)
        }
        _ => None,
    }
}

/// The URL an `embed` node stores for video `id` of `provider`: its plain watch page,
/// `https://www.youtube.com/watch?v=ID` or `https://vimeo.com/ID` (the admin's
/// `canonicalEmbedUrl`; both pinned to `tests/fixtures/editor-schema.json`). `None` for
/// any other provider. `id` is used as given: pass one [`embed_id`] returned.
pub fn canonical_embed_url(provider: &str, id: &str) -> Option<String> {
    match provider {
        "youtube" => Some(format!("https://www.youtube.com/watch?v={id}")),
        "vimeo" => Some(format!("https://vimeo.com/{id}")),
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

    /// The URL cases the admin's embed node shares (`editor-schema.json`): `id: null`
    /// means no provider recognises the URL.
    #[test]
    fn embed_ids_match_the_shared_cases() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/editor-schema.json")).unwrap();
        let cases = schema["embed_cases"].as_array().unwrap();
        assert!(cases.len() > 10);
        for case in cases {
            let url = case["url"].as_str().unwrap();
            match (case["provider"].as_str(), case["id"].as_str()) {
                (Some(provider), Some(id)) => {
                    assert_eq!(embed_id(provider, url).as_deref(), Some(id), "{url}");
                    let other = if provider == "youtube" {
                        "vimeo"
                    } else {
                        "youtube"
                    };
                    assert_eq!(embed_id(other, url), None, "{other} {url}");
                }
                _ => {
                    for provider in ["youtube", "vimeo"] {
                        assert_eq!(embed_id(provider, url), None, "{provider} {url}");
                    }
                }
            }
        }
        assert_eq!(
            embed_id("dailymotion", "https://youtu.be/dQw4w9WgXcQ"),
            None
        );
        assert_eq!(embed_id("YouTube", "https://youtu.be/dQw4w9WgXcQ"), None);
    }

    #[test]
    fn canonical_urls_match_the_shared_cases() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/editor-schema.json")).unwrap();
        let cases = schema["embed_canonical"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let (provider, id, url) = (
                case["provider"].as_str().unwrap(),
                case["id"].as_str().unwrap(),
                case["url"].as_str().unwrap(),
            );
            assert_eq!(canonical_embed_url(provider, id).as_deref(), Some(url));
            // It parses back to the same video.
            assert_eq!(embed_id(provider, url).as_deref(), Some(id));
        }
        assert_eq!(canonical_embed_url("dailymotion", "x"), None);
    }
}
