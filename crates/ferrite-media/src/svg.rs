//! SVG sanitiser: an element and attribute allowlist over quick-xml's namespace-aware reader
//! (M2a-2b P7). Hostile structure (DOCTYPE, unknown entities, bombs) is refused; anything not on
//! the allowlist is dropped; the output is a canonical UTF-8 document that re-sanitises to
//! itself.

use quick_xml::Writer;
use quick_xml::XmlVersion;
use quick_xml::encoding::EncodingError;
use quick_xml::escape::{EscapeError, escape};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesDecl, BytesEnd, BytesRef, BytesStart, BytesText, Event};
use quick_xml::name::{NamespaceResolver, ResolveResult};
use quick_xml::reader::NsReader;

/// The MIME type sanitised SVGs are stored and served under.
pub const SVG_MIME: &str = "image/svg+xml";
/// Largest SVG input accepted (2 MiB).
pub const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;

const MAX_DEPTH: usize = 64;
const MAX_ELEMENTS: usize = 50_000;
const MAX_ATTRIBUTES: usize = 64;
const MAX_VALUE_BYTES: usize = 256 * 1024;
/// How far into the upload `looks_like_svg` looks for `<svg`.
const HEAD_BYTES: usize = 8 * 1024;
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

const SVG_NS: &[u8] = b"http://www.w3.org/2000/svg";
const XLINK_NS: &[u8] = b"http://www.w3.org/1999/xlink";
const XML_NS: &[u8] = b"http://www.w3.org/XML/1998/namespace";

/// Elements kept (SVG namespace only, matched by local name, case-sensitive).
const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "title",
    "desc",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
    "marker",
    "filter",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feFlood",
    "feGaussianBlur",
    "feMerge",
    "feMergeNode",
    "feOffset",
    "feFuncR",
    "feFuncG",
    "feFuncB",
    "feFuncA",
    "image",
];

/// Elements whose text content is kept; text anywhere else is dropped.
const TEXT_ELEMENTS: &[&str] = &["title", "desc", "text", "tspan", "textPath"];

/// Unprefixed attributes kept (besides `href`, `xlink:href` and `xml:space`).
const ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "width",
    "height",
    "viewBox",
    "preserveAspectRatio",
    "d",
    "points",
    "transform",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-opacity",
    "stroke-miterlimit",
    "opacity",
    "offset",
    "stop-color",
    "stop-opacity",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "fx",
    "fy",
    "patternUnits",
    "patternContentUnits",
    "patternTransform",
    "clipPathUnits",
    "clip-path",
    "clip-rule",
    "mask",
    "maskUnits",
    "maskContentUnits",
    "markerWidth",
    "markerHeight",
    "refX",
    "refY",
    "orient",
    "markerUnits",
    "marker-start",
    "marker-mid",
    "marker-end",
    "filter",
    "filterUnits",
    "primitiveUnits",
    "in",
    "in2",
    "result",
    "stdDeviation",
    "dx",
    "dy",
    "mode",
    "operator",
    "k1",
    "k2",
    "k3",
    "k4",
    "values",
    "type",
    "tableValues",
    "slope",
    "intercept",
    "amplitude",
    "exponent",
    "flood-color",
    "flood-opacity",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "word-spacing",
    "textLength",
    "lengthAdjust",
    "startOffset",
    "visibility",
    "display",
    "style",
    "version",
];

/// CSS properties kept inside a `style` attribute: the presentation properties of `ATTRIBUTES`.
const STYLE_PROPERTIES: &[&str] = &[
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-opacity",
    "stroke-miterlimit",
    "opacity",
    "stop-color",
    "stop-opacity",
    "clip-path",
    "clip-rule",
    "mask",
    "marker-start",
    "marker-mid",
    "marker-end",
    "filter",
    "flood-color",
    "flood-opacity",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "word-spacing",
    "visibility",
    "display",
    "transform",
];

/// Raster types allowed as a `data:` URI in `<image href>`.
const DATA_IMAGE_TYPES: &[&str] = &["png", "jpeg", "gif", "webp"];

