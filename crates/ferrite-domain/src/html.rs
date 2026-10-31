//! The URL policy shared by every HTML sanitiser (M2a-2b P21): content blocks' `html`
//! nodes (`ferrite-core`) and plugin `html` results (`ferrite-plugin-host`) apply the
//! same rules, so a link that one drops the other drops too.
//!
//! Absolute URLs need a scheme in [`SAFE_URL_SCHEMES`]; relative ones must pass
//! [`relative_url_allowed`].

/// The schemes an absolute link or image URL may use in sanitised HTML. `ftp:`, `tel:`,
/// `data:`, `javascript:` and every other scheme are dropped.
pub const SAFE_URL_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Elements whose `cite` attribute sanitisers drop: ammonia allows `cite` on them but
/// does not treat it as a URL attribute, so it would bypass this policy (it is not
/// navigable today, but nothing outside the policy should carry a URL).
pub const CITE_TAGS: [&str; 4] = ["blockquote", "del", "ins", "q"];

/// Whether a relative URL (one without a scheme) may stay in sanitised HTML: root-relative
/// (`/x`), fragment (`#a`), query (`?q`) and path-relative (`x/y`) text. Refused:
/// protocol-relative `//host…`, anything with a backslash (browsers read `\` as `/`, so
/// `\\host` and `/\host` are protocol-relative too), anything with a control character,
/// the empty string, and anything that starts with a space or a C0 control (U+0000 to
/// U+0020): the WHATWG URL parser strips those before parsing, so ` //host` is
/// protocol-relative in a browser. ammonia hands this function the raw attribute value
/// (entities decoded, nothing trimmed), so `&#32;//host` arrives as ` //host`.
pub fn relative_url_allowed(url: &str) -> bool {
    url.chars().next().is_some_and(|c| c > '\u{20}')
        && !url.starts_with("//")
        && !url.chars().any(|c| c == '\\' || c.is_control())
}

/// Fragment ids that theme landmarks, core markup and the starter themes' fixed ids use
/// (M4-3b P54). The block renderer seeds its heading-id counter with them, so a heading
/// "Content" gets `content-1` and never collides with a landmark, and the plugin
/// sanitiser drops them from headings ([`heading_id_allowed`]). Every fixed id of
/// `themes/{blog,business,docs}` is here (test `fixed_ids` in `ferrite-theme`); ids built
/// from data (`comment-<id>`, `reply-<id>`) are not.
pub const RESERVED_FRAGMENT_IDS: [&str; 30] = [
    // Landmarks and core markup (`content` is also the skip links' target).
    "content",
    "comments",
    "respond",
    "search",
    "menu",
    "top",
    "main",
    "toc",
    "nav",
    "footer",
    // The comment thread (`comment-pending` is the held-comment redirect's fragment).
    "comment-pending",
    "comments-title",
    "comment-form",
    "comment-form-author",
    "comment-form-email",
    "comment-form-url",
    "comment-form-trap",
    "comment-form-comment",
    // Section titles and search forms.
    "related-title",
    "news-title",
    "contact-form-title",
    "search-main",
    "search-error",
    "docs-search-page",
    "docs-search-error",
    // The contact form's fields.
    "cf-name",
    "cf-email",
    "cf-subject",
    "cf-message",
    "cf-website",
];

/// Whether `id` may stay on an `h1`–`h6` in sanitised plugin output: an id core's block
/// renderer could have given a heading (M4-3b P54). That is a non-empty string of
/// lowercase (Unicode) alphanumerics, `-` and `_` (the heading-id rule's output
/// alphabet) that is not one of [`RESERVED_FRAGMENT_IDS`]. Such an id needs no escaping
/// in an attribute and cannot take a theme's landmark.
pub fn heading_id_allowed(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        && id.to_lowercase() == id
        && !RESERVED_FRAGMENT_IDS.contains(&id)
}

/// The YouTube player URL prefix of a content `embed` node's `<iframe>` (M3 P11); the
/// 11-character video id follows.
pub const YOUTUBE_PLAYER: &str = "https://www.youtube-nocookie.com/embed/";
/// The Vimeo player URL prefix of an `embed` node's `<iframe>`; the numeric id follows.
pub const VIMEO_PLAYER: &str = "https://player.vimeo.com/video/";
/// The `sandbox` of an `embed` node's `<iframe>`.
pub const EMBED_SANDBOX: &str = "allow-scripts allow-same-origin allow-presentation allow-popups";
/// The `referrerpolicy` of an `embed` node's `<iframe>`.
pub const EMBED_REFERRER_POLICY: &str = "strict-origin-when-cross-origin";
/// The `class` of the `<figure>` around an `embed` node's `<iframe>`, per provider.
pub const EMBED_FIGURE_CLASSES: [&str; 2] = ["embed embed-youtube", "embed embed-vimeo"];

/// Whether `src` is exactly a player URL that core's renderer emits for an `embed`
/// node: [`YOUTUBE_PLAYER`] and an 11-character `[A-Za-z0-9_-]` id, or [`VIMEO_PLAYER`]
/// and 1–20 digits, with nothing after the id.
pub fn embed_player_src_allowed(src: &str) -> bool {
    if let Some(id) = src.strip_prefix(YOUTUBE_PLAYER) {
        return id.len() == 11
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
    }
    src.strip_prefix(VIMEO_PLAYER)
        .is_some_and(|id| (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_ids() {
        for ok in [
            "install-the-binary",
            "zażółć-gęślą",
            "привет-мир",
            "日本語",
            "section",
            "content-1",
            "a--b",
            "snake_case",
            "-",
            "v12-beta",
        ] {
            assert!(heading_id_allowed(ok), "{ok}");
        }
        for bad in [
            "",
            "Upper",
            "ZAŻÓŁĆ",
            "a b",
            "a.b",
            "a:b",
            "javascript:alert(1)",
            "a\"b",
            "a\tb",
            "a\u{0}b",
            "🎉",
        ] {
            assert!(!heading_id_allowed(bad), "{bad:?}");
        }
        for reserved in RESERVED_FRAGMENT_IDS {
            assert!(!heading_id_allowed(reserved), "{reserved}");
        }
    }

    #[test]
    fn reserved_ids_are_unique_and_in_the_alphabet() {
        let mut seen = std::collections::HashSet::new();
        for id in RESERVED_FRAGMENT_IDS {
            assert!(seen.insert(id), "{id} twice");
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{id}"
            );
        }
    }

    #[test]
    fn embed_player_srcs() {
        for ok in [
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            "https://www.youtube-nocookie.com/embed/a_b-C1d2E3f",
            "https://player.vimeo.com/video/76979871",
        ] {
            assert!(embed_player_src_allowed(ok), "{ok}");
        }
        for bad in [
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXc",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ?autoplay=1",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXc/",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "http://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            "https://player.vimeo.com/video/",
            "https://player.vimeo.com/video/1x",
            "https://player.vimeo.com/video/1?h=a",
            "https://evil.example/",
            "",
        ] {
            assert!(!embed_player_src_allowed(bad), "{bad}");
        }
    }

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
            " //evil.example/x",
            "  //evil.example/x",
            "\u{1}//evil.example/x",
            " /x",
            " ",
        ] {
            assert!(!relative_url_allowed(bad), "{bad:?} allowed");
        }
        assert_eq!(SAFE_URL_SCHEMES, ["http", "https", "mailto"]);
    }
}
