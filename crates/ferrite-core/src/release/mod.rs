//! Releases (design 15 §4–§5): named sets of staged changes applied in one transaction,
//! now or on a schedule, and the sandbox the whole-site preview renders through.
//!
//! Visibility and rights (§4.7): a release is seen by its creator and by `edit_others`;
//! creating needs `edit_own`; staging an entry needs the item's edit right and, for a
//! transition, its publish right; applying and scheduling run every entry's own checks
//! as the actor.

mod apply;
mod sandbox;
mod staged;
mod tx;

use std::sync::Arc;

use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, EntryTransition, MAX_RELEASE_ENTRIES, Page,
    PreviewTarget, Release, ReleaseEntry, ReleaseId, ReleaseQuery, ReleaseStatus, StagedChange,
    Timestamp,
};
use ferrite_store::{Repos, Store};

pub(crate) use apply::unschedule_in;
pub use apply::{EntryCheck, EntryOutcome, ReleaseCheck};
pub use sandbox::{SandboxReport, SandboxStore};
pub use tx::{ApplyTx, MAX_RELEASE_KEYS, Mode};

use crate::{
    Actor, ContentService, CoreError,
    actor::{Area, Capability, can, can_edit, require, require_read},
    permalink::Permalinks,
    tx::finish,
    workflow::{ReviewSummary, gate},
};

/// Longest release title.
pub const MAX_RELEASE_TITLE: usize = 200;
/// Longest release description.
pub const MAX_RELEASE_DESCRIPTION: usize = 2000;

/// A new release.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewRelease {
    pub title: String,
    pub description: String,
}

/// One entry to add or replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInput {
    pub item: ContentId,
    pub transition: EntryTransition,
    pub changes: Option<StagedChange>,
}

/// An entry with its item as stored now.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryView {
    pub entry: ReleaseEntry,
    pub item: ContentItem,
    /// The item's version moved since the entry's base (D4).
    pub conflict: bool,
    /// The entry review, if the type has a workflow and one is open.
    pub review: Option<ReviewSummary>,
}

/// A release with its entries.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseView {
    pub release: Release,
    pub entries: Vec<EntryView>,
}

/// `Locked { release }` when item `id` is in a scheduled release (D5). The public
/// content write paths call it; system paths (imports, seed, `publish_due`) do not.
pub(crate) async fn check_unlocked(repos: &dyn Repos, id: ContentId) -> Result<(), CoreError> {
    match repos.releases().lock_of(id).await? {
        Some(release) => Err(CoreError::Locked { release }),
        None => Ok(()),
    }
}

/// The content-quality gate for an item a release publishes (ruling P4, M6-4's
/// `quality::gate::check`): with the type's gate on, unsuppressed accessibility errors
/// refuse the entry, and so the whole release (all or nothing).
pub(crate) async fn quality_gate(repos: &dyn Repos, item: &ContentItem) -> Result<(), CoreError> {
    let Some(ty) = repos.types().get(&item.kind).await? else {
        return Ok(());
    };
    crate::quality::gate::check(repos, item, &ty).await
}

/// Whether `actor` may see `release` (§4.7).
pub(crate) fn may_see(actor: &Actor, release: &Release) -> bool {
    release.created_by == Some(actor.user) || can(actor.role, Capability::EditOthers)
}

fn check_title(title: &str) -> Result<String, CoreError> {
    let t = title.trim();
    if t.is_empty() || t.chars().count() > MAX_RELEASE_TITLE || t.chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            "title",
            format!("1 to {MAX_RELEASE_TITLE} characters"),
        ));
    }
    Ok(t.to_owned())
}

fn check_description(d: &str) -> Result<String, CoreError> {
    let d = d.trim();
    if d.chars().count() > MAX_RELEASE_DESCRIPTION
        || d.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(CoreError::invalid_field(
            "description",
            format!("at most {MAX_RELEASE_DESCRIPTION} characters"),
        ));
    }
    Ok(d.to_owned())
}

/// Entries can change while a release is `open` or `failed`.
fn editable(r: &Release) -> Result<(), CoreError> {
    match r.status {
        ReleaseStatus::Open | ReleaseStatus::Failed => Ok(()),
        ReleaseStatus::Scheduled => Err(CoreError::Invalid("unschedule it first".into())),
        ReleaseStatus::Applied => Err(CoreError::Invalid("the release is applied".into())),
    }
}

/// Releases: create, stage, check, schedule, apply (design 15 §4).
pub struct ReleaseService {
    store: Arc<dyn Store>,
    permalinks: Arc<dyn Permalinks>,
    content: Arc<ContentService>,
}

impl ReleaseService {
    pub fn new(
        store: Arc<dyn Store>,
        permalinks: Arc<dyn Permalinks>,
        content: Arc<ContentService>,
    ) -> Self {
        Self {
            store,
            permalinks,
            content,
        }
    }

    /// The permalink rules this service writes with.
    pub fn permalinks(&self) -> Arc<dyn Permalinks> {
        self.permalinks.clone()
    }

