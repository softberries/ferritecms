//! The rule catalogue (design 16 §3): every rule's stable id, severity, category, the
//! WCAG 2.2 success criteria it relates to and where it runs.

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// A clear failure a reader meets.
    Error,
    /// Very likely a problem; a person decides.
    Warning,
    /// Worth a look.
    Notice,
}

impl Severity {
    pub const ALL: &'static [Severity] = &[Severity::Error, Severity::Warning, Severity::Notice];

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Notice => "notice",
        }
    }

    /// The severity spelled `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.as_str() == s)
    }
}

/// What a rule is about (the health screen's tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Accessibility,
    Links,
    Media,
    Site,
    Freshness,
}

impl Category {
    pub const ALL: &'static [Category] = &[
        Category::Accessibility,
        Category::Links,
        Category::Media,
        Category::Site,
        Category::Freshness,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Accessibility => "accessibility",
            Category::Links => "links",
            Category::Media => "media",
            Category::Site => "site",
            Category::Freshness => "freshness",
        }
    }

    /// The category spelled `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.as_str() == s)
    }
}

/// Where a rule runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleScope {
    /// One item's title, body and rich-text fields: the editor, MCP and the health job.
    Item,
    /// The whole site: the health job only.
    Site,
}

impl RuleScope {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleScope::Item => "item",
            RuleScope::Site => "site",
        }
    }
}

/// One rule of the catalogue. Ids are stable: never renamed, never reused.
#[derive(Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: &'static str,
    pub severity: Severity,
    pub category: Category,
    /// WCAG 2.2 success criteria, e.g. `1.1.1`.
    pub wcag: &'static [&'static str],
    pub scope: RuleScope,
}

const fn r(
    id: &'static str,
    severity: Severity,
    category: Category,
    wcag: &'static [&'static str],
    scope: RuleScope,
) -> Rule {
    Rule {
        id,
        severity,
        category,
        wcag,
        scope,
    }
}

use Category::{Accessibility, Freshness, Links, Media, Site};
use RuleScope::{Item, Site as SiteScope};
use Severity::{Error, Notice, Warning};

/// Every rule (design 16 §3, ruling P3's `image-alt-empty`, and the engine's own
/// `check-truncated` and `findings-truncated`).
pub static RULES: &[Rule] = &[
    r("image-alt-missing", Error, Accessibility, &["1.1.1"], Item),
    r("image-alt-empty", Warning, Accessibility, &["1.1.1"], Item),
    r(
        "image-alt-filename",
        Warning,
        Accessibility,
        &["1.1.1"],
        Item,
    ),
    r(
        "heading-empty",
        Error,
        Accessibility,
        &["1.3.1", "2.4.6"],
        Item,
    ),
    r(
        "heading-level-skipped",
        Warning,
        Accessibility,
        &["1.3.1"],
        Item,
    ),
    r(
        "link-text-empty",
        Error,
        Accessibility,
        &["2.4.4", "4.1.2"],
        Item,
    ),
    r("link-text-vague", Warning, Accessibility, &["2.4.4"], Item),
    r("link-text-url", Notice, Accessibility, &["2.4.4"], Item),
    r(
        "table-header-missing",
        Error,
        Accessibility,
        &["1.3.1"],
        Item,
    ),
    r(
        "table-header-empty",
        Warning,
        Accessibility,
        &["1.3.1"],
        Item,
    ),
    r("html-unchecked", Notice, Accessibility, &[], Item),
    r("media-missing", Error, Media, &[], Item),
    r("featured-alt-missing", Warning, Media, &["1.1.1"], Item),
    r("link-internal-broken", Error, Links, &[], Item),
    r("link-internal-redirected", Notice, Links, &[], Item),
    r("link-anchor-missing", Notice, Links, &[], Item),
    r("link-external-broken", Error, Links, &[], SiteScope),
    r("orphan", Warning, Site, &[], SiteScope),
    r("redirect-chain", Warning, Site, &[], SiteScope),
    r("redirect-loop", Error, Site, &[], SiteScope),
    r("redirect-target-broken", Error, Site, &[], SiteScope),
    r("stale", Notice, Freshness, &[], SiteScope),
    r("check-truncated", Notice, Accessibility, &[], Item),
    r("findings-truncated", Notice, Site, &[], SiteScope),
];

/// The rule named `id`.
pub fn rule(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

/// The rule named `id`, which must be in [`RULES`] (the engine's own ids).
pub(crate) fn known(id: &'static str) -> &'static Rule {
    // Every caller passes an id of the table above; the fallback keeps a typo from
    // panicking and is caught by `every_internal_id_is_known`.
    rule(id).unwrap_or(&RULES[RULES.len() - 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_shaped() {
        for (i, r) in RULES.iter().enumerate() {
            assert!(
                r.id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{}",
                r.id
            );
            assert!(RULES[..i].iter().all(|o| o.id != r.id), "{}", r.id);
            assert_eq!(rule(r.id), Some(r));
        }
        assert_eq!(rule("nope"), None);
        assert_eq!(known("check-truncated").id, "check-truncated");
    }

    #[test]
    fn enums_round_trip() {
        for s in Severity::ALL {
            assert_eq!(Severity::parse(s.as_str()), Some(*s));
        }
        for c in Category::ALL {
            assert_eq!(Category::parse(c.as_str()), Some(*c));
        }
        assert_eq!(Severity::parse("x"), None);
        assert_eq!(RuleScope::Item.as_str(), "item");
    }
}
