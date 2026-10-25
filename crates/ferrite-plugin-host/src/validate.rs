//! Output validation for filter results (design 05 §7): the variant, the size caps, and
//! `ammonia` sanitising of `html` results (M0 ruling R4 closed in M1c-2). Head rendering
//! (escaping, `script-tag` checks) is the HTTP layer's job: the head stays structured
//! here. Route responses get their own checks ([`validate_route_response`], 05 §4b):
//! what headers the visitor finally gets is the HTTP layer's choice.

use std::sync::LazyLock;

use ferrite_domain::html::{SAFE_URL_SCHEMES, relative_url_allowed};

use crate::capability::FilterType;
use crate::manifest::HtmlMode;
use crate::types::{FilterValue, Head, RouteResponse};

/// Most bytes of a route request body, and of a route response body (05 §4b).
pub const MAX_ROUTE_BODY_BYTES: usize = 1024 * 1024;
/// Most headers a route response may carry.
pub const MAX_ROUTE_HEADERS: usize = 32;
/// Most bytes of one route response header name, and of one value.
pub const MAX_ROUTE_HEADER_BYTES: usize = 4 * 1024;

/// Policy for `html` results of plugins without `render.html = "raw"` (ruling R22):
/// ammonia's defaults (no `<script>`/`<style>`/`<iframe>`/`<form>`/`<svg>`/`<math>`, no
/// `on*` handlers) with the shared URL policy of content blocks (M2a-2b P21,
/// [`ferrite_domain::html`]): absolute URLs only `http`, `https` or `mailto`; relative
/// ones without `//`, a backslash or a control character;
/// `rel="noopener noreferrer"` on every link, plus two additions: `a[target]`, and
/// `code[class]` restricted to `language-*` classes, so core's code-block classes
/// survive a sanitising plugin.
///
/// **Chaining.** Each non-raw plugin's whole output is sanitised after it runs, and that
/// output includes whatever it was given. So markup a `raw` plugin added earlier in the
/// chain survives only until the next non-raw plugin runs; raw markup is kept only when
/// no non-raw plugin runs after it.
static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut b = ammonia::Builder::default();
    b.url_schemes(SAFE_URL_SCHEMES.into_iter().collect())
        .url_relative(ammonia::UrlRelative::Custom(Box::new(keep_relative)))
        .link_rel(Some("noopener noreferrer"))
        .add_tag_attributes("a", ["target"])
        .add_tag_attributes("code", ["class"])
        .attribute_filter(|element, attribute, value| {
            if element == "code" && attribute == "class" {
                return language_classes(value);
            }
            Some(value.into())
        });
    b
});

/// A relative URL as the shared policy allows it, else `None` (the attribute is dropped).
fn keep_relative(url: &str) -> Option<std::borrow::Cow<'_, str>> {
    relative_url_allowed(url).then_some(std::borrow::Cow::Borrowed(url))
}

/// Only `language-<name>` tokens (core's `valid_language` charset), else none.
fn language_classes(value: &str) -> Option<std::borrow::Cow<'_, str>> {
    let kept: Vec<&str> = value
        .split_ascii_whitespace()
        .filter(|c| {
            c.strip_prefix("language-").is_some_and(|l| {
                (1..=32).contains(&l.len())
                    && l.bytes().all(|b| {
                        b.is_ascii_lowercase()
                            || b.is_ascii_digit()
                            || matches!(b, b'+' | b'#' | b'-')
                    })
            })
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join(" ").into())
}

