//! Permalinks: the public URL path of a content item.
//!
//! A [`Permalinks`] implementation maps a [`PermalinkInput`] (the item, its ancestors
//! and its primary category) to a path. It must be pure and cheap: core calls it inside
//! write transactions. [`PermalinkRules`] is the WordPress-style implementation built
//! from the `permalink.post` / `permalink.page` options.
//!
//! Structures are `/`-separated segments; each segment is either one token or a
//! literal. Tokens:
//!
//! - `%slug%`: the item's slug (required in every structure);
//! - `%year%`, `%month%`: from `published_at` (4 and 2 digits, UTC); items never
//!   published use `created_at`;
//! - `%category%`: the slug of the primary term (the first `category` term by
//!   position), `uncategorized` when there is none;
//! - `%parent_path%`: the ancestor slugs, root first, joined by `/`; it collapses to
//!   nothing for a top-level item.
//!
//! Every expanded segment is percent-encoded (UTF-8, uppercase hex); only RFC 3986
//! unreserved characters (`A-Z a-z 0-9 - . _ ~`) are kept. Paths always end in `/`
//! (the canonical form).

use ferrite_domain::{ContentId, ContentItem, Term};
use ferrite_store::Repos;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::CoreError;

/// Longest parent chain walked when loading ancestors.
const MAX_DEPTH: usize = 64;
/// Taxonomy whose first term is the primary term.
const PRIMARY_TAXONOMY: &str = "category";
/// `%category%` of an item without a category.
const UNCATEGORIZED: &str = "uncategorized";
/// Everything but RFC 3986 unreserved characters is encoded.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// What a permalink is computed from.
#[derive(Debug, Clone, PartialEq)]
pub struct PermalinkInput {
    pub item: ContentItem,
    /// Ancestors as (id, slug), root first; empty for a top-level item.
    pub ancestors: Vec<(ContentId, String)>,
    /// The item's first `category` term by position.
    pub primary_term: Option<Term>,
}

/// Maps an item to its public URL path. Must be pure and cheap: it runs inside write
/// transactions.
pub trait Permalinks: Send + Sync {
    fn path(&self, input: &PermalinkInput) -> String;
}

/// One segment of a [`Structure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Slug,
    Year,
    Month,
    Category,
    /// Zero or more segments: the ancestor slugs.
    ParentPath,
}

/// A parsed permalink structure such as `/%year%/%month%/%slug%/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    source: String,
    segments: Vec<Segment>,
}

impl Structure {
    /// Parses `s`: it must start and end with `/` and contain `%slug%`; each segment is
    /// a single known token or a literal of unreserved characters. Anything else is
    /// `Invalid`.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        let invalid = |why: &str| CoreError::Invalid(format!("permalink structure {s:?}: {why}"));
        let inner = s
            .strip_prefix('/')
            .and_then(|r| r.strip_suffix('/'))
            .ok_or_else(|| invalid("must start and end with '/'"))?;
        if inner.is_empty() {
            return Err(invalid("must contain %slug%"));
        }
        let mut segments = Vec::new();
        for part in inner.split('/') {
            let segment = match part {
                "" => return Err(invalid("empty segment")),
                "%slug%" => Segment::Slug,
                "%year%" => Segment::Year,
                "%month%" => Segment::Month,
                "%category%" => Segment::Category,
                "%parent_path%" => Segment::ParentPath,
                t if t.contains('%') => {
                    return Err(invalid(&format!(
                        "unknown token or mixed segment {t:?} (a segment is one token or a literal)"
                    )));
                }
                lit if lit == "." || lit == ".." => return Err(invalid("dot segment")),
                lit if lit.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
                }) =>
                {
                    Segment::Literal(lit.to_owned())
                }
                lit => return Err(invalid(&format!("literal {lit:?} has reserved characters"))),
            };
            segments.push(segment);
        }
        if !segments.contains(&Segment::Slug) {
            return Err(invalid("must contain %slug%"));
        }
        Ok(Self {
            source: s.to_owned(),
            segments,
        })
    }

    /// The structure as written.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Whether the path depends on the primary term.
    pub fn uses_category(&self) -> bool {
        self.segments.contains(&Segment::Category)
    }

    /// Expands the structure for `input`; `prefix` (already a safe literal) goes first.
    fn expand(&self, prefix: Option<&str>, input: &PermalinkInput) -> String {
        let item = &input.item;
        let date = item.published_at.unwrap_or(item.created_at).as_datetime();
        let mut path = String::from("/");
        let mut push = |raw: &str| {
            path.extend(utf8_percent_encode(raw, SEGMENT));
            path.push('/');
        };
        if let Some(prefix) = prefix {
            push(prefix);
        }
        for segment in &self.segments {
            match segment {
                Segment::Literal(l) => push(l),
                Segment::Slug => push(&item.slug),
                Segment::Year => push(&date.format("%Y").to_string()),
                Segment::Month => push(&date.format("%m").to_string()),
                Segment::Category => push(
                    input
                        .primary_term
                        .as_ref()
                        .map_or(UNCATEGORIZED, |t| t.slug.as_str()),
                ),
                Segment::ParentPath => {
                    for (_, slug) in &input.ancestors {
                        push(slug);
                    }
                }
            }
        }
        path
    }
}

