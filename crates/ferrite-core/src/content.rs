//! Content create/update: slugs, terms, revisions, search, redirects and outbox events,
//! all in one transaction per call.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

use ferrite_domain::{
    ContentId, ContentItem, ContentQuery, ContentStatus, ContentType, MEDIA_KIND, NewContent,
    ParentFilter, Revision, RevisionId, Term, TermId, Timestamp,
};
use ferrite_store::{MAX_ID_FILTER, Repos, Store, StoreError, Tx};

use crate::{
    Actor, CoreError, MAX_PARENT_DEPTH, actions,
    actor::{Area, Capability, can_edit, can_read, require, require_read},
    blocks, events, fields,
    permalink::{PermalinkInput, Permalinks, ancestors, load_input, primary_term},
    redirects::{move_redirects, redirect_keys},
    slug,
    tx::finish,
};

mod admin;
mod allowed;
mod lifecycle;
pub mod seo;

pub use admin::{
    AdminQuery, AuthorRef, AutosaveDraft, ContentRef, MAX_REFS, MAX_REVISIONS_LISTED, SiteSearch,
    StatusCounts,
};
pub use allowed::{AllowedActions, allowed_actions};
pub(crate) use lifecycle::reclaim_path;
pub use seo::{Robots, Seo, SeoPatch};

/// Unique index on (type, parent, slug); a violation of it means another writer took
/// the slug between our lookup and our write.
const SLUG_CONSTRAINT: &str = "content_items_slug_uq";
const SLUG_ATTEMPTS: usize = 5;
const KEEP_REVISIONS: u32 = 50;
/// Length of a derived excerpt.
pub(crate) const EXCERPT_CHARS: usize = 160;
/// Most descendants walked (any non-trash status) when an item moves.
const MAX_DESCENDANTS: usize = 1000;
/// Statuses walked by [`move_descendants`]: everything but trash.
pub(crate) const WALKED: [ContentStatus; 5] = [
    ContentStatus::Draft,
    ContentStatus::Pending,
    ContentStatus::Scheduled,
    ContentStatus::Private,
    ContentStatus::Published,
];
/// Children fetched per query while walking descendants.
const DESCENDANT_BATCH: u32 = 200;

/// Caller-editable fields of a content item. Status, author and publish times are not
/// part of it: status changes go through the lifecycle methods only.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentDraft {
    pub kind: String,
    pub title: String,
    /// `None`: generated from the title on create, kept as is on update.
    pub slug: Option<String>,
    pub parent_id: Option<ContentId>,
    /// `None`: derived from the body (160 characters).
    pub excerpt: Option<String>,
    pub body: serde_json::Value,
    pub fields: serde_json::Value,
    pub menu_order: i32,
    /// Replaces the item's terms in this order; duplicates are ignored.
    pub terms: Vec<TermId>,
    /// Changes to the item's SEO meta, written in the same transaction; `None` keeps
    /// them (M2b P28).
    pub seo: Option<SeoPatch>,
}

/// Creates and edits content items.
pub struct ContentService {
    store: Arc<dyn Store>,
    permalinks: Arc<dyn Permalinks>,
}

impl ContentService {
    pub fn new(store: Arc<dyn Store>, permalinks: Arc<dyn Permalinks>) -> Self {
        Self { store, permalinks }
    }

    /// The public path of item `id` in any status (a missing item is `NotFound`).
    /// Reads outside a transaction; never call it from inside one.
    pub async fn permalink(&self, id: ContentId) -> Result<String, CoreError> {
        let repos = self.store.read();
        let item = repos.content().get(id).await?.ok_or(CoreError::NotFound)?;
        let input = load_input(repos, item).await?;
        Ok(self.permalinks.path(&input))
    }

