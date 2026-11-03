//! Reading change sets and reverting them (design 17 §7–§8).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ferrite_domain::{
    Change, ChangeSet, ChangeSetId, ChangeSetQuery, ChangeSummary, Image, NewAudit, Page,
    Timestamp, UserId,
};
use ferrite_store::{LOCK_EXTENSIONS, LOCK_TYPES, LockGuard, Store, Tx, lock};
use serde_json::{Value, json};
use uuid::Uuid;

use super::capture::{IDLE_WINDOW_MICROS, MAX_SET_CHANGES, agent_key};
use super::object::ObjectRef;
use super::restore::{self, Restore};
use super::snapshot::{hash_snapshot, open_image, snapshot};
use crate::{
    Actor, Area, CoreError,
    actor::{Capability, Scope, can, require, require_read, require_scope},
    permalink::Permalinks,
    webhooks::{self, NewWebhookEvent},
};

/// At most this many changes one revert request names.
pub const MAX_REVERT_CHANGES: usize = 500;

/// Which sets a list shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeSetFilter {
    /// Another user's sets (`manage_users`); `None` with `all_users` false: the actor's.
    pub user: Option<UserId>,
    /// Every user's sets (`manage_users`).
    pub all_users: bool,
    /// Only this grant's or token's.
    pub agent: Option<Uuid>,
    pub open: Option<bool>,
    /// 1..=200; 0 means the default 50.
    pub limit: u32,
    pub cursor: Option<String>,
}

/// A set with its changes, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSetDetail {
    pub set: ChangeSet,
    pub changes: Vec<ChangeSummary>,
}

/// The images of one change as the viewer may see them.
#[derive(Debug, Clone, PartialEq)]
pub enum Images {
    /// The viewer may not read the object's area now (design 17 §8).
    Hidden,
    /// `None`: absent, or too large to keep (the summary's state says which). Sealed
    /// plugin secrets are shown as `{"secret": "set"}`.
    Shown {
        before: Option<Value>,
        after: Option<Value>,
    },
}

/// One change with its images.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeDetail {
    pub change: ChangeSummary,
    pub images: Images,
}

/// What a revert asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevertRequest {
    /// `None`: every change not reverted yet; else these seqs (deduplicated).
    pub changes: Option<Vec<u32>>,
    /// Overwrite later edits (`changed_since` only).
    pub force: bool,
    /// Run every check and write, then roll back.
    pub dry_run: bool,
}

/// What a revert did (or, dry, would do).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevertReport {
    /// Newest first.
    pub reverted: Vec<u32>,
    /// Selected but reverted before.
    pub skipped: Vec<u32>,
    /// Reverted over a later edit.
    pub forced: Vec<u32>,
    pub notes: Vec<(u32, String)>,
    pub dry_run: bool,
}

/// Why a change cannot be reverted as asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictReason {
    /// The object changed after the assistant's write (forceable).
    ChangedSince {
        by: Option<UserId>,
        at: Option<Timestamp>,
    },
    /// A later change of this set on the same object is not reverted and not selected.
    LaterChange(u32),
    /// The object was deleted since.
    Gone,
    /// The image was too large to keep.
    TooLarge,
}