/// Properties (attribute or `style` declaration) whose value must be `none` or one local
/// `url(#id)`; anything else there (an `<image>` such as `image-set(…)`) could load a resource.
const REFERENCE_PROPERTIES: &[&str] = &[
    "mask",
    "clip-path",
    "filter",
    "marker-start",
    "marker-mid",
    "marker-end",
];

/// Tokens refused in any attribute or `style` value, matched after removing whitespace and
/// control characters and lowercasing, so `-webkit-image-set(` and `java\tscript:` match too.
const ACTIVE_TOKENS: &[&str] = &[
    "javascript:",
    "vbscript:",
    "expression(",
    "@import",
    "src(",
    "image-set(",
    "image(",
    "cross-fade(",
    "element(",
];

/// Most `<use>` elements kept in one document.
const MAX_USES: usize = 128;

/// Public identifiers of the standard SVG DTDs. A DOCTYPE naming one of them, without an
/// internal subset, is accepted and dropped; nothing is ever fetched.
const SVG_PUBLIC_IDS: &[&str] = &[
    "-//W3C//DTD SVG 1.0//EN",
    "-//W3C//DTD SVG 20010904//EN",
    "-//W3C//DTD SVG 1.1//EN",
    "-//W3C//DTD SVG 1.1 Basic//EN",
    "-//W3C//DTD SVG 1.1 Tiny//EN",
];

/// Longest entity name echoed in `UnknownEntity`.
const MAX_ENTITY_NAME: usize = 64;

/// Why an SVG was refused.
#[derive(Debug, thiserror::Error)]
pub enum SvgError {
    #[error("the SVG is larger than {MAX_SVG_BYTES} bytes")]
    TooLarge,
    #[error("the SVG is not UTF-8")]
    NotUtf8,
    /// A fixed description plus a byte offset; never input text.
    #[error("the SVG is not well-formed XML: {0}")]
    NotXml(String),
    #[error("the document is not an SVG (root must be <svg> in the SVG namespace)")]
    NotSvg,
    #[error(
        "the SVG has a DOCTYPE other than a standard SVG 1.0/1.1 one without an internal subset"
    )]
    Doctype,
    /// The entity name, at most 64 characters.
    #[error("the SVG references an unknown entity `&{0};`")]
    UnknownEntity(String),
    #[error("the SVG nests elements deeper than {MAX_DEPTH}")]
    TooDeep,
    #[error("the SVG has more than {MAX_ELEMENTS} elements")]
    TooManyElements,
    #[error("an SVG element has more than {MAX_ATTRIBUTES} attributes")]
    TooManyAttributes,
    #[error("an SVG attribute value is longer than {MAX_VALUE_BYTES} bytes")]
    ValueTooLong,
    #[error(
        "the SVG has more than {MAX_USES} <use> elements, or a <use> that references a <use> or \
         an element containing one"
    )]
    TooManyUses,
}

/// True when the upload head looks like an SVG document: after an optional UTF-8 BOM and
/// whitespace it starts with `<?xml`, `<!--` or `<svg`, and `<svg` occurs within the first 8 KiB.
pub fn looks_like_svg(head: &[u8]) -> bool {
    let head = &head[..head.len().min(HEAD_BYTES)];
    let body = head.strip_prefix(UTF8_BOM).unwrap_or(head);
    let start = body
        .iter()
        .position(|b| !is_xml_space(char::from(*b)))
        .unwrap_or(body.len());
    let rest = &body[start..];
    let opens =
        rest.starts_with(b"<?xml") || rest.starts_with(b"<!--") || rest.starts_with(b"<svg");
    opens && head.windows(4).any(|w| w == b"<svg")
}

