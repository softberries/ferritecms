//! The public read facade: everything the public site renders comes through
//! [`SiteReader`].
//!
//! **Published-only by construction.** Every public method builds its own
//! `ContentQuery`/`SearchQuery` and forces `status = published` and
//! `published_at <= now` (the clock is injected). Items fetched another way (by slug,
//! by id, through the search index) are re-checked with the same predicate in Rust, so
//! a lagging search index or a future-dated published item never shows. Drafts are
//! reachable only through [`SiteReader::preview`], which is gated by `can_read` and
//! marked not cacheable. Media items are never listed or rendered as singles; they
//! appear only as featured or inline images, and only while they are published too.
//!
//! **Dependency keys** (renderer side of the contract in [`crate::events`]): every
//! result carries `post:<id>` for each item shown (including ancestors used in its
//! path and media items shown in it), `term:<id>` for each term shown (and the term of
//! a term archive), `archive:<kind>` for listings, feeds, search, related and sitemaps,
//! and `option:<key>` for each option read.
//!
//! **Batch loading.** Pages are hydrated with one query per repository (terms, users,
//! media items, media rows; plus one per ancestor level for hierarchical types), never
//! one per item, and empty id sets skip the query. A listing page therefore costs a
//! constant number of store calls: the `reading.per_page` option, the page, its count
//! (skipped when page 1 is the only page; taken before the walk on later pages),
//! terms, users, and, when any item shows media, media items and media rows: at
//! most 7.
//!
//! **Offset paging.** The store pages by cursor; `/page/N/` first counts the matches
//! (a page past the last one stops there, loading no rows), then skips `N - 1` pages by
//! walking cursors in steps of up to 200 rows (`ceil(skip / 200)` extra round trips that
//! load full rows). Requests that would skip more than [`MAX_SKIP`] rows return `None`
//! (the router answers 404), which caps that cost (ruling R6). Fine for v1 archive
//! sizes; a store-side offset or keyset-by-page index is the upgrade path.
//!
//! **No existence oracle** (ruling R7): author and term archives and term feeds with
//! no public items return `None`, the same as an unknown user or term, so drafts do
//! not reveal that a login or tag exists.
//!
//! **Public view only** (ruling R8): templates get a [`PublicItem`], never the row:
//! no body source, status, version or scheduling, and `fields` stripped of references
//! to non-public content.
//!
//! **Hidden ancestors** (accepted consequence of the WordPress-style rule in
//! [`SiteReader::single`]): a published child under a draft page keeps its full path,
//! so its URL (in pages, listings and the sitemap) contains the draft ancestor's slug.

mod hydrate;
mod model;

use std::{collections::HashMap, sync::Arc};

use chrono::NaiveDate;
use ferrite_domain::{
    ArtifactHash, ContentId, ContentItem, ContentQuery, ContentStatus, MAX_PAGE, SearchQuery,
    Taxonomy, TermId, Timestamp,
};
use ferrite_store::{MAX_CONTENT_KINDS, MAX_ID_FILTER, MAX_SEARCH_KINDS, Repos, Store};

use crate::{
    Actor, Capability, CoreError, MAX_PARENT_DEPTH,
    actor::can_read,
    comments::{self, CommentThread},
    events::{archive_key, option_key, post_key, term_key, user_key},
    permalink::{PermalinkInput, Permalinks, primary_term},
};
use hydrate::{Hydrator, Nodes, chain, unique};
pub use model::{
    DepKeys, FeedSource, FrontPage, ItemLink, Listing, MediaFile, MediaPublic, MediaVariant,
    PageItem, Previewed, PublicItem, SiteRead, SitemapEntry, SitemapPage, TermPublic, UserPublic,
};

/// Maps an artifact hash and a file name to a public URL (`MediaStore::public_url`).
pub type MediaUrlFn = Arc<dyn Fn(&ArtifactHash, &str) -> String + Send + Sync>;
/// The current time; injected so tests control "now".
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// Content type of media items.
pub(crate) const MEDIA_KIND: &str = "media";
/// Content type listed on the front page and in date and author archives.
const POST_KIND: &str = "post";
/// Option: items per listing page (integer, clamped to 1..=100; default 10).
pub const PER_PAGE_OPTION: &str = "reading.per_page";
/// Option: id (UUID string) of the item shown at the site root instead of the posts.
pub const FRONT_PAGE_OPTION: &str = "reading.front_page";
const DEFAULT_PER_PAGE: u32 = 10;
const MAX_PER_PAGE: i64 = 100;
/// Most rows a listing may skip to reach its page (`(page - 1) * per_page`); deeper
/// pages are `None`.
pub const MAX_SKIP: u64 = 5000;
/// Items per feed.
pub const FEED_LEN: u32 = 20;
/// URLs per sitemap file.
pub const SITEMAP_FILE_LEN: usize = 50_000;
/// Most media rows [`SiteReader::media_file`] considers for one file: more media
/// items sharing one stored file than this is not a real library (the same upload
/// would have to be added 100 times), and the first public one answers anyway.
pub const MAX_MEDIA_ROWS: u32 = 100;
/// Most items [`SiteReader::related`] returns.
pub const MAX_RELATED: usize = 10;
/// Most items [`SiteReader::term_items`] returns (the theme `query()` helper's cap).
pub const MAX_QUERY_LIMIT: u32 = 50;
/// Most items [`SiteReader::latest`] returns.
pub const MAX_LATEST: u32 = 100;
/// Most search matches paged through; a search `total` never exceeds it (ruling R9:
/// bounds what one anonymous request can make the store load).
pub const SEARCH_CAP: usize = 200;