impl ConflictReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ChangedSince { .. } => "changed_since",
            Self::LaterChange(_) => "later_change",
            Self::Gone => "gone",
            Self::TooLarge => "too_large",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertConflict {
    pub seq: u32,
    pub reason: ConflictReason,
    pub object_kind: String,
    pub object_id: String,
    pub label: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RevertError {
    #[error("{} change(s) conflict", .0.len())]
    Conflicts(Vec<RevertConflict>),
    #[error("reverting change {seq} failed: {error}")]
    Failed { seq: u32, error: CoreError },
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl From<ferrite_store::StoreError> for RevertError {
    fn from(e: ferrite_store::StoreError) -> Self {
        Self::Core(e.into())
    }
}

/// Lists and reverts change sets.
pub struct ChangeSetService {
    store: Arc<dyn Store>,
    permalinks: Arc<dyn Permalinks>,
    /// The content write paths the reverse writes go through.
    content: crate::ContentService,
}

fn now_idle_since(now: Timestamp) -> Timestamp {
    Timestamp::from_micros(now.as_micros() - IDLE_WINDOW_MICROS).unwrap_or(now)
}

/// Whether the set still takes changes: it holds its key, was written in the last 30
/// minutes and is not full.
pub fn is_open(set: &ChangeSet, now: Timestamp) -> bool {
    set.open && set.last_at >= now_idle_since(now) && set.changes < MAX_SET_CHANGES
}

/// The read every change-set view needs: `read_admin` and `users:read` (as Connected
/// apps).
fn require_viewer(actor: &Actor) -> Result<(), CoreError> {
    require(actor, Capability::ReadAdmin)?;
    require_scope(actor, Some(Scope::read(Area::Users)))
}

/// The set's user, or an administrator (`manage_users`); others get `NotFound`.
fn may_see(actor: &Actor, set: &ChangeSet) -> bool {
    set.user_id == Some(actor.user) || can(actor.role, Capability::ManageUsers)
}

/// Whether `actor` may read the images of an object (design 17 §8, D16).
fn may_read_images(actor: &Actor, object: &ObjectRef) -> bool {
    let cap = |c: Capability, area: Area| can(actor.role, c) && require_read(actor, area).is_ok();
    match object {
        ObjectRef::Option(key) if key.starts_with("theme.") => {
            cap(Capability::ManageThemes, Area::Extensions)
        }
        ObjectRef::Option(key) if key.starts_with("plugin.") => {
            cap(Capability::ManagePlugins, Area::Extensions)
        }
        ObjectRef::Option(_) => cap(Capability::ManageSettings, Area::Settings),
        ObjectRef::ActiveTheme => cap(Capability::ManageThemes, Area::Extensions),
        ObjectRef::Plugin(_) => cap(Capability::ManagePlugins, Area::Extensions),
        other => require_read(actor, other.area()).is_ok(),
    }
}

/// A sealed secret (`{"sealed": …}`) as `{"secret": "set"}`, recursively.
fn redact(v: Value) -> Value {
    match v {
        Value::Object(map) if map.len() == 1 && map.contains_key("sealed") => {
            json!({"secret": "set"})
        }
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, redact(v))).collect()),
        Value::Array(items) => Value::Array(items.into_iter().map(redact).collect()),
        other => other,
    }
}

fn shown(img: &Image) -> Result<Option<Value>, CoreError> {
    match img {
        Image::Skipped { .. } => Ok(None),
        other => Ok(open_image(other)?.map(redact)),
    }
}

fn summary_of(c: &Change) -> ChangeSummary {
    ChangeSummary {
        set_id: c.set_id,
        seq: c.seq,
        at: c.at,
        op: c.op.clone(),
        object_kind: c.object_kind.clone(),
        object_id: c.object_id.clone(),
        label: c.label.clone(),
        before_state: c.before.state(),
        before_hash: c.before.hash().map(str::to_owned),
        after_state: c.after.state(),
        after_hash: c.after.hash().map(str::to_owned),
        reverted_at: c.reverted_at,
        reverted_by: c.reverted_by,
        revert_forced: c.revert_forced,
    }
}

impl ChangeSetService {
    pub fn new(store: Arc<dyn Store>, permalinks: Arc<dyn Permalinks>) -> Self {
        Self {
            content: crate::ContentService::new(store.clone(), permalinks.clone()),
            store,
            permalinks,
        }
    }

    /// The sets `filter` names, newest `last_at` first. Others' sets (or every user's)
    /// need `manage_users`.
    pub async fn list(
        &self,
        actor: &Actor,
        filter: &ChangeSetFilter,
    ) -> Result<Page<ChangeSet>, CoreError> {
        require_viewer(actor)?;
        let user = if filter.all_users {
            None
        } else {
            Some(filter.user.unwrap_or(actor.user))
        };
        if user != Some(actor.user) {
            require(actor, Capability::ManageUsers)?;
        }
        let now = Timestamp::now();
        Ok(self
            .store
            .read()
            .change_sets()
            .list(&ChangeSetQuery {
                user,
                agent_id: filter.agent,
                open: filter.open,
                idle_since: now_idle_since(now),
                limit: if filter.limit == 0 { 50 } else { filter.limit },
                cursor: filter.cursor.clone(),
            })
            .await?)
    }