/// Sanitises an SVG: refuses DOCTYPEs (other than a standard SVG one), unknown entities, non-SVG
/// roots, `<use>` fan-out and oversized or overly deep documents; drops every element, attribute
/// and value outside the allowlist; writes a canonical UTF-8 document (unprefixed names, fixed
/// namespace declarations, escaped values).
pub fn sanitize_svg(input: &[u8]) -> Result<Vec<u8>, SvgError> {
    if input.len() > MAX_SVG_BYTES {
        return Err(SvgError::TooLarge);
    }
    let original_len = input.len();
    let input = input.strip_prefix(UTF8_BOM).unwrap_or(input);
    // Byte offsets in errors count from the start of the upload, BOM included.
    let base = (original_len - input.len()) as u64;
    // NUL never occurs in XML; in practice it means UTF-16 without a BOM.
    if input.contains(&0) {
        return Err(SvgError::NotUtf8);
    }
    let text = std::str::from_utf8(input).map_err(|_| SvgError::NotUtf8)?;
    if let Some((at, _)) = text.char_indices().find(|(_, c)| !is_xml_char(*c)) {
        return Err(not_xml("character not allowed in XML", base + at as u64));
    }

    let mut reader = NsReader::from_str(text);
    let mut doc = Sanitizer::default();
    loop {
        let at = base + reader.buffer_position();
        let (ns, event) = match reader.read_resolved_event() {
            Ok((ns, event)) => (is_svg_ns(&ns), event),
            Err(err) => return Err(xml_error(err, base + reader.error_position())),
        };
        match event {
            Event::Decl(decl) => check_decl(&decl, at)?,
            Event::DocType(d) => doc.doctype(&d)?,
            Event::Start(e) => doc.open(reader.resolver(), &e, ns, false, at)?,
            Event::Empty(e) => doc.open(reader.resolver(), &e, ns, true, at)?,
            Event::End(_) => doc.close(),
            Event::Text(t) => {
                let t = t.xml10_content().map_err(encoding_error)?;
                doc.text(&t, at)?;
            }
            Event::GeneralRef(r) => {
                let c = resolve_reference(&r, at)?;
                doc.text(c.encode_utf8(&mut [0; 4]), at)?;
            }
            Event::Comment(_) | Event::PI(_) | Event::CData(_) => {}
            Event::Eof => break,
        }
    }
    doc.finish(base + text.len() as u64)
}

fn is_svg_ns(ns: &ResolveResult<'_>) -> bool {
    matches!(ns, ResolveResult::Bound(n) if n.as_ref() == SVG_NS)
}

/// The XML 1.0 `Char` production.
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// `NotXml` with a fixed description and a byte offset; input text is never echoed.
fn not_xml(what: &'static str, at: u64) -> SvgError {
    SvgError::NotXml(format!("{what} at byte {at}"))
}

/// `UnknownEntity` with the name capped at `MAX_ENTITY_NAME` characters.
fn unknown_entity(name: &str) -> SvgError {
    SvgError::UnknownEntity(name.chars().take(MAX_ENTITY_NAME).collect())
}

fn xml_error(err: quick_xml::Error, at: u64) -> SvgError {
    let what = match err {
        quick_xml::Error::Escape(EscapeError::UnrecognizedEntity(_, name)) => {
            return unknown_entity(&name);
        }
        quick_xml::Error::Encoding(_) => return SvgError::NotUtf8,
        quick_xml::Error::Syntax(_) => "malformed markup",
        quick_xml::Error::IllFormed(_) => "ill-formed document",
        quick_xml::Error::InvalidAttr(_) => "malformed attribute",
        quick_xml::Error::Escape(_) => "malformed reference",
        quick_xml::Error::Namespace(_) => "namespace error",
        quick_xml::Error::Io(_) => "read error",
    };
    not_xml(what, at)
}

fn encoding_error(_: EncodingError) -> SvgError {
    SvgError::NotUtf8
}

/// The input is read as UTF-8 XML 1.0; a declaration claiming anything else is refused rather
/// than reinterpreted.
fn check_decl(decl: &BytesDecl<'_>, at: u64) -> Result<(), SvgError> {
    let version = decl
        .version()
        .map_err(|_| not_xml("malformed XML declaration", at))?;
    if version.as_ref() != b"1.0" {
        return Err(not_xml("only XML 1.0 is accepted", at));
    }
    match decl.encoding() {
        None => Ok(()),
        Some(Ok(enc)) if enc.eq_ignore_ascii_case(b"utf-8") => Ok(()),
        Some(Ok(_)) => Err(SvgError::NotUtf8),
        Some(Err(_)) => Err(not_xml("malformed XML declaration", at)),
    }
}