    /// Fetches an item; a missing one is `NotFound`.
    pub async fn get(&self, id: ContentId) -> Result<ContentItem, CoreError> {
        self.store
            .read()
            .content()
            .get(id)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// Creates a draft authored by `actor` (needs `edit_own`).
    ///
    /// The slug is the validated caller slug or one generated from the title. If it is
    /// taken under the same type and parent, `-2`, `-3`, … is appended, for caller slugs
    /// too (as WordPress does); concurrent creates that race for the same slug retry up
    /// to 5 times. A top-level item may not take a [`slug::RESERVED`] word, the name
    /// of a registered content type (its archive prefix) or taxonomy (its term
    /// prefix), a [`slug::ROUTE_WORDS`] word or four digits (a year archive; ruling
    /// R13), and a child may not take a [`slug::CHILD_RESERVED`] word (`page`): a
    /// caller slug is `InvalidField { field: "slug", .. }` ("reserved slug"), a
    /// generated one gets the `-N` suffix. The type check and these rules share one
    /// `types().list()` read.
    ///
    /// Top-level items of types that share the root path namespace
    /// ([`Permalinks::root_namespace_peers`], e.g. posts at `/%slug%/` and top-level
    /// pages) also count each other's top-level slugs as taken (`-N`, caller slugs
    /// too). That check reads committed rows: two concurrent creates of *different*
    /// types can still both take a slug (no unique index spans types); the router's
    /// precedence (page before post) decides which one is served.
    ///
    /// A parent is allowed only for hierarchical types; it must exist, have the same
    /// type and not be trashed. `draft.fields` must satisfy the type's field schema
    /// ([`fields::validate_item`]; a type without a schema takes any object).
    /// `draft.body` must be a document root (`InvalidField { field: "body", .. }`, see
    /// [`blocks::is_document`]); `draft.seo` is checked and written as [`SeoPatch`]
    /// says, in the same transaction.
    pub async fn create(
        &self,
        actor: &Actor,
        draft: ContentDraft,
    ) -> Result<ContentItem, CoreError> {
        require(actor, Capability::EditOwn)?;
        // One set of rules for the whole write (R10).
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = self
            .create_in(tx.as_ref(), rules.as_ref(), actor, draft)
            .await;
        finish(tx, done).await
    }

    /// [`create`](Self::create) inside the caller's `tx` with the caller's pinned
    /// `rules`: the same validation, rows and event, and the `post.saved` action jobs.
    /// It checks **no** capability: the caller has (`create` needs `edit_own`; a media
    /// upload `upload_media`).
    pub(crate) async fn create_in(
        &self,
        tx: &dyn Tx,
        rules: &dyn Permalinks,
        actor: &Actor,
        draft: ContentDraft,
    ) -> Result<ContentItem, CoreError> {
        check_body(&draft.body)?;
        check_seo(&draft)?;
        let base = base_slug(draft.slug.as_deref(), draft.parent_id, None, || {
            slug::slugify(&draft.title)
        })?;
        let terms = dedup(&draft.terms);
        let types = tx.types().list().await?;
        check_item(
            tx,
            actor,
            &types,
            &draft.kind,
            &draft.fields,
            draft.parent_id.map(|p| (p, None)),
        )
        .await?;
        let root = root_namespace(
            tx,
            rules,
            &types,
            &draft.kind,
            draft.parent_id,
            draft.slug.is_some(),
            &base,
        )
        .await?;
        let excerpt = excerpt_of(&draft);
        let seo = draft.seo;
        let row = Row::New(NewContent {
            id: ContentId::new(),
            kind: draft.kind,
            status: ContentStatus::Draft,
            slug: String::new(),
            parent_id: draft.parent_id,
            author_id: actor.user,
            title: draft.title,
            excerpt,
            body: draft.body,
            body_html: None,
            fields: draft.fields,
            menu_order: draft.menu_order,
            published_at: None,
            scheduled_at: None,
            created_at: None,
            updated_at: None,
        });
        let saved = save_with_free_slug(tx, row, &base, None, &root).await?;
        set_terms(tx, saved.id, &terms).await?;
        if let Some(seo) = &seo {
            seo.apply(tx, saved.id).await?;
        }
        let keys = invalidation_keys(&saved, &[], &terms, None, &[]);
        after_save(tx, actor, &saved, keys, None).await?;
        Ok(saved)
    }

    /// Replaces the editable fields of item `id`, which must still be at
    /// `expected_version` (else `Conflict`). The status is kept. Own items need
    /// `edit_own` (contributors: only while draft or pending), others' `edit_others`.
    /// A trashed item is `Invalid("restore first")`. The save deletes `actor`'s own
    /// autosave of the item in the same transaction (M2b P15); other users' stay. `draft.fields` is validated as on
    /// create, also when a revision is restored, and so is `draft.seo`. `draft.body` gets
    /// the root check only when it differs from the stored body (a body the caller
    /// sends, or a restored revision's); an unchanged one is kept as stored, even one
    /// that predates the check. A write that changes
    /// only `seo` is a full update: version + 1, a revision, events, invalidation.
    ///
    /// `draft.slug = None` keeps the current slug; a slug that is taken gets `-N` as on
    /// create, and the reserved-slug and root-namespace rules of create apply (an item
    /// keeping its slug under the same parent is never refused or renamed by them, even
    /// when its stored slug is a reserved word). When the parent
    /// changes, the new one is checked as on create. When a published item's permalink changes
    /// (slug, parent, or primary category under a `%category%` structure), a `301`
    /// redirect from the old path is stored, redirects to the old path are re-targeted
    /// (no chains), and redirects from the new path (either slash variant) are deleted.
    /// When its slug or parent changes (whatever its own status), its descendants are
    /// walked breadth-first through every non-trash status, and each *published*
    /// descendant whose path changes gets the same redirect treatment. At most 1000
    /// descendants are walked (any status counts); the rest are logged and left
    /// without redirects.
    pub async fn update(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        draft: ContentDraft,
    ) -> Result<ContentItem, CoreError> {
        // One set of rules for the whole write (R10).
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = self
            .update_in(
                tx.as_ref(),
                rules.as_ref(),
                actor,
                Edit {
                    id,
                    expected_version,
                    draft,
                    note: None,
                },
            )
            .await;
        finish(tx, done).await
    }

    /// [`update`](Self::update) inside `tx`; the revision it writes carries `edit.note`.
    pub(crate) async fn update_in(
        &self,
        tx: &dyn Tx,
        permalinks: &dyn Permalinks,
        actor: &Actor,
        edit: Edit,
    ) -> Result<ContentItem, CoreError> {
        let Edit {
            id,
            expected_version,
            draft,
            note,
        } = edit;
        let current = tx.content().get(id).await?.ok_or(CoreError::NotFound)?;
        can_edit(actor, &current)?;
        if current.version != expected_version {
            return Err(CoreError::Conflict {
                current_version: current.version,
            });
        }
        if !lifecycle::editable(current.status) {
            return Err(CoreError::Invalid("restore first".into()));
        }
        if draft.kind != current.kind {
            return Err(CoreError::Invalid("the content type cannot change".into()));
        }
        // Only a body the caller sends is checked: one equal to the stored body (a PATCH
        // without `body`) is kept as stored, even when it predates the root check.
        if draft.body != current.body {
            check_body(&draft.body)?;
        }
        check_seo(&draft)?;
        // Under the same parent the item's own slug is not a conflict, and keeping it is
        // never refused or renamed, even when a later rule reserves it.
        let own = (current.parent_id == draft.parent_id).then_some(current.slug.as_str());
        let base = base_slug(draft.slug.as_deref(), draft.parent_id, own, || {
            current.slug.clone()
        })?;
        let terms = dedup(&draft.terms);
        let types = tx.types().list().await?;
        // Parent rules and the cycle walk only apply when the parent actually changes.
        let new_parent = draft
            .parent_id
            .filter(|_| draft.parent_id != current.parent_id)
            .map(|p| (p, Some(id)));
        check_item(tx, actor, &types, &current.kind, &draft.fields, new_parent).await?;
        let old_term_rows = tx.terms().for_item(id).await?;
        let old_terms: Vec<TermId> = old_term_rows.iter().map(|t| t.id).collect();

        let excerpt = excerpt_of(&draft);
        let seo = draft.seo;
        let mut next = current.clone();
        next.title = draft.title;
        next.parent_id = draft.parent_id;
        next.excerpt = excerpt;
        next.body = draft.body;
        next.body_html = None;
        next.fields = draft.fields;
        next.menu_order = draft.menu_order;
        let root = if own == Some(base.as_str()) {
            // Keeps its slug where it is: never renamed by the root rules.
            RootNamespace::default()
        } else {
            root_namespace(
                tx,
                permalinks,
                &types,
                &current.kind,
                next.parent_id,
                draft.slug.is_some(),
                &base,
            )
            .await?
        };
        let saved = save_with_free_slug(tx, Row::Existing(next), &base, own, &root).await?;

        set_terms(tx, id, &terms).await?;
        if let Some(seo) = &seo {
            seo.apply(tx, id).await?;
        }
        let mut redirected = Vec::new();
        let moved_descendants = record_moves(
            tx,
            permalinks,
            &current,
            old_term_rows,
            &saved,
            terms != old_terms,
            &mut redirected,
        )
        .await?;
        let mut keys = invalidation_keys(
            &saved,
            &old_terms,
            &terms,
            current.parent_id,
            &moved_descendants,
        );
        keys.extend(redirect_keys(redirected));
        after_save(tx, actor, &saved, keys, note).await?;
        // A real save supersedes the actor's own autosave; other users' slots stay.
        tx.autosaves().delete(id, actor.user).await?;
        Ok(saved)
    }
}

/// The redirects of an update of `current` (its terms then were `old_term_rows`) to
/// `saved` (`terms_changed`: its terms were replaced by others), as
/// [`update`](ContentService::update) describes: an item published before and after
/// the write whose path changed gets a move (a move to a path that is not live, such
/// as an import that also unpublishes it, gets none), and when its slug or parent
/// changed its published descendants do too.
/// Returns the descendants whose path changed and adds the sources of every redirect
/// touched to `redirected` (the caller emits their keys). Call it after the item's row
/// and terms are written.
pub(crate) async fn record_moves(
    tx: &dyn Repos,
    permalinks: &dyn Permalinks,
    current: &ContentItem,
    old_term_rows: Vec<Term>,
    saved: &ContentItem,
    terms_changed: bool,
    redirected: &mut Vec<String>,
) -> Result<Vec<ContentId>, CoreError> {
    let published =
        current.status == ContentStatus::Published && saved.status == ContentStatus::Published;
    let moved = current.slug != saved.slug || current.parent_id != saved.parent_id;
    if !(published || moved) {
        return Ok(Vec::new());
    }
    // The write touched only this item, so its ancestors read now are the old ones.
    let old_ancestors = ancestors(tx, current.parent_id).await?;
    let new_ancestors = if saved.parent_id == current.parent_id {
        old_ancestors.clone()
    } else {
        ancestors(tx, saved.parent_id).await?
    };
    let old_primary = primary_term(old_term_rows);
    let new_primary = if terms_changed {
        primary_term(tx.terms().for_item(saved.id).await?)
    } else {
        old_primary.clone()
    };
    let old = PermalinkInput {
        item: current.clone(),
        ancestors: old_ancestors,
        primary_term: old_primary,
    };
    let new = PermalinkInput {
        item: saved.clone(),
        ancestors: new_ancestors,
        primary_term: new_primary,
    };
    if published {
        record_item_move(tx, permalinks, &old, &new, redirected).await?;
    }
    if moved {
        return move_descendants(tx, permalinks, &old, &new, redirected).await;
    }
    Ok(Vec::new())
}

/// One [`ContentService::update_in`] call: item `id`, expected at `expected_version`,
/// gets `draft`; its revision carries `note`.
pub(crate) struct Edit {
    pub(crate) id: ContentId,
    pub(crate) expected_version: i64,
    pub(crate) draft: ContentDraft,
    pub(crate) note: Option<String>,
}

/// The validated caller slug, or `fallback()` when there is none. A caller slug that is
/// a reserved word ([`slug::RESERVED`] for a top-level item, [`slug::CHILD_RESERVED`]
/// for a child) is `InvalidField { field: "slug", .. }` ("reserved slug"); a fallback one
/// is not (the save steps around it), and neither is `own`, the item's current slug
/// under an unchanged parent.
fn base_slug(
    explicit: Option<&str>,
    parent: Option<ContentId>,
    own: Option<&str>,
    fallback: impl FnOnce() -> String,
) -> Result<String, CoreError> {
    let Some(s) = explicit else {
        return Ok(fallback());
    };
    let s = slug::validate(s)?;
    if own == Some(s.as_str()) {
        return Ok(s);
    }
    let reserved = match parent {
        None => slug::is_reserved(&s),
        Some(_) => slug::CHILD_RESERVED.contains(&s.as_str()),
    };
    if reserved {
        return Err(CoreError::invalid_field("slug", "reserved slug"));
    }
    Ok(s)
}

/// Extra slugs a top-level item may not take, beyond [`slug::RESERVED`].
#[derive(Debug, Default)]
pub(crate) struct RootNamespace {
    /// Router prefixes: content type names (`/{type}/`), taxonomy names
    /// (`/{taxonomy}/{slug}/`) and [`slug::ROUTE_WORDS`].
    names: Vec<String>,
    /// The base slug is four digits (a `/{yyyy}/` archive), so it is taken itself.
    year: bool,
    /// Types whose top-level slugs share this item's root paths.
    peers: Vec<String>,
}

/// The root-namespace rules for an item of `kind` under `parent` (none below the
/// top level). `types` is every registered type, read once by the caller for its
/// kind check too. A caller slug (`explicit`) that is a router prefix or four digits
/// is `InvalidField { field: "slug", .. }` ("reserved slug").
pub(crate) async fn root_namespace(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    types: &[ContentType],
    kind: &str,
    parent: Option<ContentId>,
    explicit: bool,
    base: &str,
) -> Result<RootNamespace, CoreError> {
    if parent.is_some() {
        return Ok(RootNamespace::default());
    }
    let mut names: Vec<String> = types.iter().map(|t| t.name.clone()).collect();
    names.extend(repos.taxonomies().list().await?.into_iter().map(|t| t.name));
    names.extend(slug::ROUTE_WORDS.iter().map(|w| (*w).to_owned()));
    let year = slug::is_year(base);
    if explicit && (year || names.iter().any(|n| n == base)) {
        return Err(CoreError::invalid_field("slug", "reserved slug"));
    }
    Ok(RootNamespace {
        names,
        year,
        peers: permalinks.root_namespace_peers(kind),
    })
}

/// The row being written: a new item or a new version of an existing one.
pub(crate) enum Row {
    New(NewContent),
    Existing(ContentItem),
}

impl Row {
    fn kind(&self) -> &str {
        match self {
            Row::New(n) => &n.kind,
            Row::Existing(i) => &i.kind,
        }
    }

