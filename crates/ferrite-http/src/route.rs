//! The URL router: a pure function from a request path and query to a [`Route`].
//!
//! [`resolve`] does no I/O. The redirect table is checked by the handler *before* it
//! calls `resolve`; then static routes win over the permalink matcher, and anything
//! left is [`Route::NotFound`].
//!
//! # Canonical form
//!
//! Every path segment is percent-decoded exactly once and re-encoded with core's
//! [`encode_segment`] (UTF-8, uppercase hex, only RFC 3986 unreserved characters
//! kept), so a request compares byte for byte with stored permalinks. Fixed words
//! (`page`, `feed`, `author`, taxonomy and type names, structure literals, file names)
//! match ASCII case-insensitively and are written as defined; slug positions (item,
//! ancestor, term and `%category%` slugs) are lowercased with Unicode `to_lowercase`,
//! as `slug::validate` stores them; author logins keep their case. There is no Unicode
//! normalisation: a request in NFD for a slug stored in NFC (or the reverse) is a
//! different path and 404s. Every route ends in `/` except the files `/robots.txt`,
//! `/sitemap.xml` and `/sitemap-<kind>-<n>.xml`. When the request path is not its
//! route's canonical path the result is [`Route::Canonical`] (a 301 target). The raw
//! query is appended with every byte outside visible ASCII (`0x21..=0x7E`) and `#`
//! percent-encoded, so a target is always visible ASCII; a canonical path longer than
//! [`MAX_PATH_BYTES`] is 404 instead. `/_media/…`, `/_theme/…`, `/healthz` and
//! `/readyz` must be exact and are never redirected.
//!
//! # Rejected (404)
//!
//! Paths over [`MAX_PATH_BYTES`] bytes or [`MAX_SEGMENTS`] segments; `.` and `..`
//! segments, raw or decoded; empty interior segments (`//x` and `/a//b/` are 404, not
//! collapsed into a redirect, so no alternate spelling of a URL exists); malformed
//! escapes, invalid UTF-8, control characters (`%00` included) and `\` anywhere; `/`
//! decoded from `%2F` inside a segment.
//!
//! # Static routes
//!
//! `/`, `/feed/`, `/robots.txt`, `/sitemap.xml`, `/sitemap-<kind>-<n>.xml`,
//! `/{taxonomy}/{slug}/` (the shape of `term_path`), `/{taxonomy}/{slug}/feed/`,
//! `/author/{login}/`, `/{yyyy}/`, `/{yyyy}/{mm}/`, `/{type}/` for types with an
//! archive, `/_media/{hash}/{name}`, `/_theme/{hash}/{path…}`, `/healthz`, `/readyz`.
//! Listings (front, search, archive, term, author, date) take a `/page/N/` suffix with
//! `N` in `2..=1000` written without a leading zero; `/page/1/` redirects to the path
//! without it, any other `N` is 404 (the 5000-row skip cap of ruling R6 is enforced by
//! `SiteReader`). `?s=` on `/` or `/page/N/` is a search; an empty `s` is the front page.
//!
//! Static routes win over content. Core keeps them apart (ruling R13): top-level slugs
//! may not be taxonomy or type names, `author`, `page`, `search` or four digits, so
//! `/category/news/` or `/2024/` never hides an item. A trailing `page/<x>` pair is
//! always read as pagination (a child page named `page` is still shadowed). Date
//! archives exist under every post structure; with `/%year%/%month%/%slug%/` the slug
//! segment tells a post from the month archive.
//!
//! # Content
//!
//! Whatever is left goes to the permalink matcher, which may find several meanings for
//! one path; [`Route::Content`] lists them in order and the handler tries each with
//! `SiteReader::single`, the first hit winning. Pages come first (ruling R3: page
//! before post on `/<slug>/`, a safety net for collisions core's slug allocation did
//! not prevent), then posts, then `/{type}/…` items of the other types.
//!
//! **Handler obligation:** a hit counts only when the item's computed permalink (its
//! `url`) equals the request path. For [`Candidate::PostByStructure`] this is what
//! makes a wrong year, month or category a 404 instead of the post under another URL;
//! for the other candidates it is already true by construction and the check is cheap.
//! Only candidates whose canonical path equals the request path are listed; when none
//! does, the result is a redirect to the first candidate's canonical path.