/// Whether `item` is publicly visible at `now`: published, with a publish time not in
/// the future. The one definition of "public": `SiteReader`'s re-checks and
/// [`can_read`] (admin reads, previews) both use it.
pub(crate) fn is_public(item: &ContentItem, now: Timestamp) -> bool {
    item.status == ContentStatus::Published && item.published_at.is_some_and(|t| t <= now)
}

/// The published-only read facade; see the module docs.
pub struct SiteReader {
    store: Arc<dyn Store>,
    permalinks: Arc<dyn Permalinks>,
    /// URLs per sitemap file ([`SITEMAP_FILE_LEN`] unless overridden).
    sitemap_file_len: usize,
    media_url: MediaUrlFn,
    clock: Clock,
}

impl SiteReader {
    pub fn new(
        store: Arc<dyn Store>,
        permalinks: Arc<dyn Permalinks>,
        media_url: MediaUrlFn,
        clock: Clock,
    ) -> Self {
        Self {
            store,
            permalinks,
            sitemap_file_len: SITEMAP_FILE_LEN,
            media_url,
            clock,
        }
    }

    /// Splits sitemaps every `len` URLs (at least 1) instead of [`SITEMAP_FILE_LEN`];
    /// for tests of the split (a real site would need 50 001 items).
    pub fn with_sitemap_file_len(mut self, len: usize) -> Self {
        self.sitemap_file_len = len.max(1);
        self
    }

    /// How many sitemap files `kind` has: `ceil(public items / file length)`, from one
    /// count query (no rows loaded); 0 for media. Records `archive:<kind>`.
    pub async fn sitemap_files(&self, kind: &str) -> Result<SiteRead<u32>, CoreError> {
        let now = (self.clock)();
        let mut deps = DepKeys::default();
        deps.insert(archive_key(kind));
        if kind == MEDIA_KIND {
            return Ok(public(0, deps));
        }
        let mut q = published(now);
        q.kind = Some(kind.to_owned());
        let count = self.store.read().content().count(&q).await?;
        let files = count.div_ceil(self.sitemap_file_len as u64);
        Ok(public(u32::try_from(files).unwrap_or(u32::MAX), deps))
    }

