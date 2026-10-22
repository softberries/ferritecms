//! Content create/update: slugs, terms, revisions, search, redirects and outbox events,
//! all in one transaction per call.

use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};

use ferrite_domain::{
    ContentId, ContentItem, ContentQuery, ContentStatus, ContentType, NewContent, ParentFilter,
    Redirect, RedirectId, RedirectOrigin, Revision, RevisionId, TermId, Timestamp,
};
use ferrite_store::{MAX_REDIRECTS_BY_TARGET, Repos, Store, StoreError, Tx};

use crate::{
    Actor, CoreError,
    actor::{Capability, can_edit, require},
    blocks, events,
    permalink::{PermalinkInput, Permalinks, ancestors, load_input, primary_term},
    redirects::toggled,
    slug,
    tx::finish,
};

mod lifecycle;

/// Unique index on (type, parent, slug); a violation of it means another writer took
/// the slug between our lookup and our write.
const SLUG_CONSTRAINT: &str = "content_items_slug_uq";
const SLUG_ATTEMPTS: usize = 5;
const KEEP_REVISIONS: u32 = 50;
const EXCERPT_CHARS: usize = 160;
/// Longest parent chain walked when checking a new parent for cycles.
const MAX_PARENT_DEPTH: usize = 64;
/// Most descendants re-pointed when a published item moves.
const MAX_DESCENDANTS: usize = 1000;
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
    /// to 5 times. A top-level item may not take a [`slug::RESERVED`] word: a caller
    /// slug is `Invalid("reserved slug")`, a generated one gets the `-N` suffix.
    ///
    /// A parent is allowed only for hierarchical types; it must exist, have the same
    /// type and not be trashed.
    pub async fn create(
        &self,
        actor: &Actor,
        draft: ContentDraft,
    ) -> Result<ContentItem, CoreError> {
        require(actor, Capability::EditOwn)?;
        let base = base_slug(draft.slug.as_deref(), draft.parent_id, || {
            slug::slugify(&draft.title)
        })?;
        let terms = dedup(&draft.terms);
        let tx = self.store.begin().await?;
        let done = async {
            let ty = content_type(tx.as_ref(), &draft.kind).await?;
            if let Some(parent) = draft.parent_id {
                check_parent(tx.as_ref(), &ty, None, parent).await?;
            }
            let excerpt = excerpt_of(&draft);
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
            });
            let saved = save_with_free_slug(tx.as_ref(), row, &base, None).await?;
            set_terms(tx.as_ref(), saved.id, &terms).await?;
            let keys = invalidation_keys(&saved, &[], &terms, None);
            after_save(tx.as_ref(), actor, &saved, keys).await?;
            Ok(saved)
        }
        .await;
        finish(tx, done).await
    }

    /// Replaces the editable fields of item `id`, which must still be at
    /// `expected_version` (else `Conflict`). The status is kept. Own items need
    /// `edit_own` (contributors: only while draft or pending), others' `edit_others`.
    /// A trashed item is `Invalid("restore first")`.
    ///
    /// `draft.slug = None` keeps the current slug; a slug that is taken gets `-N` as on
    /// create, and the [`slug::RESERVED`] rules of create apply. When the parent changes,
    /// the new one is checked as on create. When a published item's permalink changes
    /// (slug, parent, or primary category under a `%category%` structure), a `301`
    /// redirect from the old path is stored, redirects to the old path are re-targeted
    /// (no chains), and redirects from the new path (either slash variant) are deleted.
    /// When its slug or parent changes, the same is done for each published descendant
    /// whose path changes (walked breadth-first through published children only, at most
    /// 1000; the rest are logged and left without redirects).
    pub async fn update(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        draft: ContentDraft,
    ) -> Result<ContentItem, CoreError> {
        let tx = self.store.begin().await?;
        let done = self
            .update_in(tx.as_ref(), actor, id, expected_version, draft)
            .await;
        finish(tx, done).await
    }

    async fn update_in(
        &self,
        tx: &dyn Tx,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        draft: ContentDraft,
    ) -> Result<ContentItem, CoreError> {
        let current = tx.content().get(id).await?.ok_or(CoreError::NotFound)?;
        can_edit(actor, &current)?;
        if current.version != expected_version {
            return Err(CoreError::Conflict {
                current_version: current.version,
            });
        }
        if current.status == ContentStatus::Trash {
            return Err(CoreError::Invalid("restore first".into()));
        }
        if draft.kind != current.kind {
            return Err(CoreError::Invalid("the content type cannot change".into()));
        }
        let base = base_slug(draft.slug.as_deref(), draft.parent_id, || {
            current.slug.clone()
        })?;
        let terms = dedup(&draft.terms);
        // Parent rules and the cycle walk only apply when the parent actually changes.
        if let Some(parent) = draft.parent_id
            && draft.parent_id != current.parent_id
        {
            let ty = content_type(tx, &current.kind).await?;
            check_parent(tx, &ty, Some(id), parent).await?;
        }
        let old_term_rows = tx.terms().for_item(id).await?;
        let old_terms: Vec<TermId> = old_term_rows.iter().map(|t| t.id).collect();
        // Only a published item's old path matters (redirects); load it before writing.
        let old_input = if current.status == ContentStatus::Published {
            Some(PermalinkInput {
                item: current.clone(),
                ancestors: ancestors(tx, current.parent_id).await?,
                primary_term: primary_term(old_term_rows),
            })
        } else {
            None
        };

        let excerpt = excerpt_of(&draft);
        let mut next = current.clone();
        next.title = draft.title;
        next.parent_id = draft.parent_id;
        next.excerpt = excerpt;
        next.body = draft.body;
        next.body_html = None;
        next.fields = draft.fields;
        next.menu_order = draft.menu_order;
        // Under the same parent the item's own slug is not a conflict.
        let own = (current.parent_id == next.parent_id).then_some(current.slug.as_str());
        let saved = save_with_free_slug(tx, Row::Existing(next), &base, own).await?;

        set_terms(tx, id, &terms).await?;
        if let Some(old) = old_input {
            let ancestors = if saved.parent_id == current.parent_id {
                old.ancestors.clone()
            } else {
                ancestors(tx, saved.parent_id).await?
            };
            let primary_term = if terms == old_terms {
                old.primary_term.clone()
            } else {
                primary_term(tx.terms().for_item(id).await?)
            };
            let new = PermalinkInput {
                item: saved.clone(),
                ancestors,
                primary_term,
            };
            record_item_move(tx, self.permalinks.as_ref(), &old, &new).await?;
            if current.slug != saved.slug || current.parent_id != saved.parent_id {
                move_descendants(tx, self.permalinks.as_ref(), &old, &new).await?;
            }
        }
        let keys = invalidation_keys(&saved, &old_terms, &terms, Some(current.parent_id));
        after_save(tx, actor, &saved, keys).await?;
        Ok(saved)
    }
}