/// True for `<!DOCTYPE svg PUBLIC "<standard SVG id>" "<system id>">` with no internal subset.
/// `raw` is the declaration after `<!DOCTYPE` (quick-xml strips the leading whitespace).
fn is_standard_svg_doctype(raw: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(raw) else {
        return false;
    };
    if s.contains('[') {
        return false;
    }
    let Some(rest) = s.strip_prefix("svg") else {
        return false;
    };
    let Some(rest) = after_space(rest).and_then(|r| r.strip_prefix("PUBLIC")) else {
        return false;
    };
    let Some((public, rest)) = after_space(rest).and_then(quoted) else {
        return false;
    };
    let Some((_system, rest)) = after_space(rest).and_then(quoted) else {
        return false;
    };
    rest.chars().all(is_xml_space) && SVG_PUBLIC_IDS.contains(&public)
}

/// `s` without its leading whitespace, or `None` when it has none.
fn after_space(s: &str) -> Option<&str> {
    let trimmed = s.trim_start_matches(is_xml_space);
    (trimmed.len() < s.len()).then_some(trimmed)
}

/// The quoted literal at the start of `s` and what follows it.
fn quoted(s: &str) -> Option<(&str, &str)> {
    let quote = s.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let body = &s[1..];
    let end = body.find(quote)?;
    Some((&body[..end], &body[end + 1..]))
}

/// `&lt;` and friends or a numeric reference; anything else is an unknown entity.
fn resolve_reference(r: &BytesRef<'_>, at: u64) -> Result<char, SvgError> {
    let name = r.decode().map_err(encoding_error)?;
    if r.is_char_ref() {
        let c = r
            .resolve_char_ref()
            .map_err(|_| not_xml("malformed character reference", at))?
            .ok_or_else(|| not_xml("malformed character reference", at))?;
        if !is_xml_char(c) {
            return Err(not_xml("character reference not allowed in XML", at));
        }
        return Ok(c);
    }
    match name.as_ref() {
        "lt" => Ok('<'),
        "gt" => Ok('>'),
        "amp" => Ok('&'),
        "apos" => Ok('\''),
        "quot" => Ok('"'),
        other => Err(unknown_entity(other)),
    }
}

