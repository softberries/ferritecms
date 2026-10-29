//! Whether html5ever's tree builder would take quadratic time on a fragment, measured with
//! the tree builder itself.
//!
//! Imported markup is untrusted. Tree building is quadratic in the nesting depth, and in the
//! number of formatting markers that `<object>`, `<applet>` and `<marquee>` leave behind
//! (foster-parented out of a table, they are closed without clearing their marker, and every
//! later formatting end tag scans past it). Markup past [`MAX_PARSE_DEPTH`] or
//! [`MAX_MARKER_ELEMENTS`] is not parsed for real. The measuring parse runs html5ever with a
//! [`TreeSink`] that keeps only parent links, and feeds the input in chunks: once a limit is
//! passed it stops, so a body that trips the guard costs a bounded amount of work.

use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

use html5ever::driver::{ParseOpts, parse_fragment};
use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, QualName, local_name, ns};

/// Markup whose elements nest deeper than this is not parsed.
pub const MAX_PARSE_DEPTH: usize = 512;

/// Markup with more marker-pushing elements than this is not parsed. `td`, `th`, `caption`
/// and `template` push markers too, but their markers are cleared when they close (measured:
/// linear), and real tables have thousands of cells; they are not counted.
pub const MAX_MARKER_ELEMENTS: usize = 1024;

/// Elements that push a marker onto the list of active formatting elements and can leave it
/// there.
const MARKER_ELEMENTS: &[&str] = &["object", "applet", "marquee"];

/// Whether `html` is too costly to parse: nested past [`MAX_PARSE_DEPTH`], or holding more
/// than [`MAX_MARKER_ELEMENTS`] marker-pushing elements.
pub(crate) fn too_deep(html: &str) -> bool {
    let m = measure(html);
    m.depth > MAX_PARSE_DEPTH || m.markers > MAX_MARKER_ELEMENTS
}

/// What the measuring parse saw, up to where it stopped.
#[derive(Debug, Default, Clone, Copy)]
struct Measure {
    depth: usize,
    markers: usize,
}

impl Measure {
    fn past_limits(&self) -> bool {
        self.depth > MAX_PARSE_DEPTH || self.markers > MAX_MARKER_ELEMENTS
    }
}

/// How much input is fed between two depth checks.
const CHUNK: usize = 4096;

/// Depths are counted up to this; anything deeper is past the limit anyway.
const CAP: usize = MAX_PARSE_DEPTH + 1;

/// The depth of the deepest node the tree builder creates for `html` (parsed as a `<body>`
/// fragment, as `scraper::Html::parse_fragment` parses it; an over-estimate where nodes are
/// moved) and its marker-pushing elements, counted until either passes its limit.
fn measure(html: &str) -> Measure {
    let seen = Rc::new(Cell::new(Measure::default()));
    let mut parser = parse_fragment(
        DepthSink::new(Rc::clone(&seen)),
        ParseOpts::default(),
        QualName::new(None, ns!(html), local_name!("body")),
        Vec::new(),
        false,
    );
    let mut rest = html;
    while !rest.is_empty() {
        let mut at = CHUNK.min(rest.len());
        while !rest.is_char_boundary(at) {
            at += 1;
        }
        let (chunk, tail) = rest.split_at(at);
        parser.process(StrTendril::from_slice(chunk));
        if seen.get().past_limits() {
            return seen.get();
        }
        rest = tail;
    }
    parser.finish();
    seen.get()
}

struct Node {
    name: QualName,
    parent: Option<usize>,
    /// Children as attached (a moved child stays listed; its `parent` tells).
    children: Vec<usize>,
    /// At least the height of the subtree below (it only grows).
    height: usize,
    template_contents: Option<usize>,
}

/// A tree sink that keeps only the shape of the tree. Handles are arena indexes; the
/// document is 0. Text is not kept (it adds nothing below an element).
struct DepthSink {
    nodes: RefCell<Vec<Node>>,
    seen: Rc<Cell<Measure>>,
}