    /// Reads release `id` through `repos` and checks that `actor` may see it.
    pub(crate) async fn visible(
        repos: &dyn Repos,
        actor: &Actor,
        id: ReleaseId,
    ) -> Result<Release, CoreError> {
        let r = repos.releases().get(id).await?.ok_or(CoreError::NotFound)?;
        if !may_see(actor, &r) {
            return Err(CoreError::NotFound);
        }
        Ok(r)
    }

    /// Like [`visible`](Self::visible), at `version` (`Conflict` otherwise).
    async fn at_version(
        repos: &dyn Repos,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
    ) -> Result<Release, CoreError> {
        let r = Self::visible(repos, actor, id).await?;
        if r.version != version {
            return Err(CoreError::Conflict {
                current_version: r.version,
            });
        }
        Ok(r)
    }

    /// Creates an open release (every role may: `edit_own`).
    pub async fn create(&self, actor: &Actor, new: NewRelease) -> Result<Release, CoreError> {
        require(actor, Capability::EditOwn)?;
        let now = Timestamp::now();
        let r = Release {
            id: ReleaseId::new(),
            title: check_title(&new.title)?,
            description: check_description(&new.description)?,
            status: ReleaseStatus::Open,
            scheduled_at: None,
            scheduled_by: None,
            override_reason: None,
            failure: None,
            created_by: Some(actor.user),
            created_at: now,
            updated_at: now,
            applied_at: None,
            applied_by: None,
            version: 1,
        };
        self.store.read().releases().insert(&r).await?;
        Ok(r)
    }

    /// The release with its entries and their items (`NotFound` when not visible).
    pub async fn get_for(&self, actor: &Actor, id: ReleaseId) -> Result<ReleaseView, CoreError> {
        require_read(actor, Area::Content)?;
        let repos = self.store.read();
        let release = Self::visible(repos, actor, id).await?;
        let entries = repos.releases().entries(id).await?;
        let ids: Vec<ContentId> = entries.iter().map(|e| e.item_id).collect();
        let items = repos.content().get_many(&ids).await?;
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(item) = items.iter().find(|i| i.id == entry.item_id).cloned() else {
                continue;
            };
            let review = gate::summary(
                repos,
                &ferrite_domain::ReviewTarget::Entry(id, item.id),
                Some(entry.entry_version),
            )
            .await?;
            out.push(EntryView {
                conflict: item.version != entry.base_version,
                entry,
                item,
                review,
            });
        }
        Ok(ReleaseView {
            release,
            entries: out,
        })
    }

    /// A page of the releases `actor` may see (`mine`: only their own) with each one's
    /// entry count.
    pub async fn list(
        &self,
        actor: &Actor,
        mut query: ReleaseQuery,
        mine: bool,
    ) -> Result<(Page<Release>, Vec<(ReleaseId, u32)>), CoreError> {
        require_read(actor, Area::Content)?;
        if mine || !can(actor.role, Capability::EditOthers) {
            query.created_by = Some(actor.user);
        }
        query.limit = query.limit.clamp(1, 100);
        let repos = self.store.read();
        let page = repos.releases().list(&query).await?;
        let ids: Vec<ReleaseId> = page.items.iter().map(|r| r.id).collect();
        let counts = repos.releases().entry_counts(&ids).await?;
        Ok((page, counts))
    }

    /// The releases (not applied) holding `item` that `actor` may see.
    pub async fn releases_of(
        &self,
        actor: &Actor,
        item: ContentId,
    ) -> Result<Vec<Release>, CoreError> {
        require_read(actor, Area::Content)?;
        Ok(self
            .store
            .read()
            .releases()
            .releases_of(item)
            .await?
            .into_iter()
            .filter(|r| may_see(actor, r))
            .collect())
    }

    /// Changes the title and description of an open or failed release.
    pub async fn update(
        &self,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
        title: Option<String>,
        description: Option<String>,
    ) -> Result<Release, CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let mut r = Self::at_version(tx.as_ref(), actor, id, version).await?;
            editable(&r)?;
            if let Some(t) = title {
                r.title = check_title(&t)?;
            }
            if let Some(d) = description {
                r.description = check_description(&d)?;
            }
            r.updated_at = Timestamp::now();
            Ok(tx.releases().update(&r).await?)
        }
        .await;
        finish(tx, done).await
    }

    /// Deletes an open or failed release with its entries, reviews and preview links.
    pub async fn delete(
        &self,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
    ) -> Result<(), CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let r = Self::at_version(tx.as_ref(), actor, id, version).await?;
            editable(&r)?;
            tx.preview_links()
                .delete_for_target(&PreviewTarget::Release(id))
                .await?;
            tx.releases().delete(id).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }

    /// Adds or replaces the entry of `input.item` (§4.2). The item must be readable and
    /// editable by `actor`, not trashed, not scheduled; `publish` needs a draft or
    /// pending item, `unpublish` a published or private one, both its publish right; an
    /// entry needs a change or a transition; the staged change is validated. A new entry
    /// is based on the item's current version; a replaced one keeps its base and bumps
    /// its entry version (an approval of it goes stale).
    pub async fn stage(
        &self,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
        input: EntryInput,
    ) -> Result<Release, CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let mut r = Self::at_version(tx.as_ref(), actor, id, version).await?;
            editable(&r)?;
            let item = tx
                .content()
                .get(input.item)
                .await?
                .filter(|i| crate::can_read(actor, i, Timestamp::now()))
                .ok_or_else(|| CoreError::invalid_field("item_id", "unknown item"))?;
            check_entry(tx.as_ref(), actor, &item, &input).await?;
            let existing = tx.releases().entry(id, item.id).await?;
            if existing.is_none() {
                let n = tx.releases().entry_counts(&[id]).await?;
                let count = n.first().map_or(0, |(_, c)| *c) as usize;
                if count >= MAX_RELEASE_ENTRIES {
                    return Err(CoreError::Invalid(format!(
                        "a release holds at most {MAX_RELEASE_ENTRIES} entries"
                    )));
                }
            }
            let now = Timestamp::now();
            tx.releases()
                .put_entry(&ReleaseEntry {
                    release_id: id,
                    item_id: item.id,
                    position: 0,
                    transition: input.transition,
                    changes: input.changes,
                    base_version: existing.as_ref().map_or(item.version, |e| e.base_version),
                    entry_version: 1,
                    added_by: Some(actor.user),
                    updated_at: now,
                })
                .await?;
            if existing.is_some() {
                tx.reviews().mark_stale_entry(id, item.id, now).await?;
            }
            r.updated_at = now;
            Ok(tx.releases().update(&r).await?)
        }
        .await;
        finish(tx, done).await
    }

    /// Removes the entry of `item`.
    pub async fn unstage(
        &self,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
        item: ContentId,
    ) -> Result<Release, CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let mut r = Self::at_version(tx.as_ref(), actor, id, version).await?;
            editable(&r)?;
            if !tx.releases().delete_entry(id, item).await? {
                return Err(CoreError::NotFound);
            }
            r.updated_at = Timestamp::now();
            Ok(tx.releases().update(&r).await?)
        }
        .await;
        finish(tx, done).await
    }

    /// Bases the entry of `item` on the item's current version (D4), keeping the staged
    /// change; the entry version moves, so its approval goes stale.
    pub async fn rebase(
        &self,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
        item: ContentId,
    ) -> Result<Release, CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let mut r = Self::at_version(tx.as_ref(), actor, id, version).await?;
            editable(&r)?;
            let mut entry = tx
                .releases()
                .entry(id, item)
                .await?
                .ok_or(CoreError::NotFound)?;
            let current = tx.content().get(item).await?.ok_or(CoreError::NotFound)?;
            can_edit(actor, &current)?;
            let now = Timestamp::now();
            entry.base_version = current.version;
            entry.updated_at = now;
            tx.releases().put_entry(&entry).await?;
            tx.reviews().mark_stale_entry(id, item, now).await?;
            r.updated_at = now;
            Ok(tx.releases().update(&r).await?)
        }
        .await;
        finish(tx, done).await
    }
}