enum Node {
    Start {
        name: &'static str,
        attrs: Vec<(&'static str, String)>,
        empty: bool,
    },
    End(&'static str),
    Text(String),
}

impl Node {
    fn is_use(&self) -> bool {
        matches!(self, Node::Start { name: "use", .. })
    }
}

#[derive(Default)]
struct Sanitizer {
    nodes: Vec<Node>,
    /// Depth of the element being read (the root is 1), dropped elements included.
    depth: usize,
    elements: usize,
    /// Kept `<use>` elements.
    uses: usize,
    /// Depth at which the dropped subtree being skipped started.
    skip_from: Option<usize>,
    /// Kept elements currently open, innermost last.
    open: Vec<&'static str>,
    doctype_seen: bool,
    root_seen: bool,
    root_done: bool,
    xlink: bool,
}

impl Sanitizer {
    /// One standard SVG DOCTYPE before the root is accepted (and not written out).
    fn doctype(&mut self, d: &BytesText<'_>) -> Result<(), SvgError> {
        if self.doctype_seen || self.root_seen || !is_standard_svg_doctype(d) {
            return Err(SvgError::Doctype);
        }
        self.doctype_seen = true;
        Ok(())
    }

    fn open(
        &mut self,
        resolver: &NamespaceResolver,
        e: &BytesStart<'_>,
        svg_ns: bool,
        empty: bool,
        at: u64,
    ) -> Result<(), SvgError> {
        if self.root_done {
            return Err(not_xml("content after the root element", at));
        }
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(SvgError::TooDeep);
        }
        self.elements += 1;
        if self.elements > MAX_ELEMENTS {
            return Err(SvgError::TooManyElements);
        }
        let attrs = read_attributes(e, at)?;
        let local =
            std::str::from_utf8(e.local_name().into_inner()).map_err(|_| SvgError::NotUtf8)?;
        if !self.root_seen {
            if !(svg_ns && local == "svg") {
                return Err(SvgError::NotSvg);
            }
            self.root_seen = true;
        }

        let kept = if self.skip_from.is_none() && svg_ns {
            ELEMENTS.iter().find(|n| **n == local).copied()
        } else {
            None
        };
        match kept {
            Some(name) => {
                if name == "use" {
                    self.uses += 1;
                    if self.uses > MAX_USES {
                        return Err(SvgError::TooManyUses);
                    }
                }
                let attrs = self.filter_attributes(resolver, name, &attrs);
                self.nodes.push(Node::Start { name, attrs, empty });
                if !empty {
                    self.open.push(name);
                }
            }
            None if !empty && self.skip_from.is_none() => self.skip_from = Some(self.depth),
            None => {}
        }
        if empty {
            self.leave();
        }
        Ok(())
    }

    fn close(&mut self) {
        match self.skip_from {
            Some(from) if from == self.depth => self.skip_from = None,
            Some(_) => {}
            None => {
                if let Some(name) = self.open.pop() {
                    self.nodes.push(Node::End(name));
                }
            }
        }
        self.leave();
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0 {
            self.root_done = true;
        }
    }

    fn text(&mut self, text: &str, at: u64) -> Result<(), SvgError> {
        if self.depth == 0 {
            if text.chars().all(is_xml_space) {
                return Ok(());
            }
            return Err(not_xml("text outside the root element", at));
        }
        if self.skip_from.is_some() {
            return Ok(());
        }
        if !self.open.last().is_some_and(|n| TEXT_ELEMENTS.contains(n)) {
            return Ok(());
        }
        match self.nodes.last_mut() {
            Some(Node::Text(buf)) => buf.push_str(text),
            _ => self.nodes.push(Node::Text(text.to_owned())),
        }
        Ok(())
    }

    fn filter_attributes(
        &mut self,
        resolver: &NamespaceResolver,
        element: &'static str,
        attrs: &[(Vec<u8>, String)],
    ) -> Vec<(&'static str, String)> {
        let mut kept: Vec<(&'static str, String)> = Vec::new();
        for (key, value) in attrs {
            let qname = quick_xml::name::QName(key);
            if qname.as_namespace_binding().is_some() {
                continue;
            }
            let (ns, local) = resolver.resolve_attribute(qname);
            let Ok(local) = std::str::from_utf8(local.into_inner()) else {
                continue;
            };
            let entry = match ns {
                ResolveResult::Unbound => {
                    if local.to_ascii_lowercase().starts_with("on") {
                        None
                    } else if local == "href" {
                        href_value(element, value).map(|v| ("href", v))
                    } else {
                        ATTRIBUTES
                            .iter()
                            .find(|n| **n == local)
                            .and_then(|name| attribute_value(name, value).map(|v| (*name, v)))
                    }
                }
                ResolveResult::Bound(n) if n.as_ref() == XLINK_NS && local == "href" => {
                    href_value(element, value).map(|v| ("xlink:href", v))
                }
                ResolveResult::Bound(n)
                    if n.as_ref() == XML_NS
                        && local == "space"
                        && matches!(value.as_str(), "default" | "preserve") =>
                {
                    Some(("xml:space", value.clone()))
                }
                _ => None,
            };
            if let Some((name, value)) = entry
                && !kept.iter().any(|(n, _)| *n == name)
            {
                if name == "xlink:href" {
                    self.xlink = true;
                }
                kept.push((name, value));
            }
        }
        kept
    }

    fn finish(self, end: u64) -> Result<Vec<u8>, SvgError> {
        if !self.root_seen {
            return Err(SvgError::NotSvg);
        }
        if self.depth != 0 {
            return Err(not_xml("unclosed element", end));
        }
        check_use_references(&self.nodes)?;
        let mut writer = Writer::new(Vec::new());
        let io = |_: std::io::Error| SvgError::NotXml("write failed".to_owned());
        writer
            .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .map_err(io)?;
        let mut first = true;
        for node in &self.nodes {
            match node {
                Node::Start { name, attrs, empty } => {
                    let mut start = BytesStart::new(*name);
                    if first {
                        start.push_attribute(("xmlns", "http://www.w3.org/2000/svg"));
                        if self.xlink {
                            start.push_attribute(("xmlns:xlink", "http://www.w3.org/1999/xlink"));
                        }
                        first = false;
                    }
                    for (key, value) in attrs {
                        let escaped = escape_attribute(value);
                        start.push_attribute(Attribute::from((key.as_bytes(), escaped.as_bytes())));
                    }
                    let event = if *empty {
                        Event::Empty(start)
                    } else {
                        Event::Start(start)
                    };
                    writer.write_event(event).map_err(io)?;
                }
                Node::End(name) => writer
                    .write_event(Event::End(BytesEnd::new(*name)))
                    .map_err(io)?,
                Node::Text(text) => {
                    let escaped = escape(text.as_str()).replace('\r', "&#13;");
                    writer
                        .write_event(Event::Text(BytesText::from_escaped(escaped)))
                        .map_err(io)?;
                }
            }
        }
        Ok(writer.into_inner())
    }
}

/// Refuses a kept `<use>` whose `#id` names a `<use>` or an element with a `<use>` inside it
/// (every element carrying that id is checked). With at most `MAX_USES` uses and no chains, each
/// use instantiates a use-free subtree, so instancing stays linear in the document size.
fn check_use_references(nodes: &[Node]) -> Result<(), SvgError> {
    // Index of the closing node of every start node (itself when empty).
    let mut end = vec![0usize; nodes.len()];
    let mut stack = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        match node {
            Node::Start { empty: true, .. } => end[i] = i,
            Node::Start { .. } => stack.push(i),
            Node::End(_) => {
                if let Some(start) = stack.pop() {
                    end[start] = i;
                }
            }
            Node::Text(_) => {}
        }
    }
    // `uses_before[i]`: number of `<use>` start nodes in `nodes[..i]`.
    let mut uses_before = Vec::with_capacity(nodes.len() + 1);
    uses_before.push(0usize);
    for node in nodes {
        let so_far = uses_before.last().copied().unwrap_or(0);
        uses_before.push(so_far + usize::from(node.is_use()));
    }
    let mut ids: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if let Node::Start { attrs, .. } = node
            && let Some((_, id)) = attrs.iter().find(|(k, _)| *k == "id")
        {
            ids.entry(id.as_str()).or_default().push(i);
        }
    }
    for node in nodes {
        let Node::Start {
            name: "use", attrs, ..
        } = node
        else {
            continue;
        };
        for (_, href) in attrs
            .iter()
            .filter(|(k, _)| matches!(*k, "href" | "xlink:href"))
        {
            let Some(target) = href.strip_prefix('#') else {
                continue;
            };
            for &t in ids.get(target).map(Vec::as_slice).unwrap_or_default() {
                if uses_before[end[t] + 1] > uses_before[t] {
                    return Err(SvgError::TooManyUses);
                }
            }
        }
    }
    Ok(())
}