impl DepthSink {
    fn new(seen: Rc<Cell<Measure>>) -> Self {
        let sink = Self {
            nodes: RefCell::new(Vec::new()),
            seen,
        };
        sink.node(QualName::new(None, ns!(), local_name!("")));
        sink
    }

    fn node(&self, name: QualName) -> usize {
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(Node {
            name,
            parent: None,
            children: Vec::new(),
            height: 0,
            template_contents: None,
        });
        nodes.len() - 1
    }

    /// Makes `child` (detached first) the last child of `parent`, and records how deep its
    /// subtree now reaches.
    fn attach(&self, parent: usize, child: usize) {
        let mut nodes = self.nodes.borrow_mut();
        nodes[child].parent = Some(parent);
        nodes[parent].children.push(child);
        let mut depth = 0;
        let mut at = parent;
        while let Some(p) = nodes[at].parent {
            depth += 1;
            if depth >= CAP {
                break;
            }
            at = p;
        }
        let below = nodes[child].height + 1;
        let mut seen = self.seen.get();
        seen.depth = seen.depth.max(depth + below);
        self.seen.set(seen);
        let mut h = below;
        let mut at = Some(parent);
        let mut steps = 0;
        while let Some(n) = at {
            if nodes[n].height >= h || steps >= CAP {
                break;
            }
            nodes[n].height = h;
            h += 1;
            steps += 1;
            at = nodes[n].parent;
        }
    }

    fn parent_of(&self, node: usize) -> Option<usize> {
        self.nodes.borrow()[node].parent
    }
}

