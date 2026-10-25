use super::text::collect_text;
use super::url::{ImageSrc, image_src, safe_href};
use super::{MAX_DEPTH, RenderCtx};
use ferrite_domain::html::{SAFE_URL_SCHEMES, relative_url_allowed};
use html_escape::{encode_double_quoted_attribute, encode_text};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, OnceLock};

/// The sanitiser for `html` nodes: ammonia's defaults with the shared URL policy
/// (M2a-2b P21, [`ferrite_domain::html`]): only `http`, `https` and `mailto` absolute
/// URLs, and relative URLs that [`relative_url_allowed`] accepts.
///
/// [`relative_url_allowed`]: ferrite_domain::html::relative_url_allowed
static HTML_SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut b = ammonia::Builder::default();
    b.url_schemes(SAFE_URL_SCHEMES.into_iter().collect())
        .url_relative(ammonia::UrlRelative::Custom(Box::new(keep_relative)));
    b
});

/// A relative URL as the shared policy allows it, else `None` (the attribute is dropped).
fn keep_relative(url: &str) -> Option<Cow<'_, str>> {
    relative_url_allowed(url).then_some(Cow::Borrowed(url))
}

/// Renders a TipTap/ProseMirror JSON document to HTML. All text and attribute values
/// are escaped; unsafe URLs are dropped. Nodes deeper than `MAX_DEPTH` render their
/// escaped text only.
pub fn render_html(doc: &Value, ctx: &RenderCtx) -> String {
    let mut out = String::new();
    render_node(doc, 0, ctx, &mut out);
    out
}

fn render_node(node: &Value, depth: usize, ctx: &RenderCtx, out: &mut String) {
    if depth > MAX_DEPTH {
        push_text_only(node, out);
        return;
    }
    match node {
        Value::Array(items) => {
            for item in items {
                render_node(item, depth + 1, ctx, out);
            }
        }
        Value::Object(map) => render_object(map, node, depth, ctx, out),
        _ => {}
    }
}

fn push_text_only(node: &Value, out: &mut String) {
    let mut raw = String::new();
    collect_text(node, &mut raw);
    out.push_str(&encode_text(&raw));
}

fn render_object(
    map: &Map<String, Value>,
    node: &Value,
    depth: usize,
    ctx: &RenderCtx,
    out: &mut String,
) {
    let ty = map.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "doc" => children(map, depth, ctx, out),
        "paragraph" => wrap("p", "", map, depth, ctx, out),
        "heading" => match map
            .get("attrs")
            .and_then(|a| a.get("level"))
            .and_then(Value::as_u64)
        {
            Some(l @ 1..=6) => wrap(&format!("h{l}"), "", map, depth, ctx, out),
            _ => wrap("p", "", map, depth, ctx, out),
        },
        "bulletList" => wrap("ul", "", map, depth, ctx, out),
        "orderedList" => {
            let attrs = match attr_u64(map, "start") {
                Some(n) if n >= 1 => format!(" start=\"{n}\""),
                _ => String::new(),
            };
            wrap("ol", &attrs, map, depth, ctx, out);
        }
        "listItem" => wrap("li", "", map, depth, ctx, out),
        "blockquote" => wrap("blockquote", "", map, depth, ctx, out),
        "codeBlock" => {
            out.push_str("<pre><code");
            if let Some(lang) = attr_str(map, "language").filter(|l| valid_language(l)) {
                out.push_str(&format!(" class=\"language-{lang}\""));
            }
            out.push('>');
            push_text_only(node, out);
            out.push_str("</code></pre>");
        }
        "horizontalRule" => out.push_str("<hr>"),
        "hardBreak" => out.push_str("<br>"),
        "image" => render_image(map, ctx, out),
        "table" => wrap("table", "", map, depth, ctx, out),
        "tableRow" => wrap("tr", "", map, depth, ctx, out),
        "tableHeader" => wrap("th", &span_attrs(map), map, depth, ctx, out),
        "tableCell" => wrap("td", &span_attrs(map), map, depth, ctx, out),
        "html" => {
            if let Some(h) = attr_str(map, "html") {
                out.push_str(&HTML_SANITIZER.clean(h).to_string());
            }
        }
        "text" => render_text(map, out),
        other => {
            log_unknown(other);
            push_text_only(node, out);
        }
    }
}

fn children(map: &Map<String, Value>, depth: usize, ctx: &RenderCtx, out: &mut String) {
    if let Some(Value::Array(items)) = map.get("content") {
        for item in items {
            render_node(item, depth + 1, ctx, out);
        }
    }
}

