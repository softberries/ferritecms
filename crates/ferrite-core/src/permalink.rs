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
//!
//! Validation: a literal first segment may not be a [`slug::RESERVED`] word (checked by
//! [`Structure::parse`]) or a content type name (checked by
//! [`PermalinkRules::check_types`], since the parser does not know the registered
//! types; it also rejects taxonomy names, router words and years; whoever builds the
//! rules from options calls it with `types().list()` and `taxonomies().list()`). The
//! page structure must contain `%parent_path%`, or child pages would collide.
//!
//! Root namespace: posts under `/%slug%/` and top-level pages under
//! `/%parent_path%/%slug%/` both live at `/<slug>/`. Core keeps them apart when it
//! allocates slugs ([`Permalinks::root_namespace_peers`]), and a top-level slug may
//! never be a type name (the `/{type}/` prefixes). The router (`ferrite-http`) additionally
//! resolves a single segment as a page before a post, as a safety net for collisions
//! that predate these rules or raced past them.

use ferrite_domain::{ContentId, ContentItem, Term};
use ferrite_store::Repos;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::{CoreError, MAX_PARENT_DEPTH, slug};

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

    /// The other content types whose top-level items get paths of the same shape as
    /// top-level items of `kind` (so their slugs must not collide). Default: none.
    fn root_namespace_peers(&self, kind: &str) -> Vec<String> {
        let _ = kind;
        Vec::new()
    }
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

impl Segment {
    fn literal(&self) -> Option<&str> {
        match self {
            Segment::Literal(l) => Some(l),
            _ => None,
        }
    }
}

/// A parsed permalink structure such as `/%year%/%month%/%slug%/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    source: String,
    segments: Vec<Segment>,
}

