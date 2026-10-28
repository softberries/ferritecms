//! How deep html5ever's tree builder nests a fragment, measured with the tree builder itself.
//!
//! Tree building is quadratic in the nesting depth, and imported markup is untrusted, so
//! markup nested past [`MAX_PARSE_DEPTH`] is not parsed for real. The measuring parse runs
//! html5ever with a [`TreeSink`] that keeps only parent links, and feeds the input in chunks:
//! once the depth passes the limit it stops, so a body that trips the guard costs a bounded
//! amount of work.

use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

use html5ever::driver::{ParseOpts, parse_fragment};
use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, QualName, local_name, ns};

/// Markup whose elements nest deeper than this is not parsed.
pub const MAX_PARSE_DEPTH: usize = 512;

/// How much input is fed between two depth checks.
const CHUNK: usize = 4096;

/// Depths are counted up to this; anything deeper is past the limit anyway.
const CAP: usize = MAX_PARSE_DEPTH + 1;

/// The depth of the deepest node the tree builder creates for `html` (parsed as a `<body>`
/// fragment, as `scraper::Html::parse_fragment` parses it), or a value past
/// [`MAX_PARSE_DEPTH`] as soon as it gets there. An over-estimate where nodes are moved.
pub(crate) fn nesting_depth(html: &str) -> usize {
    let max = Rc::new(Cell::new(0));
    let mut parser = parse_fragment(
        DepthSink::new(Rc::clone(&max)),
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
        if max.get() > MAX_PARSE_DEPTH {
            return max.get();
        }
        rest = tail;
    }
    parser.finish();
    max.get()
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
    max: Rc<Cell<usize>>,
}

impl DepthSink {
    fn new(max: Rc<Cell<usize>>) -> Self {
        let sink = Self {
            nodes: RefCell::new(Vec::new()),
            max,
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
        self.max.set(self.max.get().max(depth + below));
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

    #[test]
    fn measures_the_built_tree() {
        assert_eq!(nesting_depth(""), 1);
        // html > div > div; text adds nothing.
        assert_eq!(nesting_depth("<div><div></div><b>x</b></div>"), 3);
        assert_eq!(nesting_depth(&"<p>x".repeat(10_000)), 2);
        assert_eq!(nesting_depth(&"<li>x".repeat(10_000)), 2);
        assert_eq!(
            nesting_depth(&format!("<svg>{}</svg>", "<path/>".repeat(1_000))),
            3
        );
        // The adoption agency moves the `<p>` (and what follows) out of `<b>`: counted where
        // it ends up.
        assert!(nesting_depth("<b><p>x</b>y</p>") <= 4);
    }

    #[test]
    fn deep_but_legal_markup_is_under_the_limit() {
        let html = format!("{}x{}", "<div>".repeat(500), "</div>".repeat(500));
        assert!(nesting_depth(&html) <= MAX_PARSE_DEPTH);
        for run in ["<p>a", "<li>b", "<dt>c<dd>d", "<option>e", "<tr><td>f"] {
            let depth = nesting_depth(&run.repeat(10_000));
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
        assert!(nesting_depth(&html) > MAX_PARSE_DEPTH);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
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
            let depth = nesting_depth(&html);
            let took = start.elapsed();
            assert!(depth > MAX_PARSE_DEPTH, "{what}: {depth}");
            assert!(took < Duration::from_secs(1), "{what}: {took:?}");
        }
    }
}