use ferrite_core::FeedSource;
use ferrite_core::permalink::{PermalinkRules, Segment, Structure, encode_segment, term_path};

mod path;
mod query;
#[cfg(test)]
mod tests;

pub use query::NormalQuery;

/// Longest request path accepted, in raw bytes.
pub const MAX_PATH_BYTES: usize = 2048;
/// Most path segments accepted.
pub const MAX_SEGMENTS: usize = 32;
/// Highest `/page/N/` (and sitemap file number) the router accepts.
pub const MAX_PAGE: u32 = 1000;

/// A public content type, as the router needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    pub hierarchical: bool,
    pub has_archive: bool,
}

/// What [`resolve`] matches against: the permalink structures, the public content
/// types (those with public singles; `post` and `page` use their structures, every
/// other type `/{type}/…`) and the taxonomy names.
#[derive(Debug, Clone)]
pub struct RouteTable {
    post: Structure,
    page: Structure,
    types: Vec<TypeInfo>,
    taxonomies: Vec<String>,
}

impl RouteTable {
    pub fn new(rules: &PermalinkRules, public_types: &[TypeInfo], taxonomies: &[String]) -> Self {
        Self {
            post: rules.post().clone(),
            page: rules.page().clone(),
            types: public_types.to_vec(),
            taxonomies: taxonomies.to_vec(),
        }
    }

    fn type_named(&self, name: &str) -> Option<&TypeInfo> {
        self.types
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    fn taxonomy_named(&self, name: &str) -> Option<&str> {
        self.taxonomies
            .iter()
            .find(|t| t.eq_ignore_ascii_case(name))
            .map(String::as_str)
    }
}

/// One possible meaning of a content path. Slugs are decoded (not percent-encoded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidate {
    /// `SiteReader::single(kind, parts)`: ancestor slugs then the slug.
    Single { kind: String, parts: Vec<String> },
    /// A post under a structure with `%year%`, `%month%` or `%category%`: look it up
    /// with `single("post", [slug])`, then accept it only if its `url` equals the
    /// request path (the other fields are what the path claimed, for logging).
    PostByStructure {
        slug: String,
        year: Option<i32>,
        month: Option<u32>,
        category: Option<String>,
    },
}

/// One sitemap file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SitemapPart {
    /// `/sitemap.xml`.
    Index,
    /// `/sitemap-<kind>-<n>.xml`, `n` from 1.
    Kind { kind: String, n: u32 },
}

/// What a request path means. `page` is 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// 301 to this path (plus the original query).
    Canonical(String),
    Health,
    Ready,
    Robots,
    Sitemap(SitemapPart),
    /// `name` decoded.
    Media {
        hash: String,
        name: String,
    },
    /// `path` decoded, segments joined by `/`.
    Theme {
        hash: String,
        path: String,
    },
    Front {
        page: u32,
    },
    Search {
        q: String,
        page: u32,
    },
    Feed(FeedSource),
    Archive {
        kind: String,
        page: u32,
    },
    Term {
        taxonomy: String,
        slug: String,
        page: u32,
    },
    Author {
        login: String,
        page: u32,
    },
    Date {
        year: i32,
        month: Option<u32>,
        page: u32,
    },
    /// Try in order; see the module docs for the handler's permalink check.
    Content(Vec<Candidate>),
    NotFound,
}

/// Fixed words that are never content and never redirected.
const EXACT_ONLY: &[&str] = &["_media", "_theme", "healthz", "readyz"];

