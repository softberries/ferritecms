//! Content create/update: slugs, terms, revisions, search, redirects and outbox events,
//! all in one transaction per call.

use std::{collections::BTreeSet, sync::Arc};

use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, NewContent, Redirect, RedirectId, RedirectOrigin,
    Revision, RevisionId, TermId, Timestamp,
};
use ferrite_store::{MAX_REDIRECTS_BY_TARGET, Repos, Store, StoreError, Tx};

use crate::{
    Actor, CoreError,
    actor::{Capability, can_edit, require},
    blocks, events, slug,
};

mod lifecycle;

/// Maps an item to its public URL path (e.g. `/post/hello-world`). Supplied by the caller
/// so core stays independent of routing; it must be pure and cheap (it runs inside the
/// write transaction).
pub type PermalinkFn = dyn Fn(&ContentItem) -> String + Send + Sync;

/// Unique index on (type, parent, slug); a violation of it means another writer took
/// the slug between our lookup and our write.
const SLUG_CONSTRAINT: &str = "content_items_slug_uq";
const SLUG_ATTEMPTS: usize = 5;
const KEEP_REVISIONS: u32 = 50;
const EXCERPT_CHARS: usize = 160;
/// Longest parent chain walked when checking a new parent for cycles.
const MAX_PARENT_DEPTH: usize = 64;

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
    permalink: Arc<PermalinkFn>,
}

impl ContentService {
    pub fn new(store: Arc<dyn Store>, permalink: Arc<PermalinkFn>) -> Self {
        Self { store, permalink }
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
    /// to 5 times.
    pub async fn create(
        &self,
        actor: &Actor,
        draft: ContentDraft,
    ) -> Result<ContentItem, CoreError> {
        require(actor, Capability::EditOwn)?;
        let base = match &draft.slug {
            Some(s) => slug::validate(s)?,
            None => slug::slugify(&draft.title),
        };
        let terms = dedup(&draft.terms);
        let tx = self.store.begin().await?;
        check_type(tx.as_ref(), &draft.kind).await?;
        if let Some(parent) = draft.parent_id {
            check_parent(tx.as_ref(), None, parent).await?;
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
        tx.commit().await?;
        Ok(saved)
    }

    /// Replaces the editable fields of item `id`, which must still be at
    /// `expected_version` (else `Conflict`). The status is kept. Own items need
    /// `edit_own` (contributors: only while draft or pending), others' `edit_others`.
    ///
    /// `draft.slug = None` keeps the current slug; a slug that is taken gets `-N` as on
    /// create. When a published item's permalink changes, a `301` redirect from the old
    /// path is stored, redirects to the old path are re-targeted (no chains), and a
    /// redirect from the new path is deleted.
    pub async fn update(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        draft: ContentDraft,
    ) -> Result<ContentItem, CoreError> {
        let tx = self.store.begin().await?;
        let current = tx.content().get(id).await?.ok_or(CoreError::NotFound)?;
        can_edit(actor, &current)?;
        if current.version != expected_version {
            return Err(CoreError::Conflict {
                current_version: current.version,
            });
        }
        if draft.kind != current.kind {
            return Err(CoreError::Invalid("the content type cannot change".into()));
        }
        let base = match &draft.slug {
            Some(s) => slug::validate(s)?,
            None => current.slug.clone(),
        };
        let terms = dedup(&draft.terms);
        // The cycle walk is only needed when the parent actually changes.
        if let Some(parent) = draft.parent_id
            && draft.parent_id != current.parent_id
        {
            check_parent(tx.as_ref(), Some(id), parent).await?;
        }
        let old_terms: Vec<TermId> = tx
            .terms()
            .for_item(id)
            .await?
            .into_iter()
            .map(|t| t.id)
            .collect();

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
        let saved = save_with_free_slug(tx.as_ref(), Row::Existing(next), &base, own).await?;

        set_terms(tx.as_ref(), id, &terms).await?;
        if current.status == ContentStatus::Published {
            let (old_path, new_path) = ((self.permalink)(&current), (self.permalink)(&saved));
            if old_path != new_path {
                record_move(tx.as_ref(), &old_path, &new_path).await?;
            }
        }
        let keys = invalidation_keys(&saved, &old_terms, &terms, Some(current.parent_id));
        after_save(tx.as_ref(), actor, &saved, keys).await?;
        tx.commit().await?;
        Ok(saved)
    }
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
/// conflicting slug. `own` is the item's current slug, which does not count as taken.
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

/// Repoints redirects after a published item moved from `old` to `new`: no redirect may
/// start at `new` (it is a live page again), redirects to `old` now go to `new` (no
/// chains), and `old` redirects to `new` with a `301`.
async fn record_move(repos: &dyn Repos, old: &str, new: &str) -> Result<(), CoreError> {
    let redirects = repos.redirects();
    if let Some(r) = redirects.get_by_source(new).await? {
        redirects.delete(r.id).await?;
    }
    // Each pass re-targets what it read, so the next pass sees the rest.
    loop {
        let batch = redirects.list_by_target(old).await?;
        let full = batch.len() >= MAX_REDIRECTS_BY_TARGET as usize;
        for mut r in batch {
            r.target = new.to_owned();
            redirects.upsert(&r).await?;
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

async fn check_type(repos: &dyn Repos, kind: &str) -> Result<(), CoreError> {
    match repos.types().get(kind).await? {
        Some(_) => Ok(()),
        None => Err(CoreError::Invalid(format!("unknown content type {kind:?}"))),
    }
}

/// The parent must exist; for an existing item (`child`), it must not be the item itself
/// or one of its descendants.
async fn check_parent(
    repos: &dyn Repos,
    child: Option<ContentId>,
    parent: ContentId,
) -> Result<(), CoreError> {
    let Some(mut item) = repos.content().get(parent).await? else {
        return Err(CoreError::Invalid("unknown parent".into()));
    };
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