fn wrap(
    tag: &str,
    attrs: &str,
    map: &Map<String, Value>,
    depth: usize,
    ctx: &RenderCtx,
    out: &mut String,
) {
    out.push('<');
    out.push_str(tag);
    out.push_str(attrs);
    out.push('>');
    children(map, depth, ctx, out);
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

fn attr_str<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get("attrs")?.get(key)?.as_str()
}

fn attr_u64(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get("attrs")?.get(key)?.as_u64()
}

fn valid_language(l: &str) -> bool {
    (1..=32).contains(&l.len())
        && l.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'+' | b'#' | b'-')
        })
}

fn span_attrs(map: &Map<String, Value>) -> String {
    let mut s = String::new();
    for key in ["colspan", "rowspan"] {
        if let Some(n @ 1..=100) = attr_u64(map, key) {
            s.push_str(&format!(" {key}=\"{n}\""));
        }
    }
    s
}

fn render_image(map: &Map<String, Value>, ctx: &RenderCtx, out: &mut String) {
    let Some(src) = attr_str(map, "src").and_then(image_src) else {
        return;
    };
    let url = match src {
        ImageSrc::Url(u) => u,
        ImageSrc::Media(id) => match (ctx.media_url)(&id) {
            Some(u) => u,
            None => return,
        },
    };
    out.push_str(&format!(
        "<img src=\"{}\" alt=\"{}\"",
        encode_double_quoted_attribute(&url),
        encode_double_quoted_attribute(attr_str(map, "alt").unwrap_or(""))
    ));
    if let Some(title) = attr_str(map, "title") {
        out.push_str(&format!(
            " title=\"{}\"",
            encode_double_quoted_attribute(title)
        ));
    }
    out.push('>');
}

fn render_text(map: &Map<String, Value>, out: &mut String) {
    let Some(text) = map.get("text").and_then(Value::as_str) else {
        return;
    };
    let mut closers: Vec<&'static str> = Vec::new();
    let mut linked = false;
    if let Some(Value::Array(marks)) = map.get("marks") {
        for mark in marks {
            let Some(m) = mark.as_object() else { continue };
            match m.get("type").and_then(Value::as_str) {
                Some("bold") => open(out, &mut closers, "<strong>", "</strong>"),
                Some("italic") => open(out, &mut closers, "<em>", "</em>"),
                Some("strike") => open(out, &mut closers, "<s>", "</s>"),
                Some("code") => open(out, &mut closers, "<code>", "</code>"),
                Some("link") if !linked => {
                    let Some(href) = attr_str(m, "href").and_then(safe_href) else {
                        continue;
                    };
                    out.push_str(&format!(
                        "<a href=\"{}\"",
                        encode_double_quoted_attribute(&href)
                    ));
                    if attr_str(m, "target") == Some("_blank") {
                        out.push_str(" target=\"_blank\" rel=\"noopener noreferrer\"");
                    }
                    out.push('>');
                    closers.push("</a>");
                    linked = true;
                }
                _ => {}
            }
        }
    }
    out.push_str(&encode_text(text));
    while let Some(c) = closers.pop() {
        out.push_str(c);
    }
}

fn open(out: &mut String, closers: &mut Vec<&'static str>, o: &str, c: &'static str) {
    out.push_str(o);
    closers.push(c);
}

/// Distinct unknown types remembered for once-only logging; bounded so hostile input
/// cannot grow it without limit.
const MAX_LOGGED_TYPES: usize = 256;