/// Resolves `raw_path` (as received, still percent-encoded) and `raw_query` (without
/// `?`). Pure.
pub fn resolve(raw_path: &str, raw_query: Option<&str>, table: &RouteTable) -> Route {
    if raw_path.len() > MAX_PATH_BYTES {
        return Route::NotFound;
    }
    match raw_path {
        "/healthz" => return Route::Health,
        "/readyz" => return Route::Ready,
        _ => {}
    }
    if let Some(rest) = raw_path.strip_prefix("/_media/") {
        return media(rest).unwrap_or(Route::NotFound);
    }
    if let Some(rest) = raw_path.strip_prefix("/_theme/") {
        return theme(rest).unwrap_or(Route::NotFound);
    }
    let Some(segs) = path::segments(raw_path) else {
        return Route::NotFound;
    };
    if segs
        .first()
        .is_some_and(|s| EXACT_ONLY.iter().any(|w| s.eq_ignore_ascii_case(w)))
    {
        return Route::NotFound;
    }
    let query = NormalQuery::parse(raw_query);
    let redirect = |to: &str| {
        if to.len() > MAX_PATH_BYTES {
            return Route::NotFound;
        }
        match raw_query {
            Some(q) if !q.is_empty() => Route::Canonical(format!("{to}?{}", visible_ascii(q))),
            _ => Route::Canonical(to.to_owned()),
        }
    };
    match matched(&segs, &query, table) {
        Matched::One(route, canonical) if canonical == raw_path => route,
        Matched::One(_, canonical) => redirect(&canonical),
        Matched::Content(found) => {
            let Some((_, first)) = found.first() else {
                return Route::NotFound;
            };
            let exact: Vec<Candidate> = found
                .iter()
                .filter(|(_, c)| c == raw_path)
                .map(|(cand, _)| cand.clone())
                .collect();
            if exact.is_empty() {
                redirect(first)
            } else {
                Route::Content(exact)
            }
        }
        Matched::None => Route::NotFound,
    }
}

/// A match and the canonical path it implies.
enum Matched {
    One(Route, String),
    Content(Vec<(Candidate, String)>),
    None,
}

fn matched(segs: &[String], query: &NormalQuery, table: &RouteTable) -> Matched {
    if let [one] = segs
        && let Some(found) = file(one, table)
    {
        return found;
    }
    if let [base @ .., word, n] = segs
        && word.eq_ignore_ascii_case("page")
    {
        let Some(page) = path::number(n, MAX_PAGE) else {
            return Matched::None;
        };
        return match listing(base, page, query, table) {
            Some((route, canonical)) => Matched::One(route, paged(canonical, page)),
            None => Matched::None,
        };
    }
    if let Some(found) = feed(segs, table) {
        return found;
    }
    if let Some((route, canonical)) = listing(segs, 1, query, table) {
        return Matched::One(route, canonical);
    }
    let found = content(segs, table);
    if found.is_empty() {
        Matched::None
    } else {
        Matched::Content(found)
    }
}

fn paged(base: String, page: u32) -> String {
    if page == 1 {
        base
    } else {
        format!("{base}page/{page}/")
    }
}

/// `/robots.txt`, `/sitemap.xml`, `/sitemap-<kind>-<n>.xml` (with or without a trailing
/// slash; the canonical form has none). `None`: not a file name.
fn file(seg: &str, table: &RouteTable) -> Option<Matched> {
    let lower = seg.to_ascii_lowercase();
    let one = |route, path: &str| Some(Matched::One(route, path.to_owned()));
    match lower.as_str() {
        "robots.txt" => return one(Route::Robots, "/robots.txt"),
        "sitemap.xml" => return one(Route::Sitemap(SitemapPart::Index), "/sitemap.xml"),
        _ => {}
    }
    let inner = lower.strip_prefix("sitemap-")?.strip_suffix(".xml")?;
    let part = inner.rsplit_once('-').and_then(|(kind, n)| {
        let ty = table.type_named(kind)?;
        let n = path::number(n, MAX_PAGE)?;
        Some((ty.name.clone(), n))
    });
    Some(match part {
        Some((kind, n)) => {
            let path = format!("/sitemap-{}-{n}.xml", encode_segment(&kind));
            Matched::One(Route::Sitemap(SitemapPart::Kind { kind, n }), path)
        }
        None => Matched::None,
    })
}