    async fn visible(&self, actor: &Actor, id: ChangeSetId) -> Result<ChangeSet, CoreError> {
        require_viewer(actor)?;
        let set = self
            .store
            .read()
            .change_sets()
            .get(id)
            .await?
            .ok_or(CoreError::NotFound)?;
        if !may_see(actor, &set) {
            return Err(CoreError::NotFound);
        }
        Ok(set)
    }

    /// A set with its changes, newest first.
    pub async fn get(&self, actor: &Actor, id: ChangeSetId) -> Result<ChangeSetDetail, CoreError> {
        let set = self.visible(actor, id).await?;
        let mut changes = self.store.read().change_sets().summaries(id).await?;
        changes.reverse();
        Ok(ChangeSetDetail { set, changes })
    }

    /// One change with its images, hidden without the viewer's read right on the area.
    pub async fn change(
        &self,
        actor: &Actor,
        id: ChangeSetId,
        seq: u32,
    ) -> Result<ChangeDetail, CoreError> {
        self.visible(actor, id).await?;
        let c = self
            .store
            .read()
            .change_sets()
            .change(id, seq)
            .await?
            .ok_or(CoreError::NotFound)?;
        let readable = ObjectRef::parse(&c.object_kind, &c.object_id)
            .is_some_and(|o| may_read_images(actor, &o));
        let images = if readable {
            Images::Shown {
                before: shown(&c.before)?,
                after: shown(&c.after)?,
            }
        } else {
            Images::Hidden
        };
        Ok(ChangeDetail {
            change: summary_of(&c),
            images,
        })
    }

    /// The calling assistant's own recent sets (MCP `changes_list`, design 17 §10):
    /// `set` alone, else the `limit` newest. Needs an agent; no images.
    pub async fn agent_sets(
        &self,
        actor: &Actor,
        set: Option<ChangeSetId>,
        limit: u32,
    ) -> Result<Vec<ChangeSetDetail>, CoreError> {
        let agent = actor
            .agent
            .ok_or(CoreError::Forbidden("an assistant connection"))?;
        let (_, _, agent_id) = agent_key(agent);
        let repos = self.store.read();
        let sets = match set {
            Some(id) => repos
                .change_sets()
                .get(id)
                .await?
                .filter(|s| s.agent_id == agent_id && s.user_id == Some(actor.user))
                .into_iter()
                .collect(),
            None => {
                repos
                    .change_sets()
                    .list(&ChangeSetQuery {
                        user: Some(actor.user),
                        agent_id: Some(agent_id),
                        open: None,
                        idle_since: now_idle_since(Timestamp::now()),
                        limit: limit.clamp(1, 20),
                        cursor: None,
                    })
                    .await?
                    .items
            }
        };
        let mut out = Vec::with_capacity(sets.len());
        for s in sets {
            let mut changes = repos.change_sets().summaries(s.id).await?;
            changes.reverse();
            out.push(ChangeSetDetail { set: s, changes });
        }
        Ok(out)
    }

    /// How many changes assistants recorded since `since` in the actor's sets (every
    /// user's for `manage_users`): the dashboard's count.
    pub async fn changes_since(&self, actor: &Actor, since: Timestamp) -> Result<u64, CoreError> {
        require_viewer(actor)?;
        let user = (!can(actor.role, Capability::ManageUsers)).then_some(actor.user);
        Ok(self
            .store
            .read()
            .change_sets()
            .changes_since(user, since)
            .await?)
    }

