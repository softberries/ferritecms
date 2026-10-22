//! The public read facade: everything the public site renders comes through
//! [`SiteReader`].
//!
//! **Published-only by construction.** Every public method builds its own
//! `ContentQuery`/`SearchQuery` and forces `status = published` and
//! `published_at <= now` (the clock is injected). Items fetched another way (by slug,
//! by id, through the search index) are re-checked with the same predicate in Rust, so
//! a lagging search index or a future-dated published item never shows. Drafts are
//! reachable only through [`SiteReader::preview`], which is gated by `can_edit` and
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
//! (skipped when the page is the last one), terms, users, and, when any item shows
//! media, media items and media rows: at most 7.
//!
//! **Offset paging.** The store pages by cursor; `/page/N/` skips `N - 1` pages by
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

use std::sync::Arc;

use chrono::NaiveDate;
use ferrite_domain::{
    ArtifactHash, ContentId, ContentItem, ContentQuery, ContentStatus, MAX_PAGE, SearchQuery,
    Timestamp,
};
use ferrite_store::{MAX_ID_FILTER, MAX_SEARCH_KINDS, Repos, Store};

use crate::{
    Actor, CoreError, MAX_PARENT_DEPTH,
    actor::can_edit,
    events::{archive_key, option_key, post_key, term_key},
    permalink::{PermalinkInput, Permalinks, primary_term},
};
use hydrate::{Hydrator, Nodes, chain, unique};
pub use model::{
    DepKeys, FeedSource, FrontPage, Listing, MediaPublic, MediaVariant, PageItem, PublicItem,
    SiteRead, SitemapEntry, SitemapPage, TermPublic, UserPublic,
};

/// Maps an artifact hash and a file name to a public URL (`MediaStore::public_url`).
pub type MediaUrlFn = Arc<dyn Fn(&ArtifactHash, &str) -> String + Send + Sync>;
/// The current time; injected so tests control "now".
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// Content type of media items.
pub(crate) const MEDIA_KIND: &str = "media";
/// Content type listed on the front page and in date, author and term archives.
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
/// Most items [`SiteReader::related`] returns.
pub const MAX_RELATED: usize = 10;
/// Most search matches paged through (and counted in `total`).
pub const SEARCH_CAP: usize = 1000;

/// Whether `item` is publicly visible at `now`: published, with a publish time not in
/// the future.
pub(crate) fn is_public(item: &ContentItem, now: Timestamp) -> bool {
    item.status == ContentStatus::Published && item.published_at.is_some_and(|t| t <= now)
}