/// The validated caller slug, or `fallback()` when there is none. A caller slug that is
/// a reserved word is `Invalid` for a top-level item; a fallback one is not (the save
/// steps around it).
fn base_slug(
    explicit: Option<&str>,
    parent: Option<ContentId>,
    fallback: impl FnOnce() -> String,
) -> Result<String, CoreError> {
    let Some(s) = explicit else {
        return Ok(fallback());
    };
    let s = slug::validate(s)?;
    if parent.is_none() && slug::is_reserved(&s) {
        return Err(CoreError::Invalid("reserved slug".into()));
    }
    Ok(s)
}

/// The row being written: a new item or a new version of an existing one.
enum Row {
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
/// at the top level the [`slug::RESERVED`] words always do.
async fn save_with_free_slug(
    tx: &dyn Tx,
    mut row: Row,
    base: &str,
    own: Option<&str>,
) -> Result<ContentItem, CoreError> {
    let mut failed: Vec<String> = Vec::new();
    // The loop only exits by returning: the last failed attempt returns its error.
    loop {
        let attempt = tx.begin_nested().await?;
        let mut taken = attempt
            .content()
            .slugs_with_prefix(row.kind(), row.parent(), base)
            .await?;
        if let Some(own) = own {
            taken.retain(|s| s != own);
        }
        if row.parent().is_none() {
            taken.extend(slug::RESERVED.iter().map(|s| (*s).to_owned()));
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

/// Revision snapshot, prune, search index and the outbox event for a saved item.
async fn after_save(
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
    keys: Vec<String>,
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
            note: None,
        })
        .await?;
    repos.revisions().prune(item.id, KEEP_REVISIONS).await?;
    repos
        .search()
        .index(item.id, &item.title, &blocks::extract_text(&item.body))
        .await?;
    repos.events().append(&events::invalidate(keys)).await?;
    Ok(())
}

/// `post:<id>`, `archive:<kind>`, `term:<id>` for every term added or removed, and
/// `post:<parent>` for the old and new parent when the parent changed. `old_parent` is
/// `None` on create (every parent counts as new).
fn invalidation_keys(
    item: &ContentItem,
    old_terms: &[TermId],
    new_terms: &[TermId],
    old_parent: Option<Option<ContentId>>,
) -> Vec<String> {
    let mut keys = vec![events::post_key(item.id), events::archive_key(&item.kind)];
    let old: BTreeSet<TermId> = old_terms.iter().copied().collect();
    let new: BTreeSet<TermId> = new_terms.iter().copied().collect();
    keys.extend(old.symmetric_difference(&new).map(|t| events::term_key(*t)));
    let old_parent = old_parent.flatten();
    if old_parent != item.parent_id {
        keys.extend(
            old_parent
                .into_iter()
                .chain(item.parent_id)
                .map(events::post_key),
        );
    }
    keys
}

/// Deletes the redirects that start at `path` or its slash-toggled variant: `path` is
/// a live page now, and [`RedirectService::resolve`](crate::RedirectService::resolve)
/// matches either variant.
pub(crate) async fn clear_redirects_from(repos: &dyn Repos, path: &str) -> Result<(), CoreError> {
    let redirects = repos.redirects();
    for source in std::iter::once(path.to_owned()).chain(toggled(path)) {
        if let Some(r) = redirects.get_by_source(&source).await? {
            redirects.delete(r.id).await?;
        }
    }
    Ok(())
}

/// Records a move when the paths of `old` and `new` (the same item) differ.
async fn record_item_move(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    old: &PermalinkInput,
    new: &PermalinkInput,
) -> Result<(), CoreError> {
    let (old_path, new_path) = (permalinks.path(old), permalinks.path(new));
    if old_path != new_path {
        record_move(repos, &old_path, &new_path).await?;
    }
    Ok(())
}

/// Records moves for the published descendants of an item that went from `old` to
/// `new` (slug or parent changed). Breadth-first through published children, at most
/// [`MAX_DESCENDANTS`]; a draft child and everything below it is skipped.
async fn move_descendants(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    old: &PermalinkInput,
    new: &PermalinkInput,
) -> Result<(), CoreError> {
    let chain = |input: &PermalinkInput| {
        let mut c = input.ancestors.clone();
        c.push((input.item.id, input.item.slug.clone()));
        c
    };
    let mut queue = VecDeque::from([(old.item.id, chain(old), chain(new))]);
    let mut walked = 0;
    while let Some((parent, old_chain, new_chain)) = queue.pop_front() {
        let mut cursor = None;
        loop {
            let page = repos
                .content()
                .list(&ContentQuery {
                    parent: ParentFilter::Of(parent),
                    statuses: vec![ContentStatus::Published],
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
                    return Ok(());
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
                record_item_move(repos, permalinks, &child_old, &child_new).await?;
                queue.push_back((child_old.item.id, chain(&child_old), chain(&child_new)));
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
    }
    Ok(())
}

/// Repoints redirects after a published item moved from `old` to `new`: no redirect may
/// start at `new` or its slash variant (it is a live page again), redirects to `old` now
/// go to `new` (no chains; one that would start at `new` is dropped rather than left as
/// a self-loop), and `old` redirects to `new` with a `301`.
async fn record_move(repos: &dyn Repos, old: &str, new: &str) -> Result<(), CoreError> {
    clear_redirects_from(repos, new).await?;
    let ours: Vec<String> = std::iter::once(new.to_owned())
        .chain(toggled(new))
        .collect();
    let redirects = repos.redirects();
    // Each pass re-targets (or drops) what it read, so the next pass sees the rest.
    loop {
        let batch = redirects.list_by_target(old).await?;
        let full = batch.len() >= MAX_REDIRECTS_BY_TARGET as usize;
        for mut r in batch {
            if ours.contains(&r.source_path) {
                redirects.delete(r.id).await?;
            } else {
                r.target = new.to_owned();
                redirects.upsert(&r).await?;
            }
        }
        if !full {
            break;
        }
    }
    redirects
        .upsert(&Redirect {
            id: RedirectId::new(),
            source_path: old.to_owned(),
            target: new.to_owned(),
            code: 301,
            hits: 0,
            origin: RedirectOrigin::SlugChange,
            created_at: Timestamp::now(),
            last_hit_at: None,
        })
        .await?;
    Ok(())
}

async fn content_type(repos: &dyn Repos, kind: &str) -> Result<ContentType, CoreError> {
    repos
        .types()
        .get(kind)
        .await?
        .ok_or_else(|| CoreError::Invalid(format!("unknown content type {kind:?}")))
}

/// A parent is allowed only for a hierarchical type `ty`; it must exist, be of the same
/// type and not be trashed. For an existing item (`child`), it must not be the item
/// itself or one of its descendants.
async fn check_parent(
    repos: &dyn Repos,
    ty: &ContentType,
    child: Option<ContentId>,
    parent: ContentId,
) -> Result<(), CoreError> {
    if !ty.hierarchical {
        return Err(CoreError::Invalid(format!(
            "content type {:?} is not hierarchical; items cannot have a parent",
            ty.name
        )));
    }
    let Some(mut item) = repos.content().get(parent).await? else {
        return Err(CoreError::Invalid("unknown parent".into()));
    };
    if item.kind != ty.name {
        return Err(CoreError::Invalid(format!(
            "parent must be of type {:?}",
            ty.name
        )));
    }
    if item.status == ContentStatus::Trash {
        return Err(CoreError::Invalid("parent is trashed".into()));
    }
    let Some(child) = child else {
        return Ok(());
    };
    for _ in 0..MAX_PARENT_DEPTH {
        if item.id == child {
            return Err(CoreError::Invalid("parent would create a cycle".into()));
        }
        let Some(up) = item.parent_id else {
            return Ok(());
        };
        item = repos
            .content()
            .get(up)
            .await?
            .ok_or_else(|| CoreError::Invalid("unknown parent".into()))?;
    }
    Err(CoreError::Invalid(format!(
        "parent chain deeper than {MAX_PARENT_DEPTH}"
    )))
}

async fn set_terms(repos: &dyn Repos, item: ContentId, terms: &[TermId]) -> Result<(), CoreError> {
    match repos.terms().set_for_item(item, terms).await {
        Err(StoreError::ForeignKey { .. }) => Err(CoreError::Invalid("unknown term".into())),
        other => Ok(other?),
    }
}

fn excerpt_of(draft: &ContentDraft) -> String {
    draft
        .excerpt
        .clone()
        .unwrap_or_else(|| blocks::excerpt(&draft.body, EXCERPT_CHARS))
}

fn dedup(terms: &[TermId]) -> Vec<TermId> {
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
    fn keys_cover_term_and_parent_changes() {
        let (p1, p2) = (ContentId::new(), ContentId::new());
        let (a, b, c) = (TermId::new(), TermId::new(), TermId::new());
        let it = item(Some(p2));
        let keys = invalidation_keys(&it, &[a, b], &[b, c], Some(Some(p1)));
        let mut want = vec![
            events::post_key(it.id),
            events::archive_key("page"),
            events::post_key(p1),
            events::post_key(p2),
        ];
        let mut terms = vec![events::term_key(a), events::term_key(c)];
        terms.sort();
        want.splice(2..2, terms);
        let mut got = keys.clone();
        got.sort();
        want.sort();
        assert_eq!(got, want);

        // Unchanged parent and terms: just the item and its archive.
        let it = item(Some(p1));
        assert_eq!(
            invalidation_keys(&it, &[a], &[a], Some(Some(p1))),
            [events::post_key(it.id), events::archive_key("page")]
        );
        // Create under a parent: the parent is invalidated.
        assert!(invalidation_keys(&it, &[], &[], None).contains(&events::post_key(p1)));
    }

    #[test]
    fn dedup_keeps_first_order() {
        let (a, b) = (TermId::new(), TermId::new());
        assert_eq!(dedup(&[b, a, b, a]), [b, a]);
    }
}