fn log_unknown(ty: &str) {
    let ty: String = ty.chars().take(64).collect();
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let Ok(mut seen) = SEEN.get_or_init(Default::default).lock() else {
        return;
    };
    if seen.len() < MAX_LOGGED_TYPES && seen.insert(ty.clone()) {
        tracing::debug!(node_type = %ty, "unknown block node type; rendering its text content");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "0190f1c0-0000-7000-8000-000000000001";

    fn r(doc: Value) -> String {
        render_ref(&doc)
    }

    fn render_ref(doc: &Value) -> String {
        let media = |id: &str| Some(format!("/m/{id}.jpg"));
        render_html(doc, &RenderCtx { media_url: &media })
    }

    fn t(s: &str) -> Value {
        json!({"type":"text","text":s})
    }

    fn doc(children: Vec<Value>) -> Value {
        json!({"type":"doc","content":children})
    }

    fn marked(s: &str, marks: Value) -> Value {
        json!({"type":"text","text":s,"marks":marks})
    }

    #[test]
    fn paragraph_and_heading() {
        let d = doc(vec![
            json!({"type":"heading","attrs":{"level":2},"content":[t("H")]}),
            json!({"type":"paragraph","content":[t("a < b & c")]}),
        ]);
        assert_eq!(r(d), "<h2>H</h2><p>a &lt; b &amp; c</p>");
    }

    #[test]
    fn invalid_heading_levels_become_paragraphs() {
        for lvl in [
            json!(0),
            json!(7),
            json!("2"),
            json!(2.5),
            json!(-1),
            json!(null),
        ] {
            let d = doc(vec![
                json!({"type":"heading","attrs":{"level":lvl},"content":[t("H")]}),
            ]);
            assert_eq!(r(d), "<p>H</p>");
        }
        assert_eq!(
            r(doc(vec![json!({"type":"heading","content":[t("H")]})])),
            "<p>H</p>"
        );
    }

    #[test]
    fn lists() {
        let li = json!({"type":"listItem","content":[{"type":"paragraph","content":[t("x")]}]});
        assert_eq!(
            r(doc(vec![
                json!({"type":"bulletList","content":[li.clone()]})
            ])),
            "<ul><li><p>x</p></li></ul>"
        );
        assert_eq!(
            r(doc(vec![
                json!({"type":"orderedList","attrs":{"start":3},"content":[li.clone()]})
            ])),
            "<ol start=\"3\"><li><p>x</p></li></ol>"
        );
        for start in [json!(0), json!(-2), json!(1.5), json!("3"), json!(null)] {
            let d = doc(vec![
                json!({"type":"orderedList","attrs":{"start":start},"content":[li.clone()]}),
            ]);
            assert_eq!(r(d), "<ol><li><p>x</p></li></ol>");
        }
    }

    #[test]
    fn blockquote_hr_br() {
        let d = doc(vec![
            json!({"type":"blockquote","content":[{"type":"paragraph","content":[t("q"),{"type":"hardBreak"},t("r")]}]}),
            json!({"type":"horizontalRule"}),
        ]);
        assert_eq!(r(d), "<blockquote><p>q<br>r</p></blockquote><hr>");
    }

    #[test]
    fn code_block_language() {
        let mk = |lang: Value| {
            doc(vec![
                json!({"type":"codeBlock","attrs":{"language":lang},"content":[t("<b>x</b>")]}),
            ])
        };
        assert_eq!(
            r(mk(json!("c++"))),
            "<pre><code class=\"language-c++\">&lt;b&gt;x&lt;/b&gt;</code></pre>"
        );
        for bad in [
            json!("\"><script>"),
            json!("Rust"),
            json!(""),
            json!("a".repeat(33)),
            json!(5),
        ] {
            assert_eq!(r(mk(bad)), "<pre><code>&lt;b&gt;x&lt;/b&gt;</code></pre>");
        }
        assert!(r(mk(json!("a".repeat(32)))).contains("language-aaaa"));
    }

    #[test]
    fn marks() {
        let p = |c: Value| r(doc(vec![json!({"type":"paragraph","content":[c]})]));
        assert_eq!(
            p(marked("x", json!([{"type":"bold"}]))),
            "<p><strong>x</strong></p>"
        );
        assert_eq!(
            p(marked("x", json!([{"type":"italic"}]))),
            "<p><em>x</em></p>"
        );
        assert_eq!(
            p(marked("x", json!([{"type":"strike"}]))),
            "<p><s>x</s></p>"
        );
        assert_eq!(
            p(marked("x", json!([{"type":"code"}]))),
            "<p><code>x</code></p>"
        );
        assert_eq!(
            p(marked(
                "x",
                json!([{"type":"bold"},{"type":"italic"},{"type":"nope"}])
            )),
            "<p><strong><em>x</em></strong></p>"
        );
    }

    #[test]
    fn links() {
        let p = |attrs: Value| {
            r(doc(vec![
                json!({"type":"paragraph","content":[marked("x", json!([{"type":"link","attrs":attrs}]))]}),
            ]))
        };
        assert_eq!(
            p(json!({"href":"https://a.b/?q=1&r=\"2\""})),
            "<p><a href=\"https://a.b/?q=1&amp;r=&quot;2&quot;\">x</a></p>"
        );
        assert_eq!(
            p(json!({"href":"/x","target":"_blank"})),
            "<p><a href=\"/x\" target=\"_blank\" rel=\"noopener noreferrer\">x</a></p>"
        );
        assert_eq!(
            p(json!({"href":"/x","target":"_top\" onclick=\"x"})),
            "<p><a href=\"/x\">x</a></p>"
        );
        for bad in [
            "javascript:alert(1)",
            "JAVASCRIPT:alert(1)",
            "java\nscript:alert(1)",
            "  javascript:x",
            "data:text/html,x",
            "//evil.com",
            "vbscript:x",
        ] {
            assert_eq!(p(json!({"href":bad})), "<p>x</p>", "{bad:?}");
        }
        assert_eq!(p(json!({})), "<p>x</p>");
    }

    #[test]
    fn only_first_valid_link_is_emitted() {
        let d = doc(vec![
            json!({"type":"paragraph","content":[marked("x", json!([
            {"type":"link","attrs":{"href":"javascript:x"}},
            {"type":"link","attrs":{"href":"/a"}},
            {"type":"link","attrs":{"href":"/b"}}]))]}),
        ]);
        assert_eq!(r(d), "<p><a href=\"/a\">x</a></p>");
    }

    #[test]
    fn xss_boundary_regressions() {
        let p = |attrs: Value| {
            r(doc(vec![
                json!({"type":"paragraph","content":[marked("x", json!([{"type":"link","attrs":attrs}]))]}),
            ]))
        };
        for bad in [
            "&#106;avascript:alert(1)",
            "&#x6A;avascript:alert(1)",
            "java\u{a0}script:alert(1)",
            "java\u{2028}script:alert(1)",
            "\u{a0}javascript:alert(1)",
        ] {
            assert_eq!(p(json!({"href":bad})), "<p>x</p>", "{bad:?}");
        }
        assert_eq!(
            p(json!({"href":"/x\" onmouseover=\"alert(1)","title":"\" onclick=\"x"})),
            "<p><a href=\"/x&quot; onmouseover=&quot;alert(1)\">x</a></p>"
        );
        let img = r(doc(vec![json!({"type":"image","attrs":{
            "src":"/i.png\" onerror=\"alert(1)","alt":"\" onerror=\"x","title":"'\"><script>"}})]));
        assert!(
            !img.contains("\" onerror") && !img.contains("<script"),
            "{img}"
        );
        assert_eq!(img.matches('<').count(), 1, "{img}");
        assert!(img.ends_with('>'));
    }

    #[test]
    fn long_unknown_type_is_handled() {
        let d = doc(vec![json!({"type":"x".repeat(100_000),"content":[t("a")]})]);
        assert_eq!(r(d), "a");
    }

    #[test]
    fn images() {
        let img = |attrs: Value| r(doc(vec![json!({"type":"image","attrs":attrs})]));
        assert_eq!(
            img(json!({"src":"https://a.b/i.png","alt":"a\"b","title":"t<"})),
            "<img src=\"https://a.b/i.png\" alt=\"a&quot;b\" title=\"t&lt;\">"
        );
        assert_eq!(
            img(json!({"src":"/i.png"})),
            "<img src=\"/i.png\" alt=\"\">"
        );
        assert_eq!(
            img(json!({"src":format!("ferrite:media/{ID}")})),
            format!("<img src=\"/m/{ID}.jpg\" alt=\"\">")
        );
        for bad in [
            "data:image/png;base64,AAAA",
            "javascript:x",
            "//evil/x.png",
            "ferrite:media/nope",
            "x.png",
            "",
        ] {
            assert_eq!(img(json!({"src":bad})), "", "{bad:?}");
        }
        assert_eq!(img(json!({})), "");
        let none = |_: &str| None;
        let d = doc(vec![
            json!({"type":"image","attrs":{"src":format!("ferrite:media/{ID}")}}),
        ]);
        assert_eq!(render_html(&d, &RenderCtx { media_url: &none }), "");
    }

    #[test]
    fn tables() {
        let d = doc(vec![json!({"type":"table","content":[
            {"type":"tableRow","content":[
                {"type":"tableHeader","attrs":{"colspan":2,"rowspan":1},"content":[{"type":"paragraph","content":[t("h")]}]}]},
            {"type":"tableRow","content":[
                {"type":"tableCell","attrs":{"colspan":0,"rowspan":101},"content":[{"type":"paragraph","content":[t("c")]}]},
                {"type":"tableCell","attrs":{"colspan":"2"},"content":[]}]}]})]);
        assert_eq!(
            r(d),
            "<table><tr><th colspan=\"2\" rowspan=\"1\"><p>h</p></th></tr><tr><td><p>c</p></td><td></td></tr></table>"
        );
        let d = doc(vec![
            json!({"type":"table","content":[{"type":"tableRow","content":[
            {"type":"tableCell","attrs":{"colspan":100,"rowspan":100}}]}]}),
        ]);
        assert_eq!(
            r(d),
            "<table><tr><td colspan=\"100\" rowspan=\"100\"></td></tr></table>"
        );
    }

    #[test]
    fn html_node_is_sanitised() {
        let d = doc(vec![
            json!({"type":"html","attrs":{"html":"<p onclick=\"x()\">ok<script>alert(1)</script></p><a href=\"javascript:x\">l</a>"}}),
        ]);
        let out = r(d);
        assert!(out.contains("<p>ok</p>"), "{out}");
        assert!(
            !out.contains("script") && !out.contains("onclick") && !out.contains("javascript"),
            "{out}"
        );
    }

    /// M2a-2b P21: `html` nodes follow the shared URL policy (`ferrite_domain::html`).
    #[test]
    fn html_link_schemes() {
        let link = |href: &str| {
            let d = doc(vec![json!({
                "type": "html",
                "attrs": {"html": format!("<a href=\"{href}\">l</a><img src=\"{href}\">")}
            })]);
            r(d)
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
            let out = link(bad);
            assert!(
                !out.contains("href=") && !out.contains("src="),
                "{bad:?} kept: {out}"
            );
        }
        for ok in [
            "https://ok.example/x",
            "mailto:a@b.example",
            "/x",
            "#a",
            "x/y",
        ] {
            let out = link(ok);
            assert!(
                out.contains(&format!("href=\"{ok}\"")),
                "{ok:?} dropped: {out}"
            );
        }
    }

    #[test]
    fn unknown_node_renders_text() {
        let d = doc(vec![
            json!({"type":"mystery","content":[t("a<b"),{"type":"paragraph","content":[t("c")]}]}),
        ]);
        assert_eq!(r(d), "a&lt;bc");
    }

    #[test]
    fn script_in_text_is_escaped() {
        let d = doc(vec![
            json!({"type":"paragraph","content":[t("<script>alert(1)</script>")]}),
        ]);
        assert_eq!(r(d), "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>");
    }

    #[test]
    fn garbage_json_does_not_panic() {
        for v in [
            json!(null),
            json!(1),
            json!("s"),
            json!([]),
            json!({"type":5}),
            json!({"type":"doc","content":"x"}),
            json!({"type":"text"}),
            json!({"type":"text","text":5,"marks":"x"}),
            json!({"type":"paragraph","content":[null,1,[[]]]}),
            json!({"type":"image","attrs":"x"}),
            json!({"type":"text","text":"a","marks":[null,{"type":5},{"type":"link","attrs":5}]}),
        ] {
            let _ = r(v);
        }
    }

    /// Built by hand: `json!` serializes an embedded `Value` recursively.
    fn node(ty: &str, child: Value) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), Value::String(ty.into()));
        m.insert("content".into(), Value::Array(vec![child]));
        Value::Object(m)
    }

    fn nested(levels: usize) -> Value {
        let mut v = t("deep");
        for _ in 0..levels {
            v = node("blockquote", v);
        }
        v
    }

    /// Drops a deeply nested value without recursion (Value's own Drop recurses).
    fn dismantle(mut v: Value) {
        while let Some(next) = v
            .get_mut("content")
            .and_then(Value::as_array_mut)
            .and_then(Vec::pop)
        {
            v = next;
        }
    }

    fn on_small_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn ten_thousand_levels_render_without_overflow() {
        on_small_stack(|| {
            let d = nested(10_000);
            let out = render_ref(&d);
            assert_eq!(out.matches("<blockquote>").count(), MAX_DEPTH + 1);
            assert_eq!(out.matches("</blockquote>").count(), MAX_DEPTH + 1);
            assert!(out.contains("deep"));
            let text = super::super::extract_text(&d);
            assert_eq!(text, "deep");
            assert_eq!(super::super::excerpt(&d, 2), "de…");
            dismantle(d);
        });
    }

    #[test]
    fn deep_nesting_below_cap_escapes_text() {
        on_small_stack(|| {
            let mut v = t("<script>x</script>");
            for _ in 0..1000 {
                v = node("mystery", v);
            }
            for _ in 0..100 {
                v = node("bulletList", node("listItem", v));
            }
            let out = r(v);
            assert!(!out.contains("<script"));
            assert!(out.contains("&lt;script&gt;x&lt;/script&gt;"));
        });
    }

    #[test]
    fn deep_arrays_do_not_overflow() {
        on_small_stack(|| {
            let mut v = t("x");
            for _ in 0..10_000 {
                v = Value::Array(vec![v]);
            }
            assert_eq!(render_ref(&v), "x");
            std::mem::forget(v); // Value's Drop recurses
        });
    }
}