/// Every attribute of `e` as (raw key, normalised value), enforcing the attribute caps and
/// refusing unknown entities and characters XML forbids.
fn read_attributes(e: &BytesStart<'_>, at: u64) -> Result<Vec<(Vec<u8>, String)>, SvgError> {
    let mut out = Vec::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|_| not_xml("malformed attribute", at))?;
        if out.len() == MAX_ATTRIBUTES {
            return Err(SvgError::TooManyAttributes);
        }
        if attr.value.len() > MAX_VALUE_BYTES {
            return Err(SvgError::ValueTooLong);
        }
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|err| xml_error(err, at))?;
        if value.len() > MAX_VALUE_BYTES {
            return Err(SvgError::ValueTooLong);
        }
        if value.chars().any(|c| !is_xml_char(c)) {
            return Err(not_xml("character reference not allowed in XML", at));
        }
        out.push((attr.key.as_ref().to_vec(), value.into_owned()));
    }
    Ok(out)
}

/// Escapes a value for a double-quoted attribute; whitespace characters other than space are
/// written as references so re-parsing yields the same value.
fn escape_attribute(value: &str) -> String {
    escape(value)
        .replace('\t', "&#9;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
}

/// True when the value contains `\` or one of `ACTIVE_TOKENS`.
fn has_active_token(value: &str) -> bool {
    if value.contains('\\') {
        return true;
    }
    let squashed: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .flat_map(char::to_lowercase)
        .collect();
    ACTIVE_TOKENS.iter().any(|t| squashed.contains(t))
}

/// True when every `url(` in the value points into the document (`url(#…)`).
fn urls_are_local(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("url(") {
        rest = &rest[at + 4..];
        let target = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'');
        if !target.starts_with('#') {
            return false;
        }
    }
    true
}