/// The published-only read facade; see the module docs.
pub struct SiteReader {
    store: Arc<dyn Store>,
    permalinks: Arc<dyn Permalinks>,
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
            media_url,
            clock,
        }
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
            .hydrator(repos, now)
            .hydrate(vec![item], nodes, &mut deps)
            .await?;
        Ok(public(items.pop(), deps))
    }

    /// The site root: the item named by `reading.front_page` if it is public, else
    /// page `page` of the post listing (`None` past [`MAX_SKIP`]).
    pub async fn front_page(&self, page: u32) -> Result<SiteRead<Option<FrontPage>>, CoreError> {
        let now = (self.clock)();
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
                    .hydrator(repos, now)
                    .hydrate(vec![item], Nodes::new(), &mut deps)
                    .await?;
                if let Some(p) = items.pop() {
                    return Ok(public(Some(FrontPage::Page(Box::new(p))), deps));
                }
            }
        }
        let listing = self.posts(repos, now, page, &mut deps).await?;
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
        let listing = self.listing(repos, now, q, page, &mut deps).await?;
        Ok(public(listing, deps))
    }

    /// Posts with the term `taxonomy`/`slug`; `None` for an unknown term, a term
    /// without public posts (R7), or past [`MAX_SKIP`].
    pub async fn term_archive(
        &self,
        taxonomy: &str,
        slug: &str,
        page: u32,
    ) -> Result<SiteRead<Option<Listing>>, CoreError> {
        let now = (self.clock)();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        let Some(term) = repos.terms().get_by_slug(taxonomy, slug).await? else {
            return Ok(public(None, deps));
        };
        deps.insert(term_key(term.id));
        deps.insert(archive_key(POST_KIND));
        let q = ContentQuery {
            kind: Some(POST_KIND.to_owned()),
            term: Some(term.id),
            ..ContentQuery::default()
        };
        let listing = self.listing(repos, now, q, page, &mut deps).await?;
        Ok(public(listing.filter(|l| l.total > 0), deps))
    }

    /// Posts by the user with `login`; `None` if there is no such user, the user has
    /// no public posts (R7), or past [`MAX_SKIP`].
    pub async fn author_archive(
        &self,
        login: &str,
        page: u32,
    ) -> Result<SiteRead<Option<Listing>>, CoreError> {
        let now = (self.clock)();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        let Some(user) = repos.users().get_by_login(login).await? else {
            return Ok(public(None, deps));
        };
        deps.insert(archive_key(POST_KIND));
        let q = ContentQuery {
            kind: Some(POST_KIND.to_owned()),
            author: Some(user.id),
            ..ContentQuery::default()
        };
        let listing = self.listing(repos, now, q, page, &mut deps).await?;
        Ok(public(listing.filter(|l| l.total > 0), deps))
    }

    /// Posts published in `year` (and `month`, 1..=12), in UTC like permalinks (R4);
    /// `None` for an invalid or unrepresentable date, or past [`MAX_SKIP`].
    pub async fn date_archive(
        &self,
        year: i32,
        month: Option<u32>,
        page: u32,
    ) -> Result<SiteRead<Option<Listing>>, CoreError> {
        let now = (self.clock)();
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
        let listing = self.listing(repos, now, q, page, &mut deps).await?;
        Ok(public(listing, deps))
    }

    /// Full-text search over every type but media, best match first; `None` past
    /// [`MAX_SKIP`].
    ///
    /// The index is never trusted for visibility: the query asks for published items
    /// only, and up to [`SEARCH_CAP`] matching ids are then loaded (`get_many`, 200 per
    /// call) and re-checked (published, `published_at <= now`, public type) before
    /// paging, so `total` counts public matches only.
    pub async fn search(&self, q: &str, page: u32) -> Result<SiteRead<Option<Listing>>, CoreError> {
        let now = (self.clock)();
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
        let mut rows: Vec<ContentItem> = Vec::new();
        for chunk in unique(ids).chunks(MAX_ID_FILTER) {
            rows.extend(
                repos
                    .content()
                    .get_many(chunk)
                    .await?
                    .into_iter()
                    .filter(|i| is_public(i, now) && kinds.contains(&i.kind)),
            );
        }
        let total = rows.len() as u64;
        let rows: Vec<ContentItem> = rows
            .into_iter()
            .skip(usize::try_from(skip).unwrap_or(usize::MAX))
            .take(per_page as usize)
            .collect();
        let items = self
            .hydrator(repos, now)
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
            .hydrator(repos, now)
            .hydrate(rows, Nodes::new(), &mut deps)
            .await?;
        Ok(public(Some(items), deps))
    }

    /// One sitemap file of `kind`: up to [`SITEMAP_FILE_LEN`] public items, newest
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
        let hydrator = self.hydrator(repos, now);
        let mut nodes = Nodes::new();
        while out.entries.len() < SITEMAP_FILE_LEN {
            q.limit = u32::try_from((SITEMAP_FILE_LEN - out.entries.len()).min(MAX_PAGE as usize))
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
                    url: self.permalinks.path(&input),
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
            .hydrator(repos, now)
            .hydrate(found, Nodes::new(), &mut deps)
            .await?;
        Ok(public(items, deps))
    }

    /// Item `id` in any status but trash, for an actor who may edit it (`can_edit`).
    /// Trashed or missing items are `NotFound`. The result is marked not cacheable.
    pub async fn preview(
        &self,
        actor: &Actor,
        id: ContentId,
    ) -> Result<SiteRead<PageItem>, CoreError> {
        let now = (self.clock)();
        let repos = self.store.read();
        let item = repos.content().get(id).await?.ok_or(CoreError::NotFound)?;
        can_edit(actor, &item)?;
        if item.status == ContentStatus::Trash {
            return Err(CoreError::NotFound);
        }
        let mut deps = DepKeys::default();
        let item = self
            .hydrator(repos, now)
            .hydrate(vec![item], Nodes::new(), &mut deps)
            .await?
            .pop()
            .ok_or(CoreError::NotFound)?;
        Ok(SiteRead {
            value: item,
            deps,
            cacheable: false,
        })
    }

    fn hydrator<'a>(&'a self, repos: &'a dyn Repos, now: Timestamp) -> Hydrator<'a> {
        Hydrator {
            repos,
            permalinks: self.permalinks.as_ref(),
            media_url: &self.media_url,
            now,
        }
    }

    async fn posts(
        &self,
        repos: &dyn Repos,
        now: Timestamp,
        page: u32,
        deps: &mut DepKeys,
    ) -> Result<Option<Listing>, CoreError> {
        deps.insert(archive_key(POST_KIND));
        let q = ContentQuery {
            kind: Some(POST_KIND.to_owned()),
            ..ContentQuery::default()
        };
        self.listing(repos, now, q, page, deps).await
    }

    /// Page `page` (1-based) of `filter` with the public restriction forced on, or
    /// `None` when reaching it would skip more than [`MAX_SKIP`] rows (see the module
    /// docs for paging and the call budget).
    async fn listing(
        &self,
        repos: &dyn Repos,
        now: Timestamp,
        filter: ContentQuery,
        page: u32,
        deps: &mut DepKeys,
    ) -> Result<Option<Listing>, CoreError> {
        let per_page = per_page(repos, deps).await?;
        let page = page.max(1);
        let mut q = published(now);
        q.kind = filter.kind;
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
        let mut walked = 0;
        while walked < skipped {
            let mut s = q.clone();
            s.limit =
                u32::try_from((skipped - walked).min(u64::from(MAX_PAGE))).unwrap_or(MAX_PAGE);
            let p = repos.content().list(&s).await?;
            walked += p.items.len() as u64;
            match p.next_cursor {
                Some(c) if !p.items.is_empty() => q.cursor = Some(c),
                // Ran out before reaching the page: everything there is was walked.
                _ => return Ok(Some(empty(page, per_page, walked))),
            }
        }
        q.limit = per_page;
        let rows = repos.content().list(&q).await?;
        let total = if rows.next_cursor.is_none() {
            skipped + rows.items.len() as u64
        } else {
            repos.content().count(&q).await?
        };
        let items = self
            .hydrator(repos, now)
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
fn published(now: Timestamp) -> ContentQuery {
    // `published_before` is exclusive; one microsecond past now includes now itself.
    let limit = Timestamp::from_micros(now.as_micros().saturating_add(1)).unwrap_or(now);
    ContentQuery {
        statuses: vec![ContentStatus::Published],
        published_before: Some(limit),
        ..ContentQuery::default()
    }
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