/// WordPress-style permalink rules: one structure for posts, one for pages; any other
/// type `t` uses `/t/%parent_path%/%slug%/` (which is `/t/%slug%/` for non-hierarchical
/// types).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermalinkRules {
    post: Structure,
    page: Structure,
    other: Structure,
}

/// Default post structure.
pub const DEFAULT_POST: &str = "/%slug%/";
/// Default page structure.
pub const DEFAULT_PAGE: &str = "/%parent_path%/%slug%/";
/// Structure of every other type, after the `/{type}` prefix.
const OTHER: &str = "/%parent_path%/%slug%/";

impl PermalinkRules {
    /// Rules from the `permalink.post` and `permalink.page` option values (`None`: the
    /// default). An invalid structure is `Invalid`.
    pub fn from_options(post: Option<&str>, page: Option<&str>) -> Result<Self, CoreError> {
        Ok(Self {
            post: Structure::parse(post.unwrap_or(DEFAULT_POST))?,
            page: Structure::parse(page.unwrap_or(DEFAULT_PAGE))?,
            other: Self::default().other,
        })
    }

    pub fn post(&self) -> &Structure {
        &self.post
    }

    pub fn page(&self) -> &Structure {
        &self.page
    }
}

impl Default for PermalinkRules {
    /// Same as `from_options(None, None)`, built without parsing.
    fn default() -> Self {
        let structure = |source: &str, segments: Vec<Segment>| Structure {
            source: source.to_owned(),
            segments,
        };
        Self {
            post: structure(DEFAULT_POST, vec![Segment::Slug]),
            page: structure(DEFAULT_PAGE, vec![Segment::ParentPath, Segment::Slug]),
            other: structure(OTHER, vec![Segment::ParentPath, Segment::Slug]),
        }
    }
}

impl Permalinks for PermalinkRules {
    fn path(&self, input: &PermalinkInput) -> String {
        match input.item.kind.as_str() {
            "post" => self.post.expand(None, input),
            "page" => self.page.expand(None, input),
            kind => self.other.expand(Some(kind), input),
        }
    }
}

/// Loads the permalink input of `item` through `repos` (a transaction's repos inside a
/// write): the parent chain (at most 64 levels, `Invalid` beyond or on a missing
/// parent) and the item's terms.
pub(crate) async fn load_input(
    repos: &dyn Repos,
    item: ContentItem,
) -> Result<PermalinkInput, CoreError> {
    let ancestors = ancestors(repos, item.parent_id).await?;
    let terms = repos.terms().for_item(item.id).await?;
    Ok(PermalinkInput {
        item,
        ancestors,
        primary_term: primary_term(terms),
    })
}

/// The ancestors of an item whose parent is `parent`, root first.
pub(crate) async fn ancestors(
    repos: &dyn Repos,
    mut parent: Option<ContentId>,
) -> Result<Vec<(ContentId, String)>, CoreError> {
    let mut chain = Vec::new();
    while let Some(id) = parent {
        if chain.len() >= MAX_DEPTH {
            return Err(CoreError::Invalid(format!(
                "parent chain deeper than {MAX_DEPTH}"
            )));
        }
        let p = repos
            .content()
            .get(id)
            .await?
            .ok_or_else(|| CoreError::Invalid("unknown parent".into()))?;
        parent = p.parent_id;
        chain.push((p.id, p.slug));
    }
    chain.reverse();
    Ok(chain)
}

