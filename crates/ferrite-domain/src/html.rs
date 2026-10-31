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
/// `themes/{blog,business,docs}` is here (test `fixed_ids` in `ferrite-theme`); the ids
/// built from a comment's id are reserved by pattern ([`is_reserved_fragment_id`]).
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

/// The fields of a theme's comment form: its ids are `<form id>-<field>`
/// (`comment-form-author`, `reply-<comment id>-email`, …).
pub const COMMENT_FORM_FIELDS: [&str; 5] = ["author", "email", "url", "trap", "comment"];

/// Whether `id` is reserved: one of [`RESERVED_FRAGMENT_IDS`], or an id a theme builds
/// from a comment's id: its permalink `comment-<uuid>`, its reply form `reply-<uuid>`,
/// or that form's fields `reply-<uuid>-<field>` ([`COMMENT_FORM_FIELDS`]). `<uuid>` is
/// any 36 characters of lowercase hex and `-` (a hyphenated UUID's shape). A heading must
/// not take any of them: the block renderer moves one off with a suffix
/// (`comment-<uuid>-1`), and the plugin sanitiser drops them from headings.
pub fn is_reserved_fragment_id(id: &str) -> bool {
    if RESERVED_FRAGMENT_IDS.contains(&id) {
        return true;
    }
    if let Some(after) = id.strip_prefix("comment-").and_then(uuid_then) {
        return after.is_empty();
    }
    if let Some(after) = id.strip_prefix("reply-").and_then(uuid_then) {
        return after.is_empty()
            || after
                .strip_prefix('-')
                .is_some_and(|f| COMMENT_FORM_FIELDS.contains(&f));
    }
    false
}

/// What follows a leading hyphenated-UUID-shaped 36 characters of `rest`, if it has them.
fn uuid_then(rest: &str) -> Option<&str> {
    let (uuid, after) = (rest.get(..36)?, &rest[36..]);
    uuid.bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f' | b'-'))
        .then_some(after)
}

/// Whether `id` may stay on an `h1`–`h6` in sanitised plugin output: an id core's block
/// renderer could have given a heading (M4-3b P54). That is a non-empty string of
/// lowercase (Unicode) alphanumerics, `-` and `_` (the heading-id rule's output
/// alphabet) that is not reserved ([`is_reserved_fragment_id`]). Such an id needs no
/// escaping in an attribute and cannot take a theme's landmark or a comment's anchor.
pub fn heading_id_allowed(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        && id.to_lowercase() == id
        && !is_reserved_fragment_id(id)
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

    /// Final re-review: the ids themes build from a comment's id (its permalink
    /// `comment-<uuid>`, the reply form `reply-<uuid>` and its fields) are reserved too.
    #[test]
    fn comment_anchor_ids_are_reserved() {
        let u = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
        let mut reserved = vec![format!("comment-{u}"), format!("reply-{u}")];
        for field in COMMENT_FORM_FIELDS {
            reserved.push(format!("reply-{u}-{field}"));
        }
        for id in &reserved {
            assert!(is_reserved_fragment_id(id), "{id}");
            assert!(!heading_id_allowed(id), "{id}");
        }
        for id in [
            format!("comment-{u}-1"),
            format!("reply-{u}-1"),
            format!("reply-{u}-author-1"),
            format!("comment-{}", &u[..35]),
            format!("comment-{u}x"),
            "comment-1".to_owned(),
            "reply-to-all".to_owned(),
            "comments-1".to_owned(),
        ] {
            assert!(!is_reserved_fragment_id(&id), "{id}");
            assert!(heading_id_allowed(&id), "{id}");
        }
        for id in RESERVED_FRAGMENT_IDS {
            assert!(is_reserved_fragment_id(id), "{id}");
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
