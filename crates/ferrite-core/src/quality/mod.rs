//! Accessibility and content health (design 16): one rule engine for the editor's
//! checks, the MCP tools, the per-type publish gate and the daily health run.
//!
//! - [`rules`]: the catalogue ([`RULES`]): stable ids with a severity, a category, the
//!   WCAG 2.2 criteria and a scope.
//! - [`check_document`]: every item-scope rule that needs no store read and no probe
//!   (pure; iterative and capped, §4.3).
//!
//! Core stores no prose: a finding is a rule id, a location, a suppression key, the text
//! found (`context`) and a rule-specific `detail`; the admin and the MCP layer word it.

mod check;
pub mod facts;
pub mod gate;
pub mod rules;
mod service;
mod vague;
mod walk;

use ferrite_domain::{ContentId, RedirectId};
use serde_json::Value;

use crate::fields::FieldDef;

pub use check::check_document;
pub use rules::{Category, RULES, Rule, RuleScope, Severity, rule};
pub use service::{
    CheckDraft, CheckReport, CheckedFinding, Counts, GateState, MAX_CHECK_PATHS, MAX_REASON,
    QualityService,
};
pub use vague::is_vague;

/// Most findings of one item; past it the walk stops with `check-truncated` (§4.3).
pub const MAX_ITEM_FINDINGS: usize = 500;
/// Most links of one item; past it the walk stops with `check-truncated`.
pub const MAX_ITEM_LINKS: usize = 1_000;
/// Longest `context` (characters).
pub const MAX_CONTEXT: usize = 120;
/// Longest suppression key (characters).
pub const MAX_KEY: usize = 300;

/// Where a finding is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Title,
    /// In the body: the child indices from the document root (`content[i]` per level).
    Body {
        path: Vec<u32>,
    },
    /// In a rich-text field: its path as the validation errors spell it
    /// (`chapters[1].summary`) and the indices inside its document.
    Field {
        name: String,
        path: Vec<u32>,
    },
    FeaturedImage,
    /// The item as a whole (orphan, stale, truncated).
    Item,
    Redirect {
        id: RedirectId,
    },
}

/// One problem found.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub rule: &'static Rule,
    pub location: Location,
    /// The suppression key (§7.2), at most [`MAX_KEY`] characters.
    pub key: String,
    /// What was found (link text, alt, href, heading text), at most [`MAX_CONTEXT`]
    /// characters.
    pub context: Option<String>,
    /// Rule-specific facts, e.g. `{"href": "/x/", "status": 404}`.
    pub detail: Value,
}

/// What the document checks read of an item (or of the editor's draft).
#[derive(Debug, Clone, Copy)]
pub struct DocInput<'a> {
    pub title: &'a str,
    pub body: &'a Value,
    pub fields: &'a Value,
    /// The type's field schema (empty: no rich-text fields are checked).
    pub field_defs: &'a [FieldDef],
    /// The item's locale (BCP 47), for the vague-phrase lists.
    pub locale: &'a str,
}

/// A link found in a body or a rich-text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundLink {
    /// The `href` as written.
    pub href: String,
    /// Internal: the path and query on this site (`/a/b?x=1`, no fragment). External:
    /// the absolute URL without its fragment.
    pub target: String,
    /// The link's joined text.
    pub text: String,
    pub location: Location,
}

impl FoundLink {
    /// The suppression key of a finding about this link: `href|text`.
    pub fn key(&self) -> String {
        cut(&format!("{}|{}", self.href, self.text), MAX_KEY)
    }
}

/// The links of an item, split by where they lead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Links {
    /// Local paths and absolute URLs on the site's origin.
    pub internal: Vec<FoundLink>,
    /// Other `http(s)` URLs.
    pub external: Vec<FoundLink>,
}

/// A media item an item shows: an inline `ferrite:media/<id>` image or the featured
/// image (the service loads facts about these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRef {
    pub id: ContentId,
    pub location: Location,
    /// The finding key: the image's `src`, or `featured_image`.
    pub key: String,
    pub featured: bool,
}

/// What a path on this site answers (design 16 §6.2), as [`PathProbe`] finds it without
/// rendering anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathStatus {
    /// Something is there (`item`: the content item it shows, if one).
    Ok {
        item: Option<ContentId>,
    },
    /// A stored redirect or a canonical `301` leads to `to` (the final path).
    Redirected {
        to: String,
        item: Option<ContentId>,
    },
    NotFound,
    Gone,
    /// A stored redirect leads off the site.
    External(String),
    /// Outside the public router (plugin routes, the admin, the API, MCP, OAuth).
    Unchecked,
}

/// Resolves a path on this site without rendering, caching, counting or calling a
/// plugin (`ferrite-http`'s `SiteProbe`; D9).
#[async_trait::async_trait]
pub trait PathProbe: Send + Sync {
    /// `path_and_query`: a local path, starting with `/`.
    async fn probe(&self, path_and_query: &str) -> PathStatus;
}

/// The first `max` characters of `s`.
pub(crate) fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_owned(),
        None => s.to_owned(),
    }
}

/// `s` without format characters (`Cf`), trimmed.
pub(crate) fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !crate::media::is_format(*c))
        .collect::<String>()
        .trim()
        .to_owned()
}