    /// Reverts changes of set `id` (design 17 §7): newest first, in one transaction, as
    /// `actor` through the normal write paths. A person only (an assistant connection is
    /// `Forbidden`), the set's user or `manage_users`.
    pub async fn revert(
        &self,
        actor: &Actor,
        id: ChangeSetId,
        req: RevertRequest,
    ) -> Result<RevertReport, RevertError> {
        if actor.agent.is_some() {
            return Err(CoreError::Forbidden("a person, not an assistant").into());
        }
        let set = self.visible(actor, id).await?;
        let all = self.store.read().change_sets().changes(id).await?;
        let plan = plan(&all, req.changes.as_deref())?;
        if plan.selected.is_empty() {
            return Ok(RevertReport {
                skipped: plan.skipped,
                dry_run: req.dry_run,
                ..RevertReport::default()
            });
        }
        let objects: Vec<ObjectRef> = plan
            .selected
            .iter()
            .filter_map(|c| ObjectRef::parse(&c.object_kind, &c.object_id))
            .collect();
        let (types, extensions) = restore::locks_needed(&objects, &plan.selected);
        let mut guards: Vec<Box<dyn LockGuard>> = Vec::new();
        if types {
            guards.push(
                lock(
                    self.store.as_ref(),
                    LOCK_TYPES,
                    crate::types::TYPES_LOCK_WAIT,
                )
                .await?,
            );
        }
        if extensions {
            match lock(
                self.store.as_ref(),
                LOCK_EXTENSIONS,
                crate::EXTENSIONS_LOCK_WAIT,
            )
            .await
            {
                Ok(g) => guards.push(g),
                Err(e) => {
                    release(guards).await;
                    return Err(CoreError::from(e).into());
                }
            }
        }
        let tx = match self.store.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                release(guards).await;
                return Err(e.into());
            }
        };
        let rules = self.permalinks.pin();
        let result = self
            .revert_in(tx.as_ref(), rules.as_ref(), actor, &set, &plan, &req)
            .await;
        let out = match result {
            Ok(report) if !req.dry_run => match tx.commit().await {
                Ok(()) => Ok(report),
                Err(e) => Err(e.into()),
            },
            other => {
                if let Err(e) = tx.rollback().await {
                    tracing::warn!(error = %e, "rollback after a revert failed");
                }
                other
            }
        };
        release(guards).await;
        out
    }

    async fn revert_in(
        &self,
        tx: &dyn Tx,
        rules: &dyn Permalinks,
        actor: &Actor,
        set: &ChangeSet,
        plan: &Plan,
        req: &RevertRequest,
    ) -> Result<RevertReport, RevertError> {
        // The conflict check simulates the object hashes newest first (design 17 §7.2):
        // each change's after-image must be the state the next newer revert leaves.
        let mut state: HashMap<(String, String), Option<String>> = HashMap::new();
        let mut blocked: HashSet<(String, String)> = HashSet::new();
        let mut conflicts = Vec::new();
        let mut forced = Vec::new();
        for c in &plan.selected {
            let key = (c.object_kind.clone(), c.object_id.clone());
            if blocked.contains(&key) {
                continue;
            }
            let Some(object) = ObjectRef::parse(&c.object_kind, &c.object_id) else {
                return Err(CoreError::Internal("unknown change object").into());
            };
            if !state.contains_key(&key) {
                let current = snapshot(tx, &object).await?;
                state.insert(key.clone(), hash_snapshot(current.as_ref()));
            }
            let current = state[&key].clone();
            let conflict = |reason| RevertConflict {
                seq: c.seq,
                reason,
                object_kind: c.object_kind.clone(),
                object_id: c.object_id.clone(),
                label: c.label.clone(),
            };
            let reason = if matches!(c.after, Image::Skipped { .. })
                || matches!(c.before, Image::Skipped { .. })
            {
                Some(conflict(ConflictReason::TooLarge))
            } else if current.as_deref() == c.after.hash() {
                None
            } else if current.is_none() {
                Some(conflict(ConflictReason::Gone))
            } else if req.force {
                forced.push(c.seq);
                None
            } else {
                let (by, at) = restore::last_editor(tx, &object).await?;
                Some(conflict(ConflictReason::ChangedSince { by, at }))
            };
            match reason {
                Some(r) => {
                    conflicts.push(r);
                    blocked.insert(key);
                }
                None => {
                    state.insert(key, c.before.hash().map(str::to_owned));
                }
            }
        }
        if !conflicts.is_empty() {
            return Err(RevertError::Conflicts(conflicts));
        }

        // One reverse write per object, to the before-image of its oldest selected
        // change, objects in the order of their newest change.
        let mut targets: Vec<Restore> = Vec::new();
        for c in &plan.selected {
            let object = ObjectRef::parse(&c.object_kind, &c.object_id)
                .ok_or(CoreError::Internal("unknown change object"))?;
            let target = open_image(&c.before)?;
            match targets.iter_mut().find(|t| t.object == object) {
                Some(t) => {
                    t.target = target;
                    t.oldest_seq = c.seq;
                }
                None => targets.push(Restore {
                    object,
                    target,
                    newest_seq: c.seq,
                    oldest_seq: c.seq,
                }),
            }
        }
        let mut notes = Vec::new();
        restore::apply(&self.content, tx, rules, actor, &targets, &mut notes).await?;

        let now = Timestamp::now();
        let seqs: Vec<u32> = plan.selected.iter().map(|c| c.seq).collect();
        let cs = tx.change_sets();
        cs.mark_reverted(set.id, &seqs, actor.user, now, &forced)
            .await?;
        cs.close(set.id).await?;
        tx.audit()
            .append(&NewAudit {
                actor_id: Some(actor.user),
                action: "change_set.revert".into(),
                target_kind: "change_set".into(),
                target_id: Some(set.id.to_string()),
                details: json!({"changes": seqs, "forced": forced, "client": set.client}),
            })
            .await?;
        webhooks::emit(
            tx,
            NewWebhookEvent {
                kind: "change_set.reverted",
                content_type: None,
                data: json!({
                    "id": set.id.to_string(),
                    "client": set.client,
                    "user": set.user_id.map(|u| u.to_string()),
                    "reverted_by": actor.user.to_string(),
                    "changes": seqs,
                    "forced": forced,
                }),
            },
        )
        .await?;
        Ok(RevertReport {
            reverted: seqs,
            skipped: plan.skipped.clone(),
            forced,
            notes,
            dry_run: req.dry_run,
        })
    }
}