/// Cap on the size of a returned `head`: all its strings plus [`HEAD_ELEMENT_COST`] per
/// meta/link/script, so many empty tags cannot slip under it.
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Bytes charged per head element on top of its strings (the host-side `Vec` entry and
/// the rendered tag's markup).
const HEAD_ELEMENT_COST: usize = 64;
/// Most elements of each kind a returned `head` may carry.
const MAX_HEAD_METAS: usize = 256;
const MAX_HEAD_LINKS: usize = 64;
const MAX_HEAD_SCRIPTS: usize = 16;
/// Floor and ceiling of the `html`/`text` cap, which is otherwise 4x the input length.
const MIN_BODY_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Whether `value` is the variant that filters of type `ty` work on.
pub(crate) fn matches_type(ty: FilterType, value: &FilterValue) -> bool {
    matches!(
        (ty, value),
        (FilterType::Head, FilterValue::Head(_))
            | (FilterType::Html, FilterValue::Html(_))
            | (FilterType::Text, FilterValue::Text(_))
    )
}

fn head_bytes(h: &Head) -> usize {
    let opt = |s: &Option<String>| s.as_deref().map_or(0, str::len);
    let metas: usize = h
        .metas
        .iter()
        .map(|m| opt(&m.name) + opt(&m.property) + m.content.len())
        .sum();
    let links: usize = h
        .links
        .iter()
        .map(|l| l.rel.len() + l.href.len() + opt(&l.hreflang) + opt(&l.kind))
        .sum();
    let scripts: usize = h
        .scripts
        .iter()
        .map(|s| opt(&s.src) + opt(&s.json_ld))
        .sum();
    let elements = h.metas.len() + h.links.len() + h.scripts.len();
    opt(&h.title) + metas + links + scripts + elements.saturating_mul(HEAD_ELEMENT_COST)
}

/// Reject a `head` with more elements of one kind than allowed.
fn check_head_counts(h: &Head) -> Result<(), String> {
    for (kind, count, max) in [
        ("metas", h.metas.len(), MAX_HEAD_METAS),
        ("links", h.links.len(), MAX_HEAD_LINKS),
        ("scripts", h.scripts.len(), MAX_HEAD_SCRIPTS),
    ] {
        if count > max {
            return Err(format!("head has {count} {kind}, limit {max}"));
        }
    }
    Ok(())
}

fn body_len(v: &FilterValue) -> usize {
    match v {
        FilterValue::Html(s) | FilterValue::Text(s) => s.len(),
        FilterValue::Head(h) => head_bytes(h),
    }
}

/// Accept `output` of a filter of type `ty` that was given `input`, or say why not.
/// `Html` output is sanitised unless `html` is [`HtmlMode::Raw`]; the size cap applies
/// before and after sanitising (escaping can grow text).
pub(crate) fn validate_output(
    ty: FilterType,
    input: &FilterValue,
    output: FilterValue,
    html: HtmlMode,
) -> Result<FilterValue, String> {
    if !matches_type(ty, &output) {
        return Err(format!("output variant does not match filter type {ty:?}"));
    }
    check_size(input, &output)?;
    match output {
        FilterValue::Html(s) if html != HtmlMode::Raw => {
            let clean = FilterValue::Html(SANITIZER.clean(&s).to_string());
            check_size(input, &clean)?;
            Ok(clean)
        }
        other => Ok(other),
    }
}

fn check_size(input: &FilterValue, output: &FilterValue) -> Result<(), String> {
    match output {
        FilterValue::Head(h) => {
            check_head_counts(h)?;
            let size = head_bytes(h);
            if size > MAX_HEAD_BYTES {
                return Err(format!("head is {size} bytes, limit {MAX_HEAD_BYTES}"));
            }
        }
        FilterValue::Html(s) | FilterValue::Text(s) => {
            let cap = body_len(input)
                .saturating_mul(4)
                .clamp(MIN_BODY_BYTES, MAX_BODY_BYTES);
            if s.len() > cap {
                return Err(format!("output is {} bytes, limit {cap}", s.len()));
            }
        }
    }
    Ok(())
}

/// Accept a route response, or say why not: status 200..=599, at most
/// [`MAX_ROUTE_HEADERS`] headers, each name a non-empty HTTP token and each value
/// visible ASCII, space or tab, both at most [`MAX_ROUTE_HEADER_BYTES`], and a body of
/// at most [`MAX_ROUTE_BODY_BYTES`]. The checks say nothing about which headers may
/// reach the visitor.
pub(crate) fn validate_route_response(res: &RouteResponse) -> Result<(), String> {
    if !(200..=599).contains(&res.status) {
        return Err(format!("status {} outside 200..=599", res.status));
    }
    if res.headers.len() > MAX_ROUTE_HEADERS {
        return Err(format!(
            "{} headers, limit {MAX_ROUTE_HEADERS}",
            res.headers.len()
        ));
    }
    for (i, (name, value)) in res.headers.iter().enumerate() {
        if name.len() > MAX_ROUTE_HEADER_BYTES || value.len() > MAX_ROUTE_HEADER_BYTES {
            return Err(format!("header {i} over {MAX_ROUTE_HEADER_BYTES} bytes"));
        }
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            return Err(format!("header {i}: invalid name"));
        }
        if !value
            .bytes()
            .all(|b| b == b'\t' || (b' '..=b'~').contains(&b))
        {
            return Err(format!("header {i}: invalid value"));
        }
    }
    if res.body.len() > MAX_ROUTE_BODY_BYTES {
        return Err(format!(
            "body is {} bytes, limit {MAX_ROUTE_BODY_BYTES}",
            res.body.len()
        ));
    }
    Ok(())
}