/// A fragment id that cannot break out of a reference.
fn is_local_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !"()'\"<>\\".contains(c))
}

/// `none`, or exactly one `url(#id)` (the id optionally quoted, whitespace around it allowed).
fn is_none_or_local_url(value: &str) -> bool {
    let value = value.trim_matches(is_xml_space);
    if value.eq_ignore_ascii_case("none") {
        return true;
    }
    let Some(inner) = value
        .get(..4)
        .filter(|p| p.eq_ignore_ascii_case("url("))
        .and_then(|_| value.get(4..))
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let inner = inner.trim_matches(is_xml_space);
    let inner = ['"', '\'']
        .iter()
        .find_map(|q| inner.strip_prefix(*q).and_then(|s| s.strip_suffix(*q)))
        .unwrap_or(inner);
    inner.strip_prefix('#').is_some_and(is_local_id)
}

/// A kept attribute's value, or `None` when the value is not safe.
fn attribute_value(name: &str, value: &str) -> Option<String> {
    if has_active_token(value) || !urls_are_local(value) {
        return None;
    }
    if name == "style" {
        return style_value(value);
    }
    if REFERENCE_PROPERTIES.contains(&name) && !is_none_or_local_url(value) {
        return None;
    }
    Some(value.to_owned())
}

/// A `style` value reduced to allowlisted declarations, written `prop:value;…`.
fn style_value(value: &str) -> Option<String> {
    if ["{", "}", "<", ">", "/*", "@"]
        .iter()
        .any(|t| value.contains(t))
    {
        return None;
    }
    let mut kept = Vec::new();
    for decl in value.split(';') {
        let Some((prop, val)) = decl.split_once(':') else {
            continue;
        };
        let prop = prop.trim().to_ascii_lowercase();
        let val = val.trim();
        let balanced = val.matches('"').count() % 2 == 0 && val.matches('\'').count() % 2 == 0;
        let reference_ok =
            !REFERENCE_PROPERTIES.contains(&prop.as_str()) || is_none_or_local_url(val);
        if val.is_empty()
            || !balanced
            || !reference_ok
            || !STYLE_PROPERTIES.contains(&prop.as_str())
        {
            continue;
        }
        kept.push(format!("{prop}:{val}"));
    }
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(";"))
    }
}