    fn parent(&self) -> Option<ContentId> {
        match self {
            Row::New(n) => n.parent_id,
            Row::Existing(i) => i.parent_id,
        }
    }

    fn set_slug(&mut self, slug: String) {
        match self {
            Row::New(n) => n.slug = slug,
            Row::Existing(i) => i.slug = slug,
        }
    }

    async fn save(&self, repos: &dyn Repos) -> Result<ContentItem, StoreError> {
        match self {
            Row::New(n) => repos.content().insert(n).await,
            Row::Existing(i) => repos.content().update(i).await,
        }
    }
}

/// Writes `row` with the first free slug derived from `base`. Each attempt runs in a
/// savepoint, so a slug race (`UniqueViolation` on the slug index) poisons only that
/// attempt (R18); the next one looks the taken slugs up again. Slugs that failed are
/// remembered, so attempts make progress even where the lookup cannot see the
/// conflicting slug. `own` is the item's current slug, which does not count as taken;
/// at the top level the [`slug::RESERVED`] words always do, as do `root`'s router
/// prefixes, a four-digit base and its peer types' top-level slugs.
pub(crate) async fn save_with_free_slug(
    tx: &dyn Tx,
    mut row: Row,
    base: &str,
    own: Option<&str>,
    root: &RootNamespace,
) -> Result<ContentItem, CoreError> {
    let mut failed: Vec<String> = Vec::new();
    // The loop only exits by returning: the last failed attempt returns its error.
    loop {
        let attempt = tx.begin_nested().await?;
        let mut taken = attempt
            .content()
            .slugs_with_prefix(row.kind(), row.parent(), base)
            .await?;
        if row.parent().is_none() {
            taken.extend(slug::RESERVED.iter().map(|s| (*s).to_owned()));
            taken.extend(root.names.iter().cloned());
            if root.year {
                taken.push(base.to_owned());
            }
            for peer in &root.peers {
                taken.extend(
                    attempt
                        .content()
                        .slugs_with_prefix(peer, None, base)
                        .await?,
                );
            }
        } else {
            taken.extend(slug::CHILD_RESERVED.iter().map(|s| (*s).to_owned()));
        }
        // Last, so no rule takes the item's own slug from it.
        if let Some(own) = own {
            taken.retain(|s| s != own);
        }
        taken.extend(failed.iter().cloned());
        let candidate = slug::next_free(base, &taken);
        row.set_slug(candidate.clone());
        match row.save(attempt.as_ref()).await {
            Ok(item) => {
                attempt.commit().await?;
                return Ok(item);
            }
            Err(StoreError::UniqueViolation { constraint }) if constraint == SLUG_CONSTRAINT => {
                tracing::debug!(slug = %candidate, "slug taken concurrently; retrying");
                attempt.rollback().await?;
                failed.push(candidate);
                if failed.len() >= SLUG_ATTEMPTS {
                    return Err(StoreError::UniqueViolation { constraint }.into());
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Refuses a relation value naming an item whose type its field does not list
/// (M2b-3 P52), as `InvalidKey` at the value's path (`fields.chapters[1].related`). The
/// items are read in one `get_many` per [`MAX_ID_FILTER`] distinct ids (one for any
/// usual item); a missing item is accepted, as [`fields`] says, and so is one `actor`
/// cannot read (M4-4 P77, [`readable_target`]), so the answer never tells whether
/// someone else's draft exists or what type it has.
async fn check_relations(
    repos: &dyn Repos,
    actor: &Actor,
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> Result<(), CoreError> {
    let refs = fields::relation_refs(schema, value)?;
    if refs.is_empty() {
        return Ok(());
    }
    let mut ids: Vec<ContentId> = refs.iter().map(|r| r.id).collect();
    ids.sort_unstable();
    ids.dedup();
    let now = Timestamp::now();
    let mut kinds = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(MAX_ID_FILTER) {
        for item in repos.content().get_many(chunk).await? {
            if readable_target(actor, &item, now) {
                kinds.insert(item.id, item.kind);
            }
        }
    }
    for r in refs {
        if let Some(kind) = kinds.get(&r.id)
            && !r.types.contains(kind)
        {
            return Err(CoreError::invalid_key(
                r.path.clone(),
                format!(
                    "{}: must be an item of type {}",
                    r.path,
                    r.types.join(" or ")
                ),
            ));
        }
    }
    Ok(())
}

/// Revision snapshot (with `note`), prune, search index, the outbox event and the
/// `post.saved` action jobs (P20) for a saved item.
async fn after_save(
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
    keys: Vec<String>,
    note: Option<String>,
) -> Result<(), CoreError> {
    write_revision(repos, actor, item, note).await?;
    index_search(repos, item).await?;
    repos.events().append(&events::invalidate(keys)).await?;
    let id = item.id.to_string();
    actions::enqueue(
        repos,
        actions::POST_SAVED,
        Some(id.clone()),
        serde_json::json!({ "post": id }),
    )
    .await?;
    Ok(())
}

/// The search entry of a saved `item` (title and body text). Every content write indexes
/// through this; `ferrite import-site` and `db copy` rebuild the index with it (M4-1 P2).
pub async fn index_search(repos: &dyn Repos, item: &ContentItem) -> Result<(), CoreError> {
    repos
        .search()
        .index(item.id, &item.title, &blocks::extract_text(&item.body))
        .await?;
    Ok(())
}

/// A revision of a saved `item` by `actor` (with its meta as it is now, and `note`),
/// then the prune to the newest 50.
pub(crate) async fn write_revision(
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
    note: Option<String>,
) -> Result<(), CoreError> {
    let meta: serde_json::Map<String, serde_json::Value> =
        repos.meta().list(item.id).await?.into_iter().collect();
    repos
        .revisions()
        .insert(&Revision {
            id: RevisionId::new(),
            item_id: item.id,
            author_id: actor.user,
            created_at: Timestamp::now(),
            title: item.title.clone(),
            excerpt: item.excerpt.clone(),
            body: item.body.clone(),
            fields: item.fields.clone(),
            meta: serde_json::Value::Object(meta),
            note,
        })
        .await?;
    repos.revisions().prune(item.id, KEEP_REVISIONS).await?;
    Ok(())
}

/// `post:<id>`, `archive:<kind>`, `term:<id>` for the union of the old and the new
/// terms, `post:<parent>` for the old and the new parent (whichever are set), and
/// `post:<id>` for each descendant whose URL moved. `old_parent` is `None` on create.
pub(crate) fn invalidation_keys(
    item: &ContentItem,
    old_terms: &[TermId],
    new_terms: &[TermId],
    old_parent: Option<ContentId>,
    moved_descendants: &[ContentId],
) -> Vec<String> {
    let terms: BTreeSet<TermId> = old_terms.iter().chain(new_terms).copied().collect();
    [events::post_key(item.id), events::archive_key(&item.kind)]
        .into_iter()
        .chain(terms.into_iter().map(events::term_key))
        .chain(old_parent.into_iter().map(events::post_key))
        .chain(item.parent_id.map(events::post_key))
        .chain(moved_descendants.iter().copied().map(events::post_key))
        .collect()
}

/// Records a move when the paths of `old` and `new` (the same item) differ, adding the
/// sources of the redirects touched to `redirected`; returns whether they did.
async fn record_item_move(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    old: &PermalinkInput,
    new: &PermalinkInput,
    redirected: &mut Vec<String>,
) -> Result<bool, CoreError> {
    let (old_path, new_path) = (permalinks.path(old), permalinks.path(new));
    if old_path == new_path {
        return Ok(false);
    }
    redirected.extend(move_redirects(repos, &old_path, &new_path).await?);
    Ok(true)
}

/// Records moves for the published descendants of an item that went from `old` to
/// `new` (slug or parent changed; the item's own status does not matter).
/// Breadth-first through children in every non-trash status (a draft page can have
/// published children whose URLs contain its slug); only published descendants get a
/// move. At most [`MAX_DESCENDANTS`] nodes are walked, whatever their status.
/// Returns the ids of the descendants whose path changed; the sources of the redirects
/// touched are added to `redirected`.
async fn move_descendants(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    old: &PermalinkInput,
    new: &PermalinkInput,
    redirected: &mut Vec<String>,
) -> Result<Vec<ContentId>, CoreError> {
    let chain = |input: &PermalinkInput| {
        let mut c = input.ancestors.clone();
        c.push((input.item.id, input.item.slug.clone()));
        c
    };
    let mut queue = VecDeque::from([(old.item.id, chain(old), chain(new))]);
    let mut walked = 0;
    let mut moved = Vec::new();
    while let Some((parent, old_chain, new_chain)) = queue.pop_front() {
        let mut cursor = None;
        loop {
            let page = repos
                .content()
                .list(&ContentQuery {
                    parent: ParentFilter::Of(parent),
                    statuses: WALKED.to_vec(),
                    limit: DESCENDANT_BATCH,
                    cursor,
                    ..ContentQuery::default()
                })
                .await?;
            let ids: Vec<ContentId> = page.items.iter().map(|i| i.id).collect();
            let mut terms = if ids.is_empty() {
                Default::default()
            } else {
                repos.terms().for_items(&ids).await?
            };
            for child in page.items {
                if walked >= MAX_DESCENDANTS {
                    tracing::warn!(
                        item = %old.item.id,
                        limit = MAX_DESCENDANTS,
                        "descendant walk capped; remaining descendants get no redirects"
                    );
                    return Ok(moved);
                }
                walked += 1;
                let primary = primary_term(terms.remove(&child.id).unwrap_or_default());
                let child_old = PermalinkInput {
                    item: child,
                    ancestors: old_chain.clone(),
                    primary_term: primary,
                };
                let child_new = PermalinkInput {
                    ancestors: new_chain.clone(),
                    ..child_old.clone()
                };
                if child_old.item.status == ContentStatus::Published
                    && record_item_move(repos, permalinks, &child_old, &child_new, redirected)
                        .await?
                {
                    moved.push(child_old.item.id);
                }
                queue.push_back((child_old.item.id, chain(&child_old), chain(&child_new)));
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
    }
    Ok(moved)
}

/// The checks every content write by `actor` runs on an item of `kind`: its type is
/// registered (`types` is every registered type), `fields` satisfies the type's schema
/// and its relation targets (those `actor` may read, [`check_relations`]), and
/// `new_parent`, when given as (parent, the item itself for an existing one), passes
/// [`check_parent`].
pub(crate) async fn check_item(
    repos: &dyn Repos,
    actor: &Actor,
    types: &[ContentType],
    kind: &str,
    fields: &serde_json::Value,
    new_parent: Option<(ContentId, Option<ContentId>)>,
) -> Result<(), CoreError> {
    let ty = content_type(types, kind)?;
    fields::validate_item(&ty.fields, fields)?;
    check_relations(repos, actor, &ty.fields, fields).await?;
    if let Some((parent, child)) = new_parent {
        check_parent(repos, actor, ty, child, parent).await?;
    }
    Ok(())
}

/// Whether `actor` may see that `item` exists when it names it in a write (a relation
/// target, a parent): [`can_read`] at `now`, and media only with `media:read`, the
/// rule of `ContentService::refs` (M4-4 P77).
fn readable_target(actor: &Actor, item: &ContentItem, now: Timestamp) -> bool {
    can_read(actor, item, now)
        && (item.kind != MEDIA_KIND || require_read(actor, Area::Media).is_ok())
}

/// Type `kind` from `types` (every registered type).
fn content_type<'a>(types: &'a [ContentType], kind: &str) -> Result<&'a ContentType, CoreError> {
    types
        .iter()
        .find(|t| t.name == kind)
        .ok_or_else(|| CoreError::Invalid(format!("unknown content type {kind:?}")))
}

/// A parent is allowed only for a hierarchical type `ty`; it must exist, be of the same
/// type and not be trashed. One `actor` cannot read ([`readable_target`]) is an
/// "unknown parent", exactly as a missing one, so the answer never tells whether
/// someone else's draft exists or what type it has (M4-4 P77). For an existing item
/// (`child`), it must not be the item itself or one of its descendants.
async fn check_parent(
    repos: &dyn Repos,
    actor: &Actor,
    ty: &ContentType,
    child: Option<ContentId>,
    parent: ContentId,
) -> Result<(), CoreError> {
    if !ty.hierarchical {
        return Err(CoreError::invalid_field(
            "parent_id",
            format!(
                "content type {:?} is not hierarchical; items cannot have a parent",
                ty.name
            ),
        ));
    }
    let Some(mut item) = repos
        .content()
        .get(parent)
        .await?
        .filter(|item| readable_target(actor, item, Timestamp::now()))
    else {
        return Err(CoreError::invalid_field("parent_id", "unknown parent"));
    };
    if item.kind != ty.name {
        return Err(CoreError::invalid_field(
            "parent_id",
            format!("parent must be of type {:?}", ty.name),
        ));
    }
    if item.status == ContentStatus::Trash {
        return Err(CoreError::invalid_field("parent_id", "parent is trashed"));
    }
    let Some(child) = child else {
        return Ok(());
    };
    for _ in 0..MAX_PARENT_DEPTH {
        if item.id == child {
            return Err(CoreError::invalid_field(
                "parent_id",
                "parent would create a cycle",
            ));
        }
        let Some(up) = item.parent_id else {
            return Ok(());
        };
        item = repos
            .content()
            .get(up)
            .await?
            .ok_or_else(|| CoreError::invalid_field("parent_id", "unknown parent"))?;
    }
    Err(CoreError::invalid_field(
        "parent_id",
        format!("parent chain deeper than {MAX_PARENT_DEPTH}"),
    ))
}

pub(crate) async fn set_terms(
    repos: &dyn Repos,
    item: ContentId,
    terms: &[TermId],
) -> Result<(), CoreError> {
    match repos.terms().set_for_item(item, terms).await {
        Err(StoreError::ForeignKey { .. }) => {
            Err(CoreError::invalid_field("terms", "unknown term"))
        }
        other => Ok(other?),
    }
}

/// [`SeoPatch::check`] of `draft.seo`, if any.
fn check_seo(draft: &ContentDraft) -> Result<(), CoreError> {
    draft.seo.as_ref().map_or(Ok(()), SeoPatch::check)
}

/// `InvalidField { field: "body", .. }` unless `body` is a document root
/// ([`blocks::is_document`]).
pub(crate) fn check_body(body: &serde_json::Value) -> Result<(), CoreError> {
    if blocks::is_document(body) {
        Ok(())
    } else {
        Err(CoreError::invalid_field(
            "body",
            "the body must be a document (type doc)",
        ))
    }
}

fn excerpt_of(draft: &ContentDraft) -> String {
    draft
        .excerpt
        .clone()
        .unwrap_or_else(|| blocks::excerpt(&draft.body, EXCERPT_CHARS))
}

pub(crate) fn dedup(terms: &[TermId]) -> Vec<TermId> {
    let mut out = Vec::with_capacity(terms.len());
    for t in terms {
        if !out.contains(t) {
            out.push(*t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrite_domain::UserId;
    use serde_json::json;

    fn item(parent: Option<ContentId>) -> ContentItem {
        let now = Timestamp::now();
        ContentItem {
            id: ContentId::new(),
            kind: "page".into(),
            status: ContentStatus::Draft,
            slug: "s".into(),
            parent_id: parent,
            author_id: UserId::new(),
            title: String::new(),
            excerpt: String::new(),
            body: json!({}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: None,
            scheduled_at: None,
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    #[test]
    fn keys_cover_term_parent_and_descendant_changes() {
        let (p1, p2, d) = (ContentId::new(), ContentId::new(), ContentId::new());
        let (a, b, c) = (TermId::new(), TermId::new(), TermId::new());
        let it = item(Some(p2));
        let got: BTreeSet<String> = invalidation_keys(&it, &[a, b], &[b, c], Some(p1), &[d])
            .into_iter()
            .collect();
        let want: BTreeSet<String> = [
            events::post_key(it.id),
            events::archive_key("page"),
            events::term_key(a),
            events::term_key(b),
            events::term_key(c),
            events::post_key(p1),
            events::post_key(p2),
            events::post_key(d),
        ]
        .into_iter()
        .collect();
        assert_eq!(got, want);

        // Unchanged terms and parent: still named, the parent lists its children.
        let it = item(Some(p1));
        // (`events::invalidate` drops the duplicate parent key.)
        let got: BTreeSet<String> = invalidation_keys(&it, &[a], &[a], Some(p1), &[])
            .into_iter()
            .collect();
        assert_eq!(got.len(), 4, "{got:?}");
        // Create under a parent: the parent is invalidated.
        assert!(invalidation_keys(&it, &[], &[], None, &[]).contains(&events::post_key(p1)));
    }

    #[test]
    fn dedup_keeps_first_order() {
        let (a, b) = (TermId::new(), TermId::new());
        assert_eq!(dedup(&[b, a, b, a]), [b, a]);
    }
}