impl Structure {
    /// Parses `s`: it must start and end with `/` and contain `%slug%`; each segment is
    /// a single known token or a literal of lowercase unreserved characters (the router
    /// canonicalises paths to lowercase, ruling R13), and a literal first segment may
    /// not be a [`slug::RESERVED`] word. Anything else is `Invalid`.
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
                lit if lit.bytes().any(|b| b.is_ascii_uppercase()) => {
                    return Err(invalid(&format!(
                        "literal {lit:?} has uppercase letters (paths are lowercase)"
                    )));
                }
                lit if lit.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || matches!(b, b'-' | b'.' | b'_' | b'~')
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
        if let Some(first) = segments.first().and_then(Segment::literal)
            && slug::is_reserved(&first.to_ascii_lowercase())
        {
            return Err(invalid(&format!("first segment {first:?} is reserved")));
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

    /// Whether top-level items get `/<slug>/` (`%parent_path%` collapses).
    fn is_root_slug(&self) -> bool {
        self.segments
            .iter()
            .filter(|s| **s != Segment::ParentPath)
            .eq([Segment::Slug].iter())
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
///
/// `%year%` and `%month%` are taken in **UTC** by design (ruling R4): a permalink must
/// not move when `site.timezone` changes. Near a month boundary the URL's month can
/// therefore differ from the local publish date a theme shows.
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
    /// default). An invalid structure is `Invalid`, as is a page structure without
    /// `%parent_path%` (pages are hierarchical).
    pub fn from_options(post: Option<&str>, page: Option<&str>) -> Result<Self, CoreError> {
        let page = Structure::parse(page.unwrap_or(DEFAULT_PAGE))?;
        if !page.segments.contains(&Segment::ParentPath) {
            return Err(CoreError::Invalid(format!(
                "permalink structure {:?}: pages need %parent_path%",
                page.source
            )));
        }
        Ok(Self {
            post: Structure::parse(post.unwrap_or(DEFAULT_POST))?,
            page,
            other: Self::default().other,
        })
    }

    /// `Invalid` when the post or page structure starts with a literal that the router
    /// resolves to something else (rulings R3, R13): a content type name in
    /// `type_names` (its `/{type}/` paths), a taxonomy name in `taxonomy_names` (its
    /// `/{taxonomy}/{slug}/` archives), a [`slug::ROUTE_WORDS`] word (author archives,
    /// pagination, search) or four digits (a `/{yyyy}/` date archive). Compared
    /// case-insensitively. Whoever builds the rules from options calls it at startup
    /// with `types().list()` and `taxonomies().list()`.
    pub fn check_types<S: AsRef<str>, T: AsRef<str>>(
        &self,
        type_names: &[S],
        taxonomy_names: &[T],
    ) -> Result<(), CoreError> {
        for structure in [&self.post, &self.page] {
            let Some(first) = structure.segments.first().and_then(Segment::literal) else {
                continue;
            };
            let matches = |name: &str| name.eq_ignore_ascii_case(first);
            let what = if type_names.iter().any(|t| matches(t.as_ref())) {
                "a content type"
            } else if taxonomy_names.iter().any(|t| matches(t.as_ref())) {
                "a taxonomy"
            } else if slug::ROUTE_WORDS.iter().any(|w| matches(w)) {
                "a route word"
            } else if slug::is_year(first) {
                "a year archive"
            } else {
                continue;
            };
            return Err(CoreError::Invalid(format!(
                "permalink structure {:?}: first segment {first:?} is {what}",
                structure.source
            )));
        }
        Ok(())
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

    /// Posts and pages are peers when both put top-level items at `/<slug>/` (the
    /// defaults do); other types live under their `/{type}/` prefix.
    fn root_namespace_peers(&self, kind: &str) -> Vec<String> {
        let shared = self.post.is_root_slug() && self.page.is_root_slug();
        match kind {
            "post" if shared => vec!["page".to_owned()],
            "page" if shared => vec!["post".to_owned()],
            _ => Vec::new(),
        }
    }
}

/// Percent-encodes one path segment the way permalinks do (UTF-8, uppercase hex; only
/// RFC 3986 unreserved characters are kept). The router uses it to build canonical
/// paths, so request paths and stored permalinks compare byte for byte.
pub fn encode_segment(raw: &str) -> String {
    utf8_percent_encode(raw, SEGMENT).to_string()
}

/// Public path of a term archive: `/{taxonomy}/{slug}/`, both segments percent-encoded
/// like item paths (so `category` and `tag` terms live under `/category/` and `/tag/`).
pub fn term_path(taxonomy: &str, slug: &str) -> String {
    format!(
        "/{}/{}/",
        utf8_percent_encode(taxonomy, SEGMENT),
        utf8_percent_encode(slug, SEGMENT)
    )
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
        if chain.len() >= MAX_PARENT_DEPTH {
            return Err(CoreError::Invalid(format!(
                "parent chain deeper than {MAX_PARENT_DEPTH}"
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
            "/admin/%slug%/",
            "/API/%slug%/",
            "/_media/%slug%/",
            "/Blog/%slug%/",
            "/blog/X/%slug%/",
        ] {
            invalid(s);
        }
        assert!(invalid("/%postname%/").contains("unknown token"));
        assert!(invalid("/%year%/").contains("%slug%"));
        assert!(invalid("/Blog/%slug%/").contains("uppercase"));
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
        // Reserved words only matter as the first segment.
        assert!(Structure::parse("/x/admin/%slug%/").is_ok());
        // Pages are hierarchical: their structure needs %parent_path%.
        assert!(PermalinkRules::from_options(None, Some("/%slug%/")).is_err());
        assert!(PermalinkRules::from_options(None, Some("/p/%parent_path%/%slug%/")).is_ok());
    }

    #[test]
    fn type_names_as_first_segment() {
        let types = ["post", "page", "media", "book"];
        let taxes = ["category", "tag"];
        assert!(
            PermalinkRules::default()
                .check_types(&types, &taxes)
                .is_ok()
        );
        let shadow = PermalinkRules::from_options(Some("/media/%slug%/"), None).unwrap();
        assert!(matches!(
            shadow.check_types(&types, &taxes),
            Err(CoreError::Invalid(_))
        ));
        let page = PermalinkRules::from_options(None, Some("/book/%parent_path%/%slug%/")).unwrap();
        assert!(page.check_types(&types, &taxes).is_err());
        let fine = PermalinkRules::from_options(Some("/blog/%slug%/"), None).unwrap();
        assert!(fine.check_types(&types, &taxes).is_ok());
    }

    #[test]
    fn router_words_as_first_segment() {
        let types = ["post", "page"];
        let taxes = ["category", "tag", "genre"];
        for (post, why) in [
            ("/category/%slug%/", "taxonomy"),
            ("/genre/%slug%/", "taxonomy"),
            ("/author/%slug%/", "route word"),
            // `page` is also a content type, which is reported first.
            ("/page/%slug%/", "content type"),
            ("/search/%slug%/", "route word"),
            ("/2024/%slug%/", "year archive"),
        ] {
            let r = PermalinkRules::from_options(Some(post), None).unwrap();
            match r.check_types(&types, &taxes) {
                Err(CoreError::Invalid(m)) => assert!(m.contains(why), "{post}: {m}"),
                other => panic!("{post}: {other:?}"),
            }
        }
        let page = PermalinkRules::from_options(None, Some("/tag/%parent_path%/%slug%/")).unwrap();
        assert!(page.check_types(&types, &taxes).is_err());
        // Not a first literal, or not a router word: fine.
        for post in ["/news/2024/%slug%/", "/12345/%slug%/", "/%year%/%slug%/"] {
            let r = PermalinkRules::from_options(Some(post), None).unwrap();
            assert!(r.check_types(&types, &taxes).is_ok(), "{post}");
        }
    }

    #[test]
    fn root_namespace_peers() {
        let rules = PermalinkRules::default();
        assert_eq!(rules.root_namespace_peers("post"), ["page"]);
        assert_eq!(rules.root_namespace_peers("page"), ["post"]);
        assert!(rules.root_namespace_peers("media").is_empty());
        for post in [
            "/%year%/%month%/%slug%/",
            "/%category%/%slug%/",
            "/blog/%slug%/",
        ] {
            let r = PermalinkRules::from_options(Some(post), None).unwrap();
            assert!(r.root_namespace_peers("post").is_empty(), "{post}");
            assert!(r.root_namespace_peers("page").is_empty(), "{post}");
        }
        let r = PermalinkRules::from_options(None, Some("/p/%parent_path%/%slug%/")).unwrap();
        assert!(r.root_namespace_peers("post").is_empty());
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

        let by_cat = PermalinkRules::from_options(
            Some("/blog/%category%/%slug%/"),
            Some("/p/%parent_path%/%slug%/"),
        )
        .unwrap();
        let mut post = input(item("post", "hi"));
        assert_eq!(by_cat.path(&post), "/blog/uncategorized/hi/");
        post.primary_term = Some(category("category", "news"));
        assert_eq!(by_cat.path(&post), "/blog/news/hi/");
        assert_eq!(by_cat.path(&page), "/p/a/b/leaf/");
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
        assert_eq!(encode_segment("zażółć"), "za%C5%BC%C3%B3%C5%82%C4%87");
        assert_eq!(encode_segment("a b/c~"), "a%20b%2Fc~");
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