/// `/feed/` and `/{taxonomy}/{slug}/feed/`.
fn feed(segs: &[String], table: &RouteTable) -> Option<Matched> {
    match segs {
        [word] if word.eq_ignore_ascii_case("feed") => Some(Matched::One(
            Route::Feed(FeedSource::Kind("post".into())),
            "/feed/".into(),
        )),
        [tax, slug, word] if word.eq_ignore_ascii_case("feed") => {
            let taxonomy = table.taxonomy_named(tax)?.to_owned();
            let slug = slug.to_lowercase();
            let canonical = format!("{}feed/", term_path(&taxonomy, &slug));
            Some(Matched::One(
                Route::Feed(FeedSource::Term { taxonomy, slug }),
                canonical,
            ))
        }
        _ => None,
    }
}

/// The paginated listings at `segs` (the path without `/page/N/`) and their canonical
/// path for page 1.
fn listing(
    segs: &[String],
    page: u32,
    query: &NormalQuery,
    table: &RouteTable,
) -> Option<(Route, String)> {
    let route = match segs {
        [] => match &query.s {
            Some(q) => Route::Search { q: q.clone(), page },
            None => Route::Front { page },
        },
        [y] if path::digits(y, 4) => Route::Date {
            year: y.parse().ok()?,
            month: None,
            page,
        },
        [y, m] if path::digits(y, 4) && month(m).is_some() => Route::Date {
            year: y.parse().ok()?,
            month: month(m),
            page,
        },
        [word, login] if word.eq_ignore_ascii_case("author") => Route::Author {
            login: login.clone(),
            page,
        },
        [tax, slug] => Route::Term {
            taxonomy: table.taxonomy_named(tax)?.to_owned(),
            slug: slug.to_lowercase(),
            page,
        },
        [kind] => {
            let ty = table.type_named(kind).filter(|t| t.has_archive)?;
            Route::Archive {
                kind: ty.name.clone(),
                page,
            }
        }
        _ => return None,
    };
    let canonical = match &route {
        Route::Date { year, month, .. } => match month {
            Some(m) => format!("/{year:04}/{m:02}/"),
            None => format!("/{year:04}/"),
        },
        Route::Author { login, .. } => format!("/author/{}/", encode_segment(login)),
        Route::Term { taxonomy, slug, .. } => term_path(taxonomy, slug),
        Route::Archive { kind, .. } => format!("/{}/", encode_segment(kind)),
        _ => "/".to_owned(),
    };
    Some((route, canonical))
}

/// A two-digit month `01..=12`.
fn month(s: &str) -> Option<u32> {
    if !path::digits(s, 2) {
        return None;
    }
    s.parse().ok().filter(|m| (1..=12).contains(m))
}

/// The permalink matcher: pages, then posts, then `/{type}/…` items.
fn content(segs: &[String], table: &RouteTable) -> Vec<(Candidate, String)> {
    let mut out = Vec::new();
    if let Some(m) = match_structure(&table.page, segs, true) {
        let mut parts = m.parents;
        parts.push(m.slug);
        out.push((
            Candidate::Single {
                kind: "page".into(),
                parts,
            },
            m.canonical,
        ));
    }
    if let Some(m) = match_structure(&table.post, segs, false) {
        let tokens = m.year.is_some() || m.month.is_some() || m.category.is_some();
        let candidate = if tokens {
            Candidate::PostByStructure {
                slug: m.slug,
                year: m.year,
                month: m.month,
                category: m.category,
            }
        } else {
            Candidate::Single {
                kind: "post".into(),
                parts: vec![m.slug],
            }
        };
        out.push((candidate, m.canonical));
    }
    if let [prefix, rest @ ..] = segs
        && !rest.is_empty()
        && let Some(ty) = table.type_named(prefix)
        && ty.name != "post"
        && ty.name != "page"
        && (ty.hierarchical || rest.len() == 1)
    {
        let parts: Vec<String> = rest.iter().map(|s| s.to_lowercase()).collect();
        let mut canonical = format!("/{}/", encode_segment(&ty.name));
        for p in &parts {
            push(&mut canonical, p);
        }
        out.push((
            Candidate::Single {
                kind: ty.name.clone(),
                parts,
            },
            canonical,
        ));
    }
    out
}