/// The staging checks of one entry (§4.2, D39).
async fn check_entry(
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
    input: &EntryInput,
) -> Result<(), CoreError> {
    can_edit(actor, item)?;
    match item.status {
        ContentStatus::Trash => return Err(CoreError::Invalid("restore it first".into())),
        ContentStatus::Scheduled => return Err(CoreError::Invalid("unschedule it first".into())),
        _ => {}
    }
    if item.kind == ferrite_domain::MEDIA_KIND {
        return Err(CoreError::invalid_field(
            "item_id",
            "media cannot be staged",
        ));
    }
    match input.transition {
        EntryTransition::None => {
            if input.changes.is_none() {
                return Err(CoreError::Invalid(
                    "an entry needs a change or a transition".into(),
                ));
            }
        }
        EntryTransition::Publish => {
            if !matches!(item.status, ContentStatus::Draft | ContentStatus::Pending) {
                return Err(CoreError::invalid_field(
                    "transition",
                    "only a draft or pending item can be published",
                ));
            }
            if !ContentService::may_publish(actor, item) {
                return Err(CoreError::Forbidden(publish_right(actor, item)));
            }
        }
        EntryTransition::Unpublish => {
            if !matches!(
                item.status,
                ContentStatus::Published | ContentStatus::Private
            ) {
                return Err(CoreError::invalid_field(
                    "transition",
                    "only a published or private item can be unpublished",
                ));
            }
            if !ContentService::may_publish(actor, item) {
                return Err(CoreError::Forbidden(publish_right(actor, item)));
            }
        }
    }
    if let Some(change) = &input.changes {
        staged::check(change, repos, actor, item).await?;
    }
    Ok(())
}

/// The capability publishing `item` needs.
fn publish_right(actor: &Actor, item: &ContentItem) -> &'static str {
    if item.author_id == actor.user {
        Capability::PublishOwn.as_str()
    } else {
        Capability::PublishOthers.as_str()
    }
}