    /// The published item of `kind` at `path_parts`: the decoded slug segments after
    /// any type prefix (for pages the ancestor slugs then the slug; for posts the slug
    /// alone; the router strips date or category segments).
    ///
    /// The path is walked from the root with `get_by_slug(kind, parent, slug)`, so every
    /// node must exist with exactly that parent. Only the target must be public: a
    /// draft (or otherwise unpublished) ancestor does not hide a published child, as in
    /// WordPress. The caller compares the returned `url` with the request path for the
    /// canonical redirect.
    pub async fn single(
        &self,
        kind: &str,
        path_parts: &[String],
    ) -> Result<SiteRead<Option<PageItem>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let mut deps = DepKeys::default();
        if kind == MEDIA_KIND || path_parts.is_empty() || path_parts.len() > MAX_PARENT_DEPTH + 1 {
            return Ok(public(None, deps));
        }
        let repos = self.store.read();
        let mut nodes = Nodes::new();
        let mut parent = None;
        let mut target = None;
        for (i, slug) in path_parts.iter().enumerate() {
            let Some(node) = repos.content().get_by_slug(kind, parent, slug).await? else {
                return Ok(public(None, deps));
            };
            deps.insert(post_key(node.id));
            parent = Some(node.id);
            if i + 1 == path_parts.len() {
                target = Some(node);
            } else {
                nodes.insert(node.id, (node.parent_id, node.slug));
            }
        }
        let Some(item) = target.filter(|t| is_public(t, now)) else {
            return Ok(public(None, deps));
        };
        let mut items = self
            .hydrator(repos, now, rules)
            .hydrate(vec![item], nodes, &mut deps)
            .await?;
        Ok(public(items.pop(), deps))
    }

    /// The site root: the item named by `reading.front_page` if it is public, else
    /// page `page` of the post listing (`None` past [`MAX_SKIP`]).
    pub async fn front_page(&self, page: u32) -> Result<SiteRead<Option<FrontPage>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(option_key(FRONT_PAGE_OPTION));
        let id = repos
            .options()
            .get(FRONT_PAGE_OPTION)
            .await?
            .and_then(|e| e.value.as_str().and_then(|s| s.parse::<ContentId>().ok()));
        if let Some(id) = id {
            // Recorded even while it is not public: publishing it must flush the root.
            deps.insert(post_key(id));
            if let Some(item) = repos.content().get(id).await?
                && item.kind != MEDIA_KIND
                && is_public(&item, now)
            {
                let mut items = self
                    .hydrator(repos, now, rules)
                    .hydrate(vec![item], Nodes::new(), &mut deps)
                    .await?;
                if let Some(p) = items.pop() {
                    return Ok(public(Some(FrontPage::Page(Box::new(p))), deps));
                }
            }
        }
        let listing = self.posts(repos, now, rules, page, &mut deps).await?;
        Ok(public(listing.map(FrontPage::Posts), deps))
    }

    /// Page `page` of all public items of `kind`, newest first. Media is never listed.
    /// Whether a type has a public archive at all is the router's decision. `None`
    /// past [`MAX_SKIP`].
    pub async fn archive(
        &self,
        kind: &str,
        page: u32,
    ) -> Result<SiteRead<Option<Listing>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(archive_key(kind));
        if kind == MEDIA_KIND {
            let per_page = per_page(repos, &mut deps).await?;
            return Ok(public(Some(empty(page, per_page, 0)), deps));
        }
        let q = ContentQuery {
            kind: Some(kind.to_owned()),
            ..ContentQuery::default()
        };
        let listing = self.listing(repos, now, rules, q, page, &mut deps).await?;
        Ok(public(listing, deps))
    }

    /// Public items with the term `taxonomy`/`slug`, of every public type of the
    /// taxonomy (its `content_types` but media); `None` for an unknown term, a term
    /// without public items (R7), a taxonomy without public types, or past
    /// [`MAX_SKIP`].
    pub async fn term_archive(
        &self,
        taxonomy: &str,
        slug: &str,
        page: u32,
    ) -> Result<SiteRead<Option<Listing>>, CoreError> {
        let read = self.term_archive_page(taxonomy, slug, page).await?;
        Ok(read.map(|v| v.map(|(_, l)| l)))
    }

    /// [`term_archive`](Self::term_archive) plus the term itself, for the archive's
    /// heading. The term is returned only together with a public listing, so this is
    /// no existence oracle either (R7). Records `term:<id>` and `archive:<kind>` for
    /// each type listed.
    pub async fn term_archive_page(
        &self,
        taxonomy: &str,
        slug: &str,
        page: u32,
    ) -> Result<SiteRead<Option<(TermPublic, Listing)>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        let Some(term) = repos.terms().get_by_slug(taxonomy, slug).await? else {
            return Ok(public(None, deps));
        };
        deps.insert(term_key(term.id));
        let kinds = term_kinds(repos, taxonomy).await?;
        for k in &kinds {
            deps.insert(archive_key(k));
        }
        // Empty kinds would mean "every type", media included.
        if kinds.is_empty() {
            return Ok(public(None, deps));
        }
        let q = ContentQuery {
            kinds,
            term: Some(term.id),
            ..ContentQuery::default()
        };
        let listing = self.listing(repos, now, rules, q, page, &mut deps).await?;
        let found = listing
            .filter(|l| l.total > 0)
            .map(|l| (hydrate::term_public(term), l));
        Ok(public(found, deps))
    }

    /// Posts by the user with `login`, with the author's public view for the archive's
    /// heading; `None` if there is no such user, the user has no public posts (R7), or
    /// past [`MAX_SKIP`]. Records `user:<id>`.
    pub async fn author_archive_page(
        &self,
        login: &str,
        page: u32,
    ) -> Result<SiteRead<Option<(UserPublic, Listing)>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        let Some(user) = repos.users().get_by_login(login).await? else {
            return Ok(public(None, deps));
        };
        deps.insert(user_key(user.id));
        deps.insert(archive_key(POST_KIND));
        let q = ContentQuery {
            kind: Some(POST_KIND.to_owned()),
            author: Some(user.id),
            ..ContentQuery::default()
        };
        let listing = self.listing(repos, now, rules, q, page, &mut deps).await?;
        let found = listing.filter(|l| l.total > 0).map(|l| {
            let author = UserPublic {
                id: user.id,
                login: user.login,
                display_name: user.display_name,
            };
            (author, l)
        });
        Ok(public(found, deps))
    }

    /// Posts published in `year` (and `month`, 1..=12), in UTC like permalinks (R4);
    /// `None` for an invalid or unrepresentable date, a range without public posts
    /// (R27), or past [`MAX_SKIP`].
    pub async fn date_archive(
        &self,
        year: i32,
        month: Option<u32>,
        page: u32,
    ) -> Result<SiteRead<Option<Listing>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let mut deps = DepKeys::default();
        let Some((from, to)) = date_range(year, month) else {
            return Ok(public(None, deps));
        };
        let repos = self.store.read();
        deps.insert(archive_key(POST_KIND));
        let q = ContentQuery {
            kind: Some(POST_KIND.to_owned()),
            published_after: Some(from),
            published_before: Some(to),
            ..ContentQuery::default()
        };
        let listing = self.listing(repos, now, rules, q, page, &mut deps).await?;
        // R27: no public items in the range is the same as no archive (like R7).
        Ok(public(listing.filter(|l| l.total > 0), deps))
    }

    /// Full-text search over every type but media, best match first; `None` past
    /// [`MAX_SKIP`].
    ///
    /// The index is never trusted for visibility: the query asks for published items
    /// only, and the first [`SEARCH_CAP`] (200) matching ids are re-checked against
    /// their rows (published, `published_at <= now`, public type) before paging, so
    /// `total` counts public matches only and is **capped at 200**. Cost per request:
    /// one search call, one `get_many` for the status check (only ids are kept), one
    /// `get_many` for the page, then hydration.
    pub async fn search(&self, q: &str, page: u32) -> Result<SiteRead<Option<Listing>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        let kinds: Vec<String> = repos
            .types()
            .list()
            .await?
            .into_iter()
            .map(|t| t.name)
            .filter(|n| n != MEDIA_KIND)
            .take(MAX_SEARCH_KINDS)
            .collect();
        for k in &kinds {
            deps.insert(archive_key(k));
        }
        let per_page = per_page(repos, &mut deps).await?;
        let page = page.max(1);
        let skip = u64::from(page - 1) * u64::from(per_page);
        if skip > MAX_SKIP {
            return Ok(public(None, deps));
        }
        // Empty kinds would mean "all types", media included.
        if kinds.is_empty() {
            return Ok(public(Some(empty(page, per_page, 0)), deps));
        }
        let mut ids = Vec::new();
        let mut query = SearchQuery {
            q: q.to_owned(),
            kinds: kinds.clone(),
            statuses: vec![ContentStatus::Published],
            readable_by: None,
            limit: MAX_PAGE,
            cursor: None,
        };
        while ids.len() < SEARCH_CAP {
            query.limit =
                u32::try_from((SEARCH_CAP - ids.len()).min(MAX_PAGE as usize)).unwrap_or(MAX_PAGE);
            let found = repos.search().search(&query).await?;
            ids.extend(found.items);
            match found.next_cursor {
                Some(c) => query.cursor = Some(c),
                None => break,
            }
        }
        // Status pass: keep the ids of public rows, in rank order; drop the rows.
        let mut public_ids: Vec<ContentId> = Vec::new();
        for chunk in unique(ids).chunks(MAX_ID_FILTER) {
            public_ids.extend(
                repos
                    .content()
                    .get_many(chunk)
                    .await?
                    .into_iter()
                    .filter(|i| is_public(i, now) && kinds.contains(&i.kind))
                    .map(|i| i.id),
            );
        }
        let total = public_ids.len() as u64;
        let slice: Vec<ContentId> = public_ids
            .into_iter()
            .skip(usize::try_from(skip).unwrap_or(usize::MAX))
            .take(per_page as usize)
            .collect();
        let rows: Vec<ContentItem> = if slice.is_empty() {
            Vec::new()
        } else {
            // Re-checked: a row may have changed since the status pass.
            repos
                .content()
                .get_many(&slice)
                .await?
                .into_iter()
                .filter(|i| is_public(i, now) && kinds.contains(&i.kind))
                .collect()
        };
        let items = self
            .hydrator(repos, now, rules)
            .hydrate(rows, Nodes::new(), &mut deps)
            .await?;
        Ok(public(
            Some(Listing {
                items,
                page,
                per_page,
                total,
            }),
            deps,
        ))
    }

    /// The latest [`FEED_LEN`] public items of a type, or posts with a term (`None` for
    /// an unknown term or one without public posts, R7). A media feed is empty.
    pub async fn feed(
        &self,
        source: &FeedSource,
    ) -> Result<SiteRead<Option<Vec<PageItem>>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        let mut q = published(now);
        match source {
            FeedSource::Kind(kind) => {
                deps.insert(archive_key(kind));
                if kind == MEDIA_KIND {
                    return Ok(public(Some(Vec::new()), deps));
                }
                q.kind = Some(kind.clone());
            }
            FeedSource::Term { taxonomy, slug } => {
                let Some(term) = repos.terms().get_by_slug(taxonomy, slug).await? else {
                    return Ok(public(None, deps));
                };
                deps.insert(term_key(term.id));
                deps.insert(archive_key(POST_KIND));
                q.kind = Some(POST_KIND.to_owned());
                q.term = Some(term.id);
            }
        }
        q.limit = FEED_LEN;
        let rows = repos.content().list(&q).await?.items;
        if rows.is_empty() && matches!(source, FeedSource::Term { .. }) {
            return Ok(public(None, deps));
        }
        let items = self
            .hydrator(repos, now, rules)
            .hydrate(rows, Nodes::new(), &mut deps)
            .await?;
        Ok(public(Some(items), deps))
    }

    /// One sitemap file of `kind`: up to [`SITEMAP_FILE_LEN`] (or the
    /// [overridden](Self::with_sitemap_file_len) length) public items, newest
    /// first, as URL and `updated_at`. Pass the returned cursor back for the next file.
    ///
    /// Deps are `archive:<kind>` only: every content write (and every term write)
    /// emits it, which covers any URL change; per-item keys for 50 000 URLs would only
    /// bloat the cache index.
    pub async fn sitemap(
        &self,
        kind: &str,
        cursor: Option<&str>,
    ) -> Result<SiteRead<SitemapPage>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(archive_key(kind));
        let mut out = SitemapPage {
            entries: Vec::new(),
            next_cursor: None,
        };
        if kind == MEDIA_KIND {
            return Ok(public(out, deps));
        }
        let mut q = published(now);
        q.kind = Some(kind.to_owned());
        q.cursor = cursor.map(str::to_owned);
        let hydrator = self.hydrator(repos, now, rules);
        let mut nodes = Nodes::new();
        let file_len = self.sitemap_file_len;
        while out.entries.len() < file_len {
            q.limit = u32::try_from((file_len - out.entries.len()).min(MAX_PAGE as usize))
                .unwrap_or(MAX_PAGE);
            let page = repos.content().list(&q).await?;
            hydrator.load_ancestors(&page.items, &mut nodes).await?;
            let ids: Vec<ContentId> = page.items.iter().map(|i| i.id).collect();
            let mut terms = if ids.is_empty() {
                Default::default()
            } else {
                repos.terms().for_items(&ids).await?
            };
            for item in page.items {
                let input = PermalinkInput {
                    ancestors: chain(&nodes, item.parent_id),
                    primary_term: primary_term(terms.remove(&item.id).unwrap_or_default()),
                    item,
                };
                out.entries.push(SitemapEntry {
                    url: rules.path(&input),
                    lastmod: input.item.updated_at,
                });
            }
            match page.next_cursor {
                Some(c) => q.cursor = Some(c),
                None => {
                    q.cursor = None;
                    break;
                }
            }
        }
        out.next_cursor = q.cursor;
        Ok(public(out, deps))
    }

    /// Up to `n` (at most [`MAX_RELATED`]) public items of the same type sharing a term
    /// with `item`, excluding itself: newest first per term, terms in `item`'s order.
    pub async fn related(
        &self,
        item: &PageItem,
        n: usize,
    ) -> Result<SiteRead<Vec<PageItem>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(archive_key(&item.item.kind));
        let n = n.min(MAX_RELATED);
        let mut found: Vec<ContentItem> = Vec::new();
        let mut exclude = vec![item.item.id];
        for term in item.terms.iter().take(MAX_RELATED) {
            if found.len() >= n {
                break;
            }
            let mut q = published(now);
            q.kind = Some(item.item.kind.clone());
            q.term = Some(term.id);
            q.exclude = exclude.clone();
            q.limit = u32::try_from(n - found.len()).unwrap_or(1);
            for row in repos.content().list(&q).await?.items {
                exclude.push(row.id);
                found.push(row);
            }
        }
        let items = self
            .hydrator(repos, now, rules)
            .hydrate(found, Nodes::new(), &mut deps)
            .await?;
        Ok(public(items, deps))
    }

    /// The public item `id` (any type but media), for lookups by id such as plugin
    /// `content.get-post`. Records `post:<id>` whether or not the item is public, so
    /// publishing it refreshes pages that asked for it.
    pub async fn by_id(&self, id: ContentId) -> Result<SiteRead<Option<PageItem>>, CoreError> {
        self.by_id_where(id, &|_| true).await
    }

    /// [`by_id`](Self::by_id) restricted to items whose type passes `kind_ok`, checked
    /// before hydration so a refused read costs one store call (plugin `content.read`
    /// type grants).
    pub async fn by_id_where(
        &self,
        id: ContentId,
        kind_ok: &(dyn Fn(&str) -> bool + Sync),
    ) -> Result<SiteRead<Option<PageItem>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(post_key(id));
        let Some(item) = repos
            .content()
            .get(id)
            .await?
            .filter(|i| i.kind != MEDIA_KIND && kind_ok(&i.kind) && is_public(i, now))
        else {
            return Ok(public(None, deps));
        };
        let mut items = self
            .hydrator(repos, now, rules)
            .hydrate(vec![item], Nodes::new(), &mut deps)
            .await?;
        Ok(public(items.pop(), deps))
    }

    /// Title and URL of each public item (any type but media) among `ids`, for menus:
    /// one `get_many` per 200 ids, then terms (for the primary term in the URL) and
    /// ancestors (hierarchical types) of the public ones; no authors, media or body
    /// rendering. Records `post:<id>` for every id asked for, shown or not, so
    /// publishing a hidden target refreshes the page, plus the keys of the terms and
    /// ancestors the URLs use.
    pub async fn links(
        &self,
        ids: &[ContentId],
    ) -> Result<SiteRead<HashMap<ContentId, ItemLink>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        let ids = unique(ids.iter().copied());
        let mut items = Vec::new();
        for chunk in ids.chunks(MAX_ID_FILTER) {
            for id in chunk {
                deps.insert(post_key(*id));
            }
            items.extend(
                repos
                    .content()
                    .get_many(chunk)
                    .await?
                    .into_iter()
                    .filter(|i| i.kind != MEDIA_KIND && is_public(i, now)),
            );
        }
        let mut out = HashMap::new();
        if items.is_empty() {
            return Ok(public(out, deps));
        }
        let mut terms = HashMap::new();
        for chunk in unique(items.iter().map(|i| i.id)).chunks(MAX_ID_FILTER) {
            terms.extend(repos.terms().for_items(chunk).await?);
        }
        let mut nodes = Nodes::new();
        self.hydrator(repos, now, rules)
            .load_ancestors(&items, &mut nodes)
            .await?;
        for item in items {
            let item_terms = terms.remove(&item.id).unwrap_or_default();
            for t in &item_terms {
                deps.insert(term_key(t.id));
            }
            let ancestors = chain(&nodes, item.parent_id);
            for (id, _) in &ancestors {
                deps.insert(post_key(*id));
            }
            let input = PermalinkInput {
                item,
                ancestors,
                primary_term: primary_term(item_terms),
            };
            let url = rules.path(&input);
            out.insert(
                input.item.id,
                ItemLink {
                    title: input.item.title,
                    url,
                },
            );
        }
        Ok(public(out, deps))
    }

    /// Term `id` if its archive would show (it has a public item of a public type of
    /// its taxonomy, ruling R7): three store calls (the term, its taxonomy, then one
    /// public item at most), no hydration. Records `term:<id>` and `archive:post`
    /// either way, plus `archive:<kind>` for each public type of the taxonomy, so the
    /// first public item in it brings the link back.
    pub async fn term_link(&self, id: TermId) -> Result<SiteRead<Option<TermPublic>>, CoreError> {
        let now = (self.clock)();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(term_key(id));
        deps.insert(archive_key(POST_KIND));
        let Some(term) = repos.terms().get(id).await? else {
            return Ok(public(None, deps));
        };
        let kinds = term_kinds(repos, &term.taxonomy).await?;
        for k in &kinds {
            deps.insert(archive_key(k));
        }
        if kinds.is_empty() {
            return Ok(public(None, deps));
        }
        let mut q = published(now);
        q.kinds = kinds;
        q.term = Some(id);
        q.limit = 1;
        let shown = !repos.content().list(&q).await?.items.is_empty();
        Ok(public(shown.then(|| hydrate::term_public(term)), deps))
    }

    /// The published media item `id` with its variants, for the `image()` template
    /// helper: `None` unless `id` is a media item that is public (published,
    /// `published_at <= now`) and has a media row. Records `post:<id>` either way, so
    /// publishing it refreshes the page.
    pub async fn media(&self, id: ContentId) -> Result<SiteRead<Option<MediaPublic>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(post_key(id));
        let public_media = repos
            .content()
            .get(id)
            .await?
            .is_some_and(|i| i.kind == MEDIA_KIND && is_public(&i, now));
        if !public_media {
            return Ok(public(None, deps));
        }
        let row = repos.media().get(id).await?;
        let hydrator = self.hydrator(repos, now, rules);
        Ok(public(row.map(|m| hydrator.media_public(&m)), deps))
    }

    /// The file `hash` names, if it is the original or a variant of a **public** media
    /// item (published, `published_at <= now`); `None` otherwise. Several media items
    /// may share a file (the same upload twice): the first public one, by item id,
    /// answers. Store calls: one lookup by artifact (at most [`MAX_MEDIA_ROWS`] rows)
    /// and one `get_many` for their status.
    pub async fn media_file(&self, hash: &ArtifactHash) -> Result<Option<MediaFile>, CoreError> {
        let now = (self.clock)();
        let repos = self.store.read();
        let rows = repos.media().find_by_artifact(hash, MAX_MEDIA_ROWS).await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let ids: Vec<ContentId> = rows.iter().map(|m| m.item_id).collect();
        let public_ids: std::collections::HashSet<ContentId> = repos
            .content()
            .get_many(&ids)
            .await?
            .into_iter()
            .filter(|i| i.kind == MEDIA_KIND && is_public(i, now))
            .map(|i| i.id)
            .collect();
        let Some(row) = rows.into_iter().find(|m| public_ids.contains(&m.item_id)) else {
            return Ok(None);
        };
        if row.artifact_hash == *hash {
            return Ok(Some(MediaFile {
                item: row.item_id,
                variant: None,
                mime: row.mime,
                name: row.original_name,
                sanitized: row.sanitized,
            }));
        }
        let variant = row.variants.as_object().and_then(|vs| {
            vs.iter()
                .find(|(_, v)| v.get("hash").and_then(|h| h.as_str()) == Some(hash.as_str()))
                .map(|(name, v)| {
                    let mime = v.get("mime").and_then(|m| m.as_str()).map(str::to_owned);
                    (name.clone(), mime)
                })
        });
        Ok(variant.map(|(name, mime)| MediaFile {
            item: row.item_id,
            variant: Some(name),
            mime: mime.unwrap_or_else(|| row.mime.clone()),
            name: row.original_name.clone(),
            sanitized: false,
        }))
    }

    /// The public comment thread of item `id`, for the `comments()` template helper:
    /// `None` unless `id` is a `post` that is public now (published,
    /// `published_at <= now`); else its approved comments in thread order (at most
    /// [`MAX_THREAD`](crate::comments::MAX_THREAD); never pending, spam or trash, never
    /// an e-mail or address hash) and `open` = `comments.enabled`. A closed thread is
    /// still listed. Records `post:<id>` and `option:comments.enabled` either way, so a
    /// moderation (which invalidates `post:<item>`) or opening and closing comments
    /// refreshes the page. Store calls: the item, the option, the approved rows, and for a
    /// reply whose ancestors are hidden at most `MAX_COMMENT_DEPTH` lookups (`can_reply`).
    pub async fn comments(
        &self,
        id: ContentId,
    ) -> Result<SiteRead<Option<CommentThread>>, CoreError> {
        let now = (self.clock)();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(post_key(id));
        deps.insert(option_key(comments::ENABLED_OPTION));
        let open_post = repos
            .content()
            .get(id)
            .await?
            .is_some_and(|i| i.kind == comments::COMMENT_KIND && is_public(&i, now));
        if !open_post {
            return Ok(public(None, deps));
        }
        let enabled = repos
            .options()
            .get(comments::ENABLED_OPTION)
            .await?
            .map(|e| e.value);
        let open = comments::settings_from(enabled.as_ref(), None).enabled;
        let rows = repos
            .comments()
            .approved_for_item(id, comments::MAX_THREAD)
            .await?;
        let items = comments::public_thread(repos, rows).await?;
        Ok(public(Some(CommentThread { open, items }), deps))
    }

    /// Whether item `id` is public (and not media): one store call, no hydration. For
    /// gating reads of per-item data such as meta.
    pub async fn is_visible(&self, id: ContentId) -> Result<bool, CoreError> {
        let now = (self.clock)();
        let item = self.store.read().content().get(id).await?;
        Ok(item.is_some_and(|i| i.kind != MEDIA_KIND && is_public(&i, now)))
    }

    /// Up to `limit` (at most [`MAX_LATEST`]) public items of `kind`, newest first,
    /// after skipping `offset` of them; empty for media and when `offset` exceeds
    /// [`MAX_SKIP`] (the same paging bound as archives).
    pub async fn latest(
        &self,
        kind: &str,
        limit: u32,
        offset: u32,
    ) -> Result<SiteRead<Vec<PageItem>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(archive_key(kind));
        let limit = limit.min(MAX_LATEST);
        if kind == MEDIA_KIND || limit == 0 || u64::from(offset) > MAX_SKIP {
            return Ok(public(Vec::new(), deps));
        }
        let mut q = published(now);
        q.kind = Some(kind.to_owned());
        if skip_rows(repos, &mut q, u64::from(offset)).await?.is_err() {
            return Ok(public(Vec::new(), deps));
        }
        q.limit = limit;
        let rows = repos.content().list(&q).await?.items;
        let items = self
            .hydrator(repos, now, rules)
            .hydrate(rows, Nodes::new(), &mut deps)
            .await?;
        Ok(public(items, deps))
    }

    /// Up to `limit` (1..=[`MAX_QUERY_LIMIT`], else `Invalid`) public items of `kind`
    /// with the term `taxonomy`/`slug`, newest first, for the theme's
    /// `query(type, term, limit)` helper: one content query, then hydration (no
    /// `reading.per_page`, no count). Empty for an unknown term or taxonomy and when
    /// `kind` is not a public type of the taxonomy. Records `archive:<kind>`, and
    /// `term:<id>` when the term exists.
    pub async fn term_items(
        &self,
        kind: &str,
        taxonomy: &str,
        slug: &str,
        limit: u32,
    ) -> Result<SiteRead<Vec<PageItem>>, CoreError> {
        if !(1..=MAX_QUERY_LIMIT).contains(&limit) {
            return Err(CoreError::invalid_field(
                "limit",
                format!("limit must be between 1 and {MAX_QUERY_LIMIT}"),
            ));
        }
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(archive_key(kind));
        let Some(term) = repos.terms().get_by_slug(taxonomy, slug).await? else {
            return Ok(public(Vec::new(), deps));
        };
        deps.insert(term_key(term.id));
        if !term_kinds(repos, taxonomy).await?.iter().any(|k| k == kind) {
            return Ok(public(Vec::new(), deps));
        }
        let mut q = published(now);
        q.kind = Some(kind.to_owned());
        q.term = Some(term.id);
        q.limit = limit;
        let rows = repos.content().list(&q).await?.items;
        let items = self
            .hydrator(repos, now, rules)
            .hydrate(rows, Nodes::new(), &mut deps)
            .await?;
        Ok(public(items, deps))
    }

    /// Item `id` in any status but trash, for an actor who may read it in the admin
    /// ([`can_read`] at the reader's clock, rulings R17/R23: own items, items publicly
    /// visible now, or `edit_others`; else `Forbidden("edit_others")`). Trashed or missing items are `NotFound`. The result
    /// is marked not cacheable.
    pub async fn preview(
        &self,
        actor: &Actor,
        id: ContentId,
    ) -> Result<SiteRead<Previewed>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let item = repos.content().get(id).await?.ok_or(CoreError::NotFound)?;
        if !can_read(actor, &item, now) {
            return Err(CoreError::Forbidden(Capability::EditOthers.as_str()));
        }
        if item.status == ContentStatus::Trash {
            return Err(CoreError::NotFound);
        }
        let mut deps = DepKeys::default();
        let status = item.status;
        let item = self
            .hydrator(repos, now, rules)
            .hydrate(vec![item], Nodes::new(), &mut deps)
            .await?
            .pop()
            .ok_or(CoreError::NotFound)?;
        Ok(SiteRead {
            value: Previewed::new(item, status),
            deps,
            cacheable: false,
        })
    }

    fn hydrator<'a>(
        &'a self,
        repos: &'a dyn Repos,
        now: Timestamp,
        rules: &'a dyn Permalinks,
    ) -> Hydrator<'a> {
        Hydrator {
            repos,
            permalinks: rules,
            media_url: &self.media_url,
            now,
        }
    }

    async fn posts(
        &self,
        repos: &dyn Repos,
        now: Timestamp,
        rules: &dyn Permalinks,
        page: u32,
        deps: &mut DepKeys,
    ) -> Result<Option<Listing>, CoreError> {
        deps.insert(archive_key(POST_KIND));
        let q = ContentQuery {
            kind: Some(POST_KIND.to_owned()),
            ..ContentQuery::default()
        };
        self.listing(repos, now, rules, q, page, deps).await
    }

    /// Page `page` (1-based) of `filter` with the public restriction forced on, or
    /// `None` when reaching it would skip more than [`MAX_SKIP`] rows (see the module
    /// docs for paging and the call budget).
    async fn listing(
        &self,
        repos: &dyn Repos,
        now: Timestamp,
        rules: &dyn Permalinks,
        filter: ContentQuery,
        page: u32,
        deps: &mut DepKeys,
    ) -> Result<Option<Listing>, CoreError> {
        let per_page = per_page(repos, deps).await?;
        let page = page.max(1);
        let mut q = published(now);
        q.kind = filter.kind;
        q.kinds = filter.kinds;
        q.term = filter.term;
        q.author = filter.author;
        q.published_after = filter.published_after;
        if let (Some(before), Some(limit)) = (filter.published_before, q.published_before) {
            q.published_before = Some(before.min(limit));
        }
        // Skip the earlier pages by walking cursors.
        let skipped = u64::from(page - 1) * u64::from(per_page);
        if skipped > MAX_SKIP {
            return Ok(None);
        }
        // Past the last page: one count instead of a walk (no rows loaded).
        let mut counted = None;
        if skipped > 0 {
            let total = repos.content().count(&q).await?;
            if skipped >= total {
                return Ok(Some(empty(page, per_page, total)));
            }
            counted = Some(total);
        }
        if let Err(walked) = skip_rows(repos, &mut q, skipped).await? {
            // Rows went away between the count and the walk.
            return Ok(Some(empty(page, per_page, walked)));
        }
        q.limit = per_page;
        let rows = repos.content().list(&q).await?;
        let total = match counted {
            Some(total) => total.max(skipped + rows.items.len() as u64),
            None if rows.next_cursor.is_none() => rows.items.len() as u64,
            None => repos.content().count(&q).await?,
        };
        let items = self
            .hydrator(repos, now, rules)
            .hydrate(rows.items, Nodes::new(), deps)
            .await?;
        Ok(Some(Listing {
            items,
            page,
            per_page,
            total,
        }))
    }
}