/// A path matched against a permalink structure.
struct StructureMatch {
    parents: Vec<String>,
    slug: String,
    year: Option<i32>,
    month: Option<u32>,
    category: Option<String>,
    canonical: String,
}

/// Matches `segs` against `structure`. `%parent_path%` takes the ancestor slugs (none
/// unless `hierarchical`); when it appears more than once every copy must be equal, as
/// expansion writes the same ancestors at each.
fn match_structure(
    structure: &Structure,
    segs: &[String],
    hierarchical: bool,
) -> Option<StructureMatch> {
    let tokens = structure.segments();
    let copies = tokens.iter().filter(|t| **t == Segment::ParentPath).count();
    let fixed = tokens.len() - copies;
    let extra = segs.len().checked_sub(fixed)?;
    let depth = match copies {
        0 if extra == 0 => 0,
        0 => return None,
        c if extra % c == 0 => extra / c,
        _ => return None,
    };
    if depth > 0 && !hierarchical {
        return None;
    }
    let mut m = StructureMatch {
        parents: Vec::new(),
        slug: String::new(),
        year: None,
        month: None,
        category: None,
        canonical: String::from("/"),
    };
    let mut rest = segs;
    let mut parents_seen = false;
    for token in tokens {
        if *token == Segment::ParentPath {
            let (chunk, tail) = rest.split_at(depth);
            rest = tail;
            let chunk: Vec<String> = chunk.iter().map(|s| s.to_lowercase()).collect();
            if parents_seen && chunk != m.parents {
                return None;
            }
            for p in &chunk {
                push(&mut m.canonical, p);
            }
            parents_seen = true;
            m.parents = chunk;
            continue;
        }
        let (seg, tail) = rest.split_first()?;
        rest = tail;
        match token {
            Segment::Literal(lit) => {
                if !seg.eq_ignore_ascii_case(lit) {
                    return None;
                }
                push(&mut m.canonical, lit);
            }
            Segment::Slug => {
                m.slug = seg.to_lowercase();
                push(&mut m.canonical, &m.slug);
            }
            Segment::Category => {
                let c = seg.to_lowercase();
                push(&mut m.canonical, &c);
                m.category = Some(c);
            }
            Segment::Year => {
                if !path::digits(seg, 4) {
                    return None;
                }
                m.year = Some(seg.parse().ok()?);
                push(&mut m.canonical, seg);
            }
            Segment::Month => {
                m.month = Some(month(seg)?);
                push(&mut m.canonical, seg);
            }
            Segment::ParentPath => {}
        }
    }
    Some(m)
}

/// `/_media/{hash}/{name}`: `hash` is 64 lowercase hex digits, `name` one segment.
fn media(rest: &str) -> Option<Route> {
    let (hash, name) = rest.split_once('/')?;
    let hex64 = hash.len() == 64 && is_lower_hex(hash);
    if !hex64 || name.contains('/') {
        return None;
    }
    Some(Route::Media {
        hash: hash.to_owned(),
        name: path::decode_segment(name)?,
    })
}

/// `/_theme/{hash}/{path…}`: `hash` is 1 to 64 lowercase hex digits, `path` one or more
/// segments.
fn theme(rest: &str) -> Option<Route> {
    let (hash, file) = rest.split_once('/')?;
    if hash.is_empty() || hash.len() > 64 || !is_lower_hex(hash) {
        return None;
    }
    // `_theme` and the hash count towards the segment limit.
    let parts = path::decode_all(file.split('/'))?;
    if parts.len() + 2 > MAX_SEGMENTS {
        return None;
    }
    Some(Route::Theme {
        hash: hash.to_owned(),
        path: parts.join("/"),
    })
}

/// `q` with every byte outside `0x21..=0x7E`, and `#`, percent-encoded (uppercase hex);
/// existing escapes are kept as they are.
fn visible_ascii(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for b in q.bytes() {
        if (0x21..=0x7E).contains(&b) && b != b'#' {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Appends `s`, encoded, and a `/`.
fn push(canonical: &mut String, s: &str) {
    canonical.push_str(&encode_segment(s));
    canonical.push('/');
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