/// An `href`/`xlink:href` value: a local `#id`, or on `<image>` a base64 raster data URI.
fn href_value(element: &str, value: &str) -> Option<String> {
    if has_active_token(value) {
        return None;
    }
    let value = value.trim_matches(is_xml_space);
    if let Some(id) = value.strip_prefix('#') {
        return is_local_id(id).then(|| value.to_owned());
    }
    if element != "image" {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let rest = lower.strip_prefix("data:image/")?;
    let (kind, _) = rest.split_once(";base64,")?;
    if !DATA_IMAGE_TYPES.contains(&kind) {
        return None;
    }
    let payload_at = "data:image/".len() + kind.len() + ";base64,".len();
    let payload: String = value[payload_at..]
        .chars()
        .filter(|c| !is_xml_space(*c))
        .collect();
    let ok = !payload.is_empty()
        && payload
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
    ok.then(|| format!("data:image/{kind};base64,{payload}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_urls_only() {
        assert!(urls_are_local("url(#a)"));
        assert!(urls_are_local("url( '#a' ) url(\"#b\")"));
        assert!(!urls_are_local("url(#a) url(https://x)"));
        assert!(!urls_are_local("URL(x)"));
        assert!(urls_are_local("red"));
    }

    #[test]
    fn reference_properties_are_none_or_one_local_url() {
        for ok in ["none", " NONE ", "url(#m)", "URL( '#m' )", "url(\"#m\")"] {
            assert!(is_none_or_local_url(ok), "{ok}");
        }
        for bad in [
            "",
            "url(#)",
            "url(#m) url(#n)",
            "url(#m) red",
            "url('#m\")",
            "image-set('https://evil/x.png' 1x)",
            "linear-gradient(red, blue)",
            "url(https://evil/x)",
            "#m",
        ] {
            assert!(!is_none_or_local_url(bad), "{bad}");
        }
    }

    #[test]
    fn image_functions_are_active_tokens() {
        for bad in [
            "image-set('https://e/x' 1x)",
            "-WEBKIT-Image-Set( 'x' 1x )",
            "image('x')",
            "cross-fade('a','b',50%)",
            "-webkit-cross-fade(url(#a), url(#b), 50%)",
            "element(#a)",
            "ima ge-set(x)",
        ] {
            assert!(has_active_token(bad), "{bad}");
        }
        assert!(!has_active_token("url(#a) red"));
    }

    #[test]
    fn standard_svg_doctypes_only() {
        assert!(is_standard_svg_doctype(
            br#"svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd""#
        ));
        assert!(is_standard_svg_doctype(
            b"svg PUBLIC '-//W3C//DTD SVG 1.0//EN'\n  'http://www.w3.org/TR/2001/REC-SVG-20010904/DTD/svg10.dtd' "
        ));
        for bad in [
            &b"svg"[..],
            b"svg SYSTEM \"x.dtd\"",
            b"svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\"",
            b"svg PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x\"",
            b"svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\" [<!ENTITY a \"b\">]",
            b"html PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\"",
            b"svgx PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\"",
            b"svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\" junk",
        ] {
            assert!(
                !is_standard_svg_doctype(bad),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn style_keeps_allowlisted_declarations() {
        assert_eq!(
            style_value("Fill: red; position: fixed"),
            Some("fill:red".to_owned())
        );
        assert_eq!(style_value("position: fixed"), None);
        assert_eq!(style_value("fill: red } x {"), None);
        assert_eq!(style_value("font-family: 'a"), None);
        assert_eq!(
            style_value("mask: linear-gradient(red, blue); fill: red; filter: url(#f)"),
            Some("fill:red;filter:url(#f)".to_owned())
        );
    }

    #[test]
    fn hrefs_are_local_or_raster_data() {
        assert_eq!(href_value("use", " #a "), Some("#a".to_owned()));
        assert_eq!(href_value("use", "#"), None);
        assert_eq!(href_value("use", "#a b"), None);
        assert_eq!(href_value("use", "x.svg#a"), None);
        assert_eq!(href_value("use", "data:image/png;base64,AA=="), None);
        assert_eq!(
            href_value("image", "Data:Image/GIF;base64,R0l GOD=="),
            Some("data:image/gif;base64,R0lGOD==".to_owned())
        );
        assert_eq!(href_value("image", "data:image/svg+xml;base64,AA=="), None);
        assert_eq!(href_value("image", "data:image/png,raw"), None);
    }
}