/// An RFC 9110 `tchar`.
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(status: u16, headers: &[(&str, &str)], body: usize) -> RouteResponse {
        RouteResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            body: vec![b'x'; body],
        }
    }

    #[test]
    fn route_responses_are_bounded() {
        let ok = |r: RouteResponse| validate_route_response(&r).is_ok();
        assert!(ok(route(200, &[("Content-Type", "text/plain")], 0)));
        assert!(ok(route(599, &[], MAX_ROUTE_BODY_BYTES)));
        assert!(ok(route(303, &[("Location", "/a b\tc")], 0)));
        for status in [0, 100, 101, 199, 600, u16::MAX] {
            assert!(!ok(route(status, &[], 0)), "{status}");
        }
        assert!(!ok(route(200, &[], MAX_ROUTE_BODY_BYTES + 1)));
        let many: Vec<(String, String)> = (0..=MAX_ROUTE_HEADERS)
            .map(|i| (format!("x-{i}"), "v".to_owned()))
            .collect();
        let refs: Vec<(&str, &str)> = many.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert!(!ok(route(200, &refs, 0)));
        assert!(ok(route(200, &refs[..MAX_ROUTE_HEADERS], 0)));
        let long = "v".repeat(MAX_ROUTE_HEADER_BYTES + 1);
        assert!(!ok(route(200, &[("x", &long)], 0)));
        assert!(!ok(route(200, &[(&long, "v")], 0)));
        assert!(ok(route(200, &[("x", &long[1..])], 0)));
        for bad_name in ["", "a b", "a:b", "a\nb", "é", "a\0"] {
            assert!(!ok(route(200, &[(bad_name, "v")], 0)), "{bad_name:?}");
        }
        for bad_value in ["a\r\nSet-Cookie: x=1", "a\nb", "\0", "é", "\u{7f}"] {
            assert!(!ok(route(200, &[("x", bad_value)], 0)), "{bad_value:?}");
        }
    }

    fn html(n: usize) -> FilterValue {
        FilterValue::Html("a".repeat(n))
    }

    #[test]
    fn head_over_64_kib_rejected() {
        let input = FilterValue::Head(Head::default());
        let big = FilterValue::Head(Head {
            title: Some("t".repeat(70 * 1024)),
            ..Head::default()
        });
        assert!(validate_output(FilterType::Head, &input, big, HtmlMode::Off).is_err());
        let ok = FilterValue::Head(Head {
            title: Some("t".repeat(64 * 1024)),
            ..Head::default()
        });
        assert!(validate_output(FilterType::Head, &input, ok, HtmlMode::Off).is_ok());
    }

    use crate::types::{LinkTag, MetaTag, ScriptTag};

    fn meta(content: &str) -> MetaTag {
        MetaTag {
            name: None,
            property: None,
            content: content.into(),
        }
    }

    fn check(h: Head) -> Result<FilterValue, String> {
        validate_output(
            FilterType::Head,
            &FilterValue::Head(Head::default()),
            FilterValue::Head(h),
            HtmlMode::Off,
        )
    }

    #[test]
    fn head_element_counts_are_capped() {
        let metas = |n| Head {
            metas: vec![meta(""); n],
            ..Head::default()
        };
        assert!(check(metas(256)).is_ok());
        assert!(check(metas(257)).is_err());
        assert!(check(metas(500_000)).is_err(), "empty tags are not free");

        let link = LinkTag {
            rel: String::new(),
            href: String::new(),
            hreflang: None,
            kind: None,
        };
        let links = |n| Head {
            links: vec![link.clone(); n],
            ..Head::default()
        };
        assert!(check(links(64)).is_ok());
        assert!(check(links(65)).is_err());

        let script = ScriptTag {
            src: None,
            json_ld: None,
        };
        let scripts = |n| Head {
            scripts: vec![script.clone(); n],
            ..Head::default()
        };
        assert!(check(scripts(16)).is_ok());
        assert!(check(scripts(17)).is_err());
    }

    #[test]
    fn each_head_element_costs_64_bytes() {
        // 256 metas: 256 * (64 + 192) = 64 KiB exactly.
        let full = Head {
            metas: vec![meta(&"c".repeat(192)); 256],
            ..Head::default()
        };
        assert!(check(full.clone()).is_ok());
        let mut over = full;
        over.metas[0].content.push('c');
        assert!(check(over).is_err());
        // A title alone may use the whole budget; one element on top does not fit.
        let title = Head {
            title: Some("t".repeat(64 * 1024 - 64)),
            scripts: vec![ScriptTag {
                src: None,
                json_ld: None,
            }],
            ..Head::default()
        };
        assert!(check(title.clone()).is_ok());
        let mut over = title;
        over.title = over.title.map(|t| t + "t");
        assert!(check(over).is_err());
    }

    #[test]
    fn html_over_four_times_input_rejected() {
        let input = html(100 * 1024);
        assert!(
            validate_output(
                FilterType::Html,
                &input,
                html(4 * 100 * 1024 + 1),
                HtmlMode::Raw
            )
            .is_err()
        );
        assert!(
            validate_output(
                FilterType::Html,
                &input,
                html(4 * 100 * 1024),
                HtmlMode::Raw
            )
            .is_ok()
        );
    }

    #[test]
    fn small_input_gets_64_kib_floor() {
        let input = html(1);
        assert!(validate_output(FilterType::Html, &input, html(60 * 1024), HtmlMode::Raw).is_ok());
        assert!(
            validate_output(FilterType::Html, &input, html(64 * 1024 + 1), HtmlMode::Raw).is_err()
        );
    }

    #[test]
    fn never_above_2_mib() {
        let input = html(1024 * 1024);
        assert!(
            validate_output(
                FilterType::Html,
                &input,
                html(2 * 1024 * 1024),
                HtmlMode::Raw
            )
            .is_ok()
        );
        assert!(
            validate_output(
                FilterType::Html,
                &input,
                html(2 * 1024 * 1024 + 1),
                HtmlMode::Raw
            )
            .is_err()
        );
    }

    fn sanitized(s: &str) -> Result<FilterValue, String> {
        validate_output(
            FilterType::Html,
            &html(1),
            FilterValue::Html(s.into()),
            HtmlMode::Sanitized,
        )
    }

    #[test]
    fn hostile_html_is_sanitised() {
        let out = sanitized(
            "<p onclick=\"x()\">a</p><script>alert(1)</script>\
             <a href=\"javascript:alert(1)\">j</a><a href=\"/ok\">k</a>\
             <pre><code class=\"language-rust\">fn</code></pre><style>p{}</style>",
        );
        assert_eq!(
            out,
            Ok(FilterValue::Html(
                "<p>a</p><a rel=\"noopener noreferrer\">j</a>\
                 <a href=\"/ok\" rel=\"noopener noreferrer\">k</a>\
                 <pre><code class=\"language-rust\">fn</code></pre>"
                    .into()
            ))
        );
    }

    #[test]
    fn active_content_is_removed() {
        let cases = [
            (
                r#"<a href="data:text/html,<script>x</script>">d</a>"#,
                r#"<a rel="noopener noreferrer">d</a>"#,
            ),
            (
                r#"<img src="data:image/svg+xml,x" alt="a">"#,
                r#"<img alt="a">"#,
            ),
            (r#"<iframe src="https://evil.example/"></iframe>i"#, "i"),
            (
                r#"<form action="/x"><input name="q"><button>go</button></form>"#,
                "go",
            ),
            ("<svg><script>x</script><circle/></svg>s", "s"),
            ("<math><mi>x</mi></math>m", "m"),
            (
                r#"<img src="/x.png" onerror="alert(1)">"#,
                r#"<img src="/x.png">"#,
            ),
            (
                r#"<a href="/a" target="_blank">t</a>"#,
                r#"<a href="/a" target="_blank" rel="noopener noreferrer">t</a>"#,
            ),
            (
                r#"<code class="language-rust evil">c</code>"#,
                r#"<code class="language-rust">c</code>"#,
            ),
            (r#"<code class="hl evil">c</code>"#, "<code>c</code>"),
            (r#"<p class="x">p</p>"#, "<p>p</p>"),
        ];
        for (input, want) in cases {
            assert_eq!(
                sanitized(input),
                Ok(FilterValue::Html(want.into())),
                "{input}"
            );
        }
    }

    /// M2a-2b P21: plugin `html` follows the same URL policy as content blocks.
    #[test]
    fn html_link_schemes() {
        let link = |href: &str| match sanitized(&format!("<a href=\"{href}\">l</a>")) {
            Ok(FilterValue::Html(h)) => h,
            other => panic!("{other:?}"),
        };
        for bad in [
            "//evil.example/x",
            "\\\\evil.example/x",
            "/\\evil.example/x",
            "ftp://files.example/x",
            "tel:+15550100",
            "javascript:alert(1)",
            "data:text/html,x",
        ] {
            assert_eq!(link(bad), "<a rel=\"noopener noreferrer\">l</a>", "{bad:?}");
        }
        for ok in [
            "https://ok.example/x",
            "mailto:a@b.example",
            "/x",
            "#a",
            "x/y",
        ] {
            assert_eq!(
                link(ok),
                format!("<a href=\"{ok}\" rel=\"noopener noreferrer\">l</a>"),
                "{ok:?}"
            );
        }
    }

    #[test]
    fn raw_html_passes_through() {
        let s = "<script>alert(1)</script><p onclick=\"x()\">a</p>";
        let out = validate_output(
            FilterType::Html,
            &html(1),
            FilterValue::Html(s.into()),
            HtmlMode::Raw,
        );
        assert_eq!(out, Ok(FilterValue::Html(s.into())));
    }

    #[test]
    fn sanitised_growth_is_capped() {
        // 60 KiB of `&` fits the 64 KiB floor; escaped to `&amp;` it is 300 KiB.
        assert!(sanitized(&"&".repeat(60 * 1024)).is_err());
    }

    #[test]
    fn text_is_not_sanitised() {
        let out = validate_output(
            FilterType::Text,
            &FilterValue::Text(String::new()),
            FilterValue::Text("<b>".into()),
            HtmlMode::Off,
        );
        assert_eq!(out, Ok(FilterValue::Text("<b>".into())));
    }

    #[test]
    fn wrong_variant_rejected() {
        let input = FilterValue::Head(Head::default());
        let out = FilterValue::Text("x".into());
        assert!(validate_output(FilterType::Head, &input, out, HtmlMode::Off).is_err());
        let out = FilterValue::Text("x".into());
        assert!(validate_output(FilterType::Html, &html(1), out, HtmlMode::Sanitized).is_err());
    }
}