/// Advance `q.cursor` past `skip` rows by walking pages of up to `MAX_PAGE` rows.
/// `Err(walked)` when the rows ran out first (`walked` is how many there were).
async fn skip_rows(
    repos: &dyn Repos,
    q: &mut ContentQuery,
    skip: u64,
) -> Result<Result<(), u64>, CoreError> {
    let mut walked = 0;
    while walked < skip {
        let mut s = q.clone();
        s.limit = u32::try_from((skip - walked).min(u64::from(MAX_PAGE))).unwrap_or(MAX_PAGE);
        let p = repos.content().list(&s).await?;
        walked += p.items.len() as u64;
        match p.next_cursor {
            Some(c) if !p.items.is_empty() => q.cursor = Some(c),
            _ => return Ok(Err(walked)),
        }
    }
    Ok(Ok(()))
}

fn public<T>(value: T, deps: DepKeys) -> SiteRead<T> {
    SiteRead {
        value,
        deps,
        cacheable: true,
    }
}

fn empty(page: u32, per_page: u32, total: u64) -> Listing {
    Listing {
        items: Vec::new(),
        page: page.max(1),
        per_page,
        total,
    }
}

/// A query for public items: published, `published_at <= now`.
pub(crate) fn published(now: Timestamp) -> ContentQuery {
    // `published_before` is exclusive; one microsecond past now includes now itself.
    let limit = Timestamp::from_micros(now.as_micros().saturating_add(1)).unwrap_or(now);
    ContentQuery {
        statuses: vec![ContentStatus::Published],
        published_before: Some(limit),
        ..ContentQuery::default()
    }
}