/// The first `category` term of `terms` (ordered by position).
pub(crate) fn primary_term(terms: Vec<Term>) -> Option<Term> {
    terms.into_iter().find(|t| t.taxonomy == PRIMARY_TAXONOMY)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ferrite_domain::{ContentStatus, TermId, Timestamp, UserId};
    use serde_json::json;

    fn item(kind: &str, slug: &str) -> ContentItem {
        // 2024-03-05T00:00:00Z created, 2025-11-20T00:00:00Z published.
        let created = Timestamp::from_micros(1_709_596_800_000_000).unwrap();
        ContentItem {
            id: ContentId::new(),
            kind: kind.into(),
            status: ContentStatus::Published,
            slug: slug.into(),
            parent_id: None,
            author_id: UserId::new(),
            title: String::new(),
            excerpt: String::new(),
            body: json!({}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: Some(Timestamp::from_micros(1_763_596_800_000_000).unwrap()),
            scheduled_at: None,
            created_at: created,
            updated_at: created,
            version: 1,
        }
    }

    fn category(taxonomy: &str, slug: &str) -> Term {
        Term {
            id: TermId::new(),
            taxonomy: taxonomy.into(),
            slug: slug.into(),
            name: slug.into(),
            description: String::new(),
            parent_id: None,
        }
    }

    fn input(item: ContentItem) -> PermalinkInput {
        PermalinkInput {
            item,
            ancestors: Vec::new(),
            primary_term: None,
        }
    }

    fn invalid(s: &str) -> String {
        match Structure::parse(s) {
            Err(CoreError::Invalid(m)) => m,
            other => panic!("{s:?}: expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn default_matches_parsed_defaults() {
        assert_eq!(
            PermalinkRules::default(),
            PermalinkRules::from_options(None, None).unwrap()
        );
    }

    #[test]
    fn structure_validation() {
        for s in [
            "%slug%/",
            "/%slug%",
            "/",
            "//",
            "/a//%slug%/",
            "/%year%/",
            "/%postname%/",
            "/%slug%.html/",
            "/%year%-%slug%/",
            "/a b/%slug%/",
            "/../%slug%/",
            "/a?b/%slug%/",
        ] {
            invalid(s);
        }
        assert!(invalid("/%postname%/").contains("unknown token"));
        assert!(invalid("/%year%/").contains("%slug%"));
        for s in [
            "/%slug%/",
            "/%year%/%month%/%slug%/",
            "/%category%/%slug%/",
            "/%parent_path%/%slug%/",
            "/blog/%slug%/",
        ] {
            assert_eq!(Structure::parse(s).unwrap().as_str(), s);
        }
        assert!(
            Structure::parse("/%category%/%slug%/")
                .unwrap()
                .uses_category()
        );
        assert!(!Structure::parse("/%slug%/").unwrap().uses_category());
        assert!(PermalinkRules::from_options(Some("/x/"), None).is_err());
        assert!(PermalinkRules::from_options(None, Some("/%nope%/%slug%/")).is_err());
    }

    #[test]
    fn each_structure() {
        let rules = PermalinkRules::default();
        assert_eq!(rules.path(&input(item("post", "hi"))), "/hi/");
        assert_eq!(rules.path(&input(item("media", "logo"))), "/media/logo/");
        assert_eq!(rules.path(&input(item("page", "top"))), "/top/");
        let mut page = input(item("page", "leaf"));
        page.ancestors = vec![
            (ContentId::new(), "a".into()),
            (ContentId::new(), "b".into()),
        ];
        assert_eq!(rules.path(&page), "/a/b/leaf/");
        let mut book = input(item("book", "ch"));
        book.ancestors = vec![(ContentId::new(), "vol".into())];
        assert_eq!(rules.path(&book), "/book/vol/ch/");

        let dated = PermalinkRules::from_options(Some("/%year%/%month%/%slug%/"), None).unwrap();
        assert_eq!(dated.path(&input(item("post", "hi"))), "/2025/11/hi/");
        let mut unpublished = item("post", "hi");
        unpublished.published_at = None;
        assert_eq!(dated.path(&input(unpublished)), "/2024/03/hi/");

        let by_cat =
            PermalinkRules::from_options(Some("/blog/%category%/%slug%/"), Some("/p/%slug%/"))
                .unwrap();
        let mut post = input(item("post", "hi"));
        assert_eq!(by_cat.path(&post), "/blog/uncategorized/hi/");
        post.primary_term = Some(category("category", "news"));
        assert_eq!(by_cat.path(&post), "/blog/news/hi/");
        assert_eq!(by_cat.path(&page), "/p/leaf/");
    }

    #[test]
    fn segments_are_percent_encoded() {
        let rules = PermalinkRules::default();
        assert_eq!(
            rules.path(&input(item("post", "zażółć"))),
            "/za%C5%BC%C3%B3%C5%82%C4%87/"
        );
        assert_eq!(
            rules.path(&input(item("post", "a&b+c=d:e@f!.g_h~i"))),
            "/a%26b%2Bc%3Dd%3Ae%40f%21.g_h~i/"
        );
        let mut page = input(item("page", "x"));
        page.ancestors = vec![(ContentId::new(), "日本".into())];
        assert_eq!(rules.path(&page), "/%E6%97%A5%E6%9C%AC/x/");
    }

    #[test]
    fn primary_term_is_first_category() {
        let terms = vec![
            category("tag", "t"),
            category("category", "b"),
            category("category", "a"),
        ];
        assert_eq!(primary_term(terms).unwrap().slug, "b");
        assert_eq!(primary_term(vec![category("tag", "t")]), None);
    }
}