async fn release(guards: Vec<Box<dyn LockGuard>>) {
    for g in guards.into_iter().rev() {
        if let Err(e) = g.release().await {
            tracing::warn!(error = %e, "releasing a lock after a revert failed");
        }
    }
}

/// The changes a request selects, newest first, and the ones it skips.
struct Plan {
    selected: Vec<Change>,
    skipped: Vec<u32>,
}

fn plan(all: &[Change], asked: Option<&[u32]>) -> Result<Plan, RevertError> {
    let mut wanted: Vec<u32> = match asked {
        None => all
            .iter()
            .filter(|c| c.reverted_at.is_none())
            .map(|c| c.seq)
            .collect(),
        Some(seqs) => {
            if seqs.len() > MAX_REVERT_CHANGES {
                return Err(CoreError::invalid_field(
                    "changes",
                    format!("at most {MAX_REVERT_CHANGES} changes"),
                )
                .into());
            }
            let mut v = seqs.to_vec();
            v.sort_unstable();
            v.dedup();
            if let Some(bad) = v.iter().find(|s| !all.iter().any(|c| c.seq == **s)) {
                return Err(CoreError::invalid_field(
                    "changes",
                    format!("no change {bad} in this set"),
                )
                .into());
            }
            v
        }
    };
    wanted.sort_unstable_by(|a, b| b.cmp(a));
    let mut selected = Vec::new();
    let mut skipped = Vec::new();
    for seq in wanted {
        let Some(c) = all.iter().find(|c| c.seq == seq) else {
            continue;
        };
        if c.reverted_at.is_some() {
            skipped.push(seq);
        } else {
            selected.push(c.clone());
        }
    }
    // A later, not reverted, unselected change on the same object (never forceable).
    let mut conflicts = Vec::new();
    for c in &selected {
        if let Some(later) = all.iter().find(|d| {
            d.seq > c.seq
                && d.reverted_at.is_none()
                && d.object_kind == c.object_kind
                && d.object_id == c.object_id
                && !selected.iter().any(|s| s.seq == d.seq)
        }) {
            conflicts.push(RevertConflict {
                seq: c.seq,
                reason: ConflictReason::LaterChange(later.seq),
                object_kind: c.object_kind.clone(),
                object_id: c.object_id.clone(),
                label: c.label.clone(),
            });
        }
    }
    if !conflicts.is_empty() {
        return Err(RevertError::Conflicts(conflicts));
    }
    Ok(Plan { selected, skipped })
}