/// The public types of `tax` (its `content_types` but media), at most
/// [`MAX_CONTENT_KINDS`]: the types its term archives list.
pub(crate) fn public_kinds(tax: &Taxonomy) -> Vec<String> {
    tax.content_types
        .iter()
        .filter(|k| *k != MEDIA_KIND)
        .take(MAX_CONTENT_KINDS)
        .cloned()
        .collect()
}

/// [`public_kinds`] of taxonomy `name`; empty when it is unknown.
async fn term_kinds(repos: &dyn Repos, name: &str) -> Result<Vec<String>, CoreError> {
    Ok(repos
        .taxonomies()
        .get(name)
        .await?
        .map(|t| public_kinds(&t))
        .unwrap_or_default())
}

/// `reading.per_page`: an integer clamped to 1..=100, else the default.
async fn per_page(repos: &dyn Repos, deps: &mut DepKeys) -> Result<u32, CoreError> {
    deps.insert(option_key(PER_PAGE_OPTION));
    let value = repos.options().get(PER_PAGE_OPTION).await?;
    Ok(value
        .and_then(|e| e.value.as_i64())
        .and_then(|n| u32::try_from(n.clamp(1, MAX_PER_PAGE)).ok())
        .unwrap_or(DEFAULT_PER_PAGE))
}

/// `[start, end)` of a UTC year or month.
fn date_range(year: i32, month: Option<u32>) -> Option<(Timestamp, Timestamp)> {
    let (start, end) = match month {
        Some(m @ 1..=12) => {
            let next = if m == 12 {
                (year.checked_add(1)?, 1)
            } else {
                (year, m + 1)
            };
            ((year, m), next)
        }
        Some(_) => return None,
        None => ((year, 1), (year.checked_add(1)?, 1)),
    };
    let at = |(y, m): (i32, u32)| {
        NaiveDate::from_ymd_opt(y, m, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| Timestamp::from(d.and_utc()))
    };
    Some((at(start)?, at(end)?))
}