impl TreeSink for DepthSink {
    type Handle = usize;
    type Output = ();
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) {}

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> usize {
        0
    }

    fn elem_name<'a>(&'a self, target: &'a usize) -> Ref<'a, QualName> {
        Ref::map(self.nodes.borrow(), |nodes| &nodes[*target].name)
    }

    fn create_element(&self, name: QualName, _: Vec<Attribute>, flags: ElementFlags) -> usize {
        if name.ns == ns!(html) && MARKER_ELEMENTS.contains(&&*name.local) {
            let mut seen = self.seen.get();
            seen.markers += 1;
            self.seen.set(seen);
        }
        let element = self.node(name);
        if flags.template {
            let contents = self.node(QualName::new(None, ns!(), local_name!("")));
            self.nodes.borrow_mut()[element].template_contents = Some(contents);
            self.attach(element, contents);
        }
        element
    }

    fn create_comment(&self, _: StrTendril) -> usize {
        self.node(QualName::new(None, ns!(), local_name!("")))
    }

    fn create_pi(&self, _: StrTendril, _: StrTendril) -> usize {
        self.node(QualName::new(None, ns!(), local_name!("")))
    }

    fn append(&self, parent: &usize, child: NodeOrText<usize>) {
        if let NodeOrText::AppendNode(child) = child {
            self.attach(*parent, child);
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &usize,
        prev_element: &usize,
        child: NodeOrText<usize>,
    ) {
        if self.parent_of(*element).is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}

    fn get_template_contents(&self, target: &usize) -> usize {
        let contents = self.nodes.borrow()[*target].template_contents;
        // The tree builder asks only for templates, which were created with contents.
        contents.unwrap_or(*target)
    }

    fn same_node(&self, x: &usize, y: &usize) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &usize, new_node: NodeOrText<usize>) {
        if let (Some(parent), NodeOrText::AppendNode(child)) = (self.parent_of(*sibling), new_node)
        {
            self.attach(parent, child);
        }
    }

    fn add_attrs_if_missing(&self, _: &usize, _: Vec<Attribute>) {}

    fn remove_from_parent(&self, target: &usize) {
        self.nodes.borrow_mut()[*target].parent = None;
    }

    fn reparent_children(&self, node: &usize, new_parent: &usize) {
        let children = std::mem::take(&mut self.nodes.borrow_mut()[*node].children);
        for child in children {
            if self.parent_of(child) == Some(*node) {
                self.attach(*new_parent, child);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn depth(html: &str) -> usize {
        measure(html).depth
    }

    #[test]
    fn measures_the_built_tree() {
        assert_eq!(depth(""), 1);
        // html > div > div; text adds nothing.
        assert_eq!(depth("<div><div></div><b>x</b></div>"), 3);
        assert_eq!(depth(&"<p>x".repeat(10_000)), 2);
        assert_eq!(depth(&"<li>x".repeat(10_000)), 2);
        assert_eq!(depth(&format!("<svg>{}</svg>", "<path/>".repeat(1_000))), 3);
        // The adoption agency moves the `<p>` (and what follows) out of `<b>`: counted where
        // it ends up.
        assert!(depth("<b><p>x</b>y</p>") <= 4);
    }

    #[test]
    fn deep_but_legal_markup_is_under_the_limit() {
        let html = format!("{}x{}", "<div>".repeat(500), "</div>".repeat(500));
        assert!(depth(&html) <= MAX_PARSE_DEPTH);
        for run in ["<p>a", "<li>b", "<dt>c<dd>d", "<option>e", "<tr><td>f"] {
            let depth = depth(&run.repeat(10_000));
            assert!(depth <= 8, "{run}: {depth}");
        }
    }

    /// Patterns that nest about 40 000 deep in the tree builder: each must be seen, fast.
    /// One body 40 000 `<div>`s deep: the chunked feed stops soon after the limit, so the
    /// quadratic tree building stays small.
    #[test]
    fn a_deep_body_costs_well_under_a_second() {
        let html = format!("{}x{}", "<div>".repeat(40_000), "</div>".repeat(40_000));
        let start = Instant::now();
        assert!(depth(&html) > MAX_PARSE_DEPTH);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn marker_elements_are_counted_and_cells_are_not() {
        let m = measure(&"<object></object><applet></applet><marquee></marquee>".repeat(400));
        assert!(m.markers > MAX_MARKER_ELEMENTS);
        assert!(too_deep(&"<marquee>x</marquee>".repeat(1_100)));
        let table = format!(
            "<table>{}</table><template></template>",
            "<tr><th>h<td>c<td><table><caption>x</caption></table>".repeat(2_000)
        );
        assert_eq!(measure(&table).markers, 0);
        assert!(!too_deep(&table));
    }

    #[test]
    fn deep_patterns_trip_and_stop_early() {
        let n = 40_000;
        let cases: Vec<(&str, String)> = vec![
            ("nested div", "<div>".repeat(n)),
            ("unmatched end tags", "<div></x>".repeat(n)),
            ("names past a punctuation byte", "<p_>".repeat(n)),
            (
                "comment ended by <!-->",
                format!("<!-->{}", "<div>".repeat(n)),
            ),
            (
                "raw-text name inside svg",
                format!("<svg><title>{}", "<div>".repeat(n)),
            ),
            ("span end inside div", "<span><div></span>".repeat(n)),
            ("div end past object", "<div><object></div>".repeat(n)),
            ("formatting end past div", "<b><div></b>".repeat(n)),
            ("li then dd", "<li><dd>".repeat(n)),
            ("self-closing outside foreign content", "<div/>".repeat(n)),
            (
                "svg breakout, then self-closing",
                format!("<svg><div>{}", "<div/>".repeat(n)),
            ),
            (
                "svg breakout, then CDATA",
                format!("<svg><div><![CDATA[>{}", "<div>".repeat(n)),
            ),
            (
                "foreignObject, then self-closing",
                format!("<svg><foreignObject>{}", "<div/>".repeat(n)),
            ),
            (
                "formatting reopened after an ancestor's end",
                format!(
                    "{}x",
                    (0..n)
                        .map(|i| format!("<div><b c={i}></div></i>"))
                        .collect::<String>()
                ),
            ),
        ];
        for (what, html) in cases {
            let start = Instant::now();
            let depth = depth(&html);
            let took = start.elapsed();
            assert!(depth > MAX_PARSE_DEPTH, "{what}: {depth}");
            // Bounded, not fast: each case stops after a fixed amount of work (about 0.5 s
            // for the reopened-formatting case in a debug build, whatever its length).
            // The limit leaves room for slow CI runners; a quadratic case takes minutes.
            assert!(took < Duration::from_secs(5), "{what}: {took:?}");
        }
    }
}
