//! Apply, check, schedule and the sandbox (design 15 §4.4–§4.6, §5.2).

use ferrite_domain::{
    ContentId, ContentItem, EntryTransition, NewAudit, PreviewTarget, Release, ReleaseEntry,
    ReleaseId, ReleaseStatus, ReviewTarget, Timestamp, UserStatus,
};
use ferrite_store::{Repos, StoreError, Tx};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    ReleaseService, SandboxReport, SandboxStore, editable, may_see, quality_gate, staged,
    tx::{ApplyTx, Mode},
};
use crate::{
    Actor, ContentService, CoreError,
    actor::{Area, Capability, require, require_read},
    content::Edit,
    jobs::{editorial_notify_job, release_apply_job},
    permalink::Permalinks,
    webhooks::{self, NewWebhookEvent},
    workflow::gate,
};

/// Releases checked per round by [`ReleaseService::apply_due`].
const DUE_BATCH: u32 = 50;
/// The furthest a release may be scheduled ahead.
const MAX_SCHEDULE_AHEAD_MICROS: i64 = 366 * 24 * 3600 * 1_000_000;

/// What applying one entry would do (design 15 §4.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum EntryOutcome {
    Ok,
    /// The item moved since the entry's base (D4).
    Conflict { current_version: i64 },
    /// The type has a workflow and the entry (or item) has no current approval.
    NeedsApproval { step: Option<String> },
    /// The actor lacks a right the entry needs.
    Forbidden { capability: String },
    /// The staged change or the transition is refused (a slug taken since, …).
    Invalid {
        field: Option<String>,
        message: String,
    },
}

/// One entry of a [`ReleaseCheck`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryCheck {
    pub item: ContentId,
    #[serde(flatten)]
    pub outcome: EntryOutcome,
}

/// The check report of a release: one line per entry, in position order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseCheck {
    pub entries: Vec<EntryCheck>,
}

impl ReleaseCheck {
    /// Every entry is `ok`.
    pub fn is_clean(&self) -> bool {
        self.entries.iter().all(|e| e.outcome == EntryOutcome::Ok)
    }
}

/// The outcome an entry error stands for; store and other errors are returned as they
/// are (they are not about the entry).
fn outcome_of(e: CoreError) -> Result<EntryOutcome, CoreError> {
    Ok(match e {
        CoreError::Conflict { current_version } => EntryOutcome::Conflict { current_version },
        CoreError::ApprovalRequired { step } => EntryOutcome::NeedsApproval { step },
        CoreError::Forbidden(c) => EntryOutcome::Forbidden {
            capability: c.to_owned(),
        },
        CoreError::Invalid(message) => EntryOutcome::Invalid {
            field: None,
            message,
        },
        CoreError::InvalidField { field, message } => EntryOutcome::Invalid {
            field: Some(field.to_owned()),
            message,
        },
        CoreError::InvalidKey { key, message } => EntryOutcome::Invalid {
            field: Some(key),
            message,
        },
        CoreError::NotFound => EntryOutcome::Invalid {
            field: None,
            message: "the item is gone".into(),
        },
        e @ CoreError::StagingRequired => EntryOutcome::Invalid {
            field: None,
            message: e.to_string(),
        },
        CoreError::Locked { release } => EntryOutcome::Invalid {
            field: None,
            message: format!("locked by release {release}"),
        },
        other => return Err(other),
    })
}

/// How [`apply_entry`] runs.
#[derive(Debug, Clone, Copy)]
struct Opts<'a> {
    /// A moved item is a `Conflict` (apply); otherwise it is applied over (check, preview).
    strict: bool,
    /// Run the approval and quality gates (apply, check), not in a preview.
    gates: bool,
    override_reason: Option<&'a str>,
}

/// One applied entry.
struct Applied {
    item: ContentItem,
    /// The item's version before the entry was applied.
    before: i64,
    action: &'static str,
    conflict: bool,
    overridden: bool,
}

/// Applies `entry` of `release` through the core write paths inside `tx` as `actor`.
async fn apply_entry(
    tx: &dyn Tx,
    rules: &dyn Permalinks,
    content: &ContentService,
    actor: &Actor,
    release: &Release,
    entry: &ReleaseEntry,
    opts: Opts<'_>,
) -> Result<Applied, CoreError> {
    let item = tx
        .content()
        .get(entry.item_id)
        .await?
        .ok_or(CoreError::NotFound)?;
    let before = item.version;
    let conflict = item.version != entry.base_version;
    if conflict && opts.strict {
        return Err(CoreError::Conflict {
            current_version: item.version,
        });
    }
    let mut overridden = false;
    if opts.gates {
        if entry.changes.is_some() {
            overridden |= gate::require_approval(
                tx,
                actor,
                &item,
                ReviewTarget::Entry(release.id, item.id),
                Some(entry.entry_version),
                opts.override_reason,
            )
            .await?;
        } else if entry.transition == EntryTransition::Publish {
            overridden |= gate::require_approval(
                tx,
                actor,
                &item,
                ReviewTarget::Item(item.id),
                None,
                opts.override_reason,
            )
            .await?;
        }
    }
    let mut saved = item;
    if let Some(change) = &entry.changes {
        let draft = staged::into_draft(change, &saved.kind)?;
        saved = content
            .update_in(
                tx,
                rules,
                actor,
                Edit {
                    id: saved.id,
                    expected_version: saved.version,
                    draft,
                    note: Some(format!("Release: {}", release.title)),
                },
            )
            .await?;
    }
    let action = match entry.transition {
        EntryTransition::None => "change",
        EntryTransition::Publish => {
            if opts.gates {
                quality_gate(tx, &saved).await?;
            }
            saved = ContentService::publish_in(tx, rules, actor, saved).await?;
            webhooks::emit_content(tx, rules, "content.published", &saved, Some(actor.user))
                .await?;
            "publish"
        }
        EntryTransition::Unpublish => {
            saved = ContentService::unpublish_in(tx, rules, actor, saved).await?;
            webhooks::emit_content(tx, rules, "content.unpublished", &saved, Some(actor.user))
                .await?;
            "unpublish"
        }
    };
    Ok(Applied {
        before,
        item: saved,
        action,
        conflict,
        overridden,
    })
}

fn release_ref(r: &Release) -> Value {
    json!({"id": r.id.to_string(), "title": r.title})
}

fn audit(actor: Option<&Actor>, action: &str, r: &Release, details: Value) -> NewAudit {
    NewAudit {
        actor_id: actor.map(|a| a.user),
        action: action.into(),
        target_kind: "release".into(),
        target_id: Some(r.id.to_string()),
        details,
    }
}

/// Whether `e` is permanent for a scheduled apply (retrying cannot help).
fn permanent(e: &CoreError) -> bool {
    !matches!(e, CoreError::Store(_) | CoreError::Unavailable(_) | CoreError::Random)
}

impl ReleaseService {
    /// Applies release `id` (at `version`) now, in one transaction (§4.4). An entry that
    /// cannot be applied rolls everything back: `ReleaseConflict` lists the moved
    /// entries, or the one that failed. An admin may pass `override_reason` for entries
    /// without approval (§6.5).
    pub async fn apply(
        &self,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
        override_reason: Option<String>,
    ) -> Result<Release, CoreError> {
        require(actor, Capability::EditOwn)?;
        if let Some(reason) = &override_reason {
            gate::check_override(actor, reason)?;
        }
        let rules = self.permalinks.pin();
        let tx = ApplyTx::new(self.store.begin().await?, Mode::Collect);
        let done = async {
            let r = Self::at_version(&tx, actor, id, version).await?;
            editable(&r)?;
            self.apply_in(&tx, rules.as_ref(), actor, r, override_reason.as_deref())
                .await
        }
        .await;
        self.finish_apply(tx, done).await
    }

    async fn finish_apply<T>(&self, tx: ApplyTx, done: Result<T, CoreError>) -> Result<T, CoreError> {
        let done = match done {
            Ok(v) => match tx.flush_invalidation().await {
                Ok(()) => Ok(v),
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e),
        };
        crate::tx::finish(Box::new(tx), done).await
    }

    /// The apply of `r` (as read in `tx`) as `actor`; see [`apply`](Self::apply).
    async fn apply_in(
        &self,
        tx: &ApplyTx,
        rules: &dyn Permalinks,
        actor: &Actor,
        mut r: Release,
        override_reason: Option<&str>,
    ) -> Result<Release, CoreError> {
        // Claim the row first: a second applier (a job and the periodic, two nodes)
        // waits here and then fails the version check.
        let now = Timestamp::now();
        r.updated_at = now;
        let mut r = tx.releases().update(&r).await?;
        let entries = tx.releases().entries(r.id).await?;
        if entries.is_empty() {
            return Err(CoreError::Invalid("the release has no entries".into()));
        }
        let mut moved = Vec::new();
        for e in &entries {
            if let Some(item) = tx.content().get(e.item_id).await?
                && item.version != e.base_version
            {
                moved.push(EntryCheck {
                    item: e.item_id,
                    outcome: EntryOutcome::Conflict {
                        current_version: item.version,
                    },
                });
            }
        }
        if !moved.is_empty() {
            return Err(CoreError::ReleaseConflict(moved));
        }
        let opts = Opts {
            strict: true,
            gates: true,
            override_reason,
        };
        let mut applied = Vec::with_capacity(entries.len());
        let mut overridden = false;
        for e in &entries {
            match apply_entry(tx, rules, &self.content, actor, &r, e, opts).await {
                Ok(a) => {
                    overridden |= a.overridden;
                    applied.push(a);
                }
                Err(err) => {
                    let outcome = outcome_of(err)?;
                    return Err(CoreError::ReleaseConflict(vec![EntryCheck {
                        item: e.item_id,
                        outcome,
                    }]));
                }
            }
        }
        let items: Vec<Value> = applied
            .iter()
            .map(|a| {
                json!({
                    "id": a.item.id.to_string(), "type": a.item.kind,
                    "locale": a.item.locale, "action": a.action,
                })
            })
            .collect();
        r.status = ReleaseStatus::Applied;
        r.applied_at = Some(now);
        r.applied_by = Some(actor.user);
        r.failure = None;
        if overridden {
            r.override_reason = override_reason.map(str::to_owned);
        }
        r.updated_at = Timestamp::now();
        let r = tx.releases().update(&r).await?;
        tx.releases().unlock_all(r.id).await?;
        tx.preview_links()
            .delete_for_target(&PreviewTarget::Release(r.id))
            .await?;
        tx.audit()
            .append(&audit(
                Some(actor),
                "release.apply",
                &r,
                json!({"title": r.title, "entries": items}),
            ))
            .await?;
        if overridden {
            tx.audit()
                .append(&audit(
                    Some(actor),
                    "release.override",
                    &r,
                    json!({"reason": override_reason}),
                ))
                .await?;
        }
        webhooks::emit(
            tx,
            NewWebhookEvent {
                kind: "release.applied",
                content_type: None,
                data: json!({
                    "release": release_ref(&r),
                    "items": items,
                    "applied_by": actor.user.to_string(),
                }),
            },
        )
        .await?;
        if r.created_by.is_some() && r.created_by != Some(actor.user) {
            tx.jobs()
                .enqueue(&editorial_notify_job("release.applied", None, Some(r.id)))
                .await?;
        }
        Ok(r)
    }

    /// The check report of release `id` (§4.5): every entry applied in a sandbox (one
    /// savepoint each, over the current version) as `actor`, then rolled back.
    pub async fn check(&self, actor: &Actor, id: ReleaseId) -> Result<ReleaseCheck, CoreError> {
        self.check_as(actor, id, None).await
    }

    async fn check_as(
        &self,
        actor: &Actor,
        id: ReleaseId,
        override_reason: Option<&str>,
    ) -> Result<ReleaseCheck, CoreError> {
        require_read(actor, Area::Content)?;
        let rules = self.permalinks.pin();
        let sandbox = SandboxStore::begin(self.store.clone()).await?;
        let done = async {
            let tx = sandbox.tx();
            let r = Self::visible(tx, actor, id).await?;
            let entries = tx.releases().entries(id).await?;
            let opts = Opts {
                strict: false,
                gates: true,
                override_reason,
            };
            let mut out = Vec::with_capacity(entries.len());
            for e in &entries {
                let nested = tx.begin_nested().await?;
                let res =
                    apply_entry(nested.as_ref(), rules.as_ref(), &self.content, actor, &r, e, opts)
                        .await;
                let outcome = match res {
                    Ok(a) => {
                        nested.commit().await?;
                        if a.conflict {
                            EntryOutcome::Conflict {
                                current_version: a.before,
                            }
                        } else {
                            EntryOutcome::Ok
                        }
                    }
                    Err(err) => {
                        if let Err(rb) = nested.rollback().await {
                            tracing::debug!(error = %rb, "release check: savepoint rollback");
                        }
                        outcome_of(err)?
                    }
                };
                out.push(EntryCheck {
                    item: e.item_id,
                    outcome,
                });
            }
            Ok(ReleaseCheck { entries: out })
        }
        .await;
        if let Err(e) = sandbox.finish().await {
            tracing::debug!(error = %e, "release check: sandbox rollback");
        }
        done
    }

    /// Schedules release `id` for `at` (§4.6): a clean check as `actor` first
    /// (`ReleaseNotReady` with the report otherwise), then the locks, the status and the
    /// `release.apply` job in one transaction.
    pub async fn schedule(
        &self,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
        at: Timestamp,
        override_reason: Option<String>,
    ) -> Result<Release, CoreError> {
        require(actor, Capability::EditOwn)?;
        let override_reason = override_reason
            .map(|r| gate::check_override(actor, &r))
            .transpose()?;
        let now = Timestamp::now();
        if at <= now || at.as_micros() - now.as_micros() > MAX_SCHEDULE_AHEAD_MICROS {
            return Err(CoreError::invalid_field(
                "at",
                "a time in the future, at most a year ahead",
            ));
        }
        let report = self.check_as(actor, id, override_reason.as_deref()).await?;
        if report.entries.is_empty() {
            return Err(CoreError::Invalid("the release has no entries".into()));
        }
        if !report.is_clean() {
            return Err(CoreError::ReleaseNotReady(report));
        }
        let tx = self.store.begin().await?;
        let done = async {
            let mut r = Self::at_version(tx.as_ref(), actor, id, version).await?;
            editable(&r)?;
            let entries = tx.releases().entries(id).await?;
            for e in &entries {
                if let Some(other) = tx.releases().lock_of(e.item_id).await? {
                    return Err(CoreError::Locked { release: other });
                }
            }
            for e in &entries {
                match tx.releases().lock(e.item_id, id).await {
                    Err(StoreError::UniqueViolation { .. }) => {
                        return Err(CoreError::Changed("another release locked an item"));
                    }
                    other => other?,
                }
            }
            r.status = ReleaseStatus::Scheduled;
            r.scheduled_at = Some(at);
            r.scheduled_by = Some(actor.user);
            r.override_reason = override_reason.clone();
            r.failure = None;
            r.updated_at = Timestamp::now();
            let r = tx.releases().update(&r).await?;
            tx.jobs().enqueue(&release_apply_job(id, at)).await?;
            tx.audit()
                .append(&audit(
                    Some(actor),
                    "release.schedule",
                    &r,
                    json!({"at": at.to_rfc3339(), "entries": entries.len()}),
                ))
                .await?;
            webhooks::emit(
                tx.as_ref(),
                NewWebhookEvent {
                    kind: "release.scheduled",
                    content_type: None,
                    data: json!({"release": release_ref(&r), "at": at.to_rfc3339(),
                                 "by": actor.user.to_string()}),
                },
            )
            .await?;
            Ok(r)
        }
        .await;
        crate::tx::finish(tx, done).await
    }

    /// Takes a scheduled release off the schedule: `open` again, locks removed; its
    /// queued job finds it no longer scheduled and does nothing.
    pub async fn unschedule(
        &self,
        actor: &Actor,
        id: ReleaseId,
        version: i64,
    ) -> Result<Release, CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let mut r = Self::at_version(tx.as_ref(), actor, id, version).await?;
            if r.status != ReleaseStatus::Scheduled {
                return Err(CoreError::Invalid("the release is not scheduled".into()));
            }
            tx.releases().unlock_all(id).await?;
            r.status = ReleaseStatus::Open;
            r.scheduled_at = None;
            r.scheduled_by = None;
            r.override_reason = None;
            r.updated_at = Timestamp::now();
            let r = tx.releases().update(&r).await?;
            webhooks::emit(
                tx.as_ref(),
                NewWebhookEvent {
                    kind: "release.unscheduled",
                    content_type: None,
                    data: json!({"release": release_ref(&r), "by": actor.user.to_string()}),
                },
            )
            .await?;
            Ok(r)
        }
        .await;
        crate::tx::finish(tx, done).await
    }

    /// Applies release `id` if it is still scheduled for exactly `at` (the
    /// `release.apply` job), as the user who scheduled it, re-read now (D8). `true` when
    /// it was applied. A permanent failure marks it `failed` with a check report, removes
    /// the locks and notifies (D9); a store error is returned (the job retries).
    pub async fn apply_scheduled(&self, id: ReleaseId, at: Timestamp) -> Result<bool, CoreError> {
        let Some(r) = self.store.read().releases().get(id).await? else {
            return Ok(false);
        };
        if r.status != ReleaseStatus::Scheduled || r.scheduled_at != Some(at) {
            return Ok(false);
        }
        let scheduler = match r.scheduled_by {
            Some(u) => self.store.read().users().get(u).await?,
            None => None,
        };
        let Some(user) = scheduler.filter(|u| u.status == UserStatus::Active) else {
            self.fail(id, at, "the user who scheduled it is gone or disabled", None)
                .await?;
            return Ok(false);
        };
        let actor = Actor::new(user.id, user.role);
        let rules = self.permalinks.pin();
        let tx = ApplyTx::new(self.store.begin().await?, Mode::Collect);
        let done = async {
            let r = tx.releases().get(id).await?.ok_or(CoreError::NotFound)?;
            if r.status != ReleaseStatus::Scheduled || r.scheduled_at != Some(at) {
                return Ok(None);
            }
            let reason = r.override_reason.clone();
            self.apply_in(&tx, rules.as_ref(), &actor, r, reason.as_deref())
                .await
                .map(Some)
        }
        .await;
        match self.finish_apply(tx, done).await {
            Ok(applied) => Ok(applied.is_some()),
            // Another applier claimed the row first.
            Err(CoreError::Conflict { .. }) | Err(CoreError::NotFound) => Ok(false),
            Err(e) if permanent(&e) => {
                let report = self
                    .check_as(&actor, id, r.override_reason.as_deref())
                    .await
                    .ok();
                self.fail(id, at, &e.to_string(), report).await?;
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    /// Marks release `id` `failed` (only while it is still scheduled for `at`).
    async fn fail(
        &self,
        id: ReleaseId,
        at: Timestamp,
        reason: &str,
        report: Option<ReleaseCheck>,
    ) -> Result<(), CoreError> {
        let tx = self.store.begin().await?;
        let done = async {
            let Some(mut r) = tx.releases().get(id).await? else {
                return Ok(());
            };
            if r.status != ReleaseStatus::Scheduled || r.scheduled_at != Some(at) {
                return Ok(());
            }
            let entries = report
                .map(|c| serde_json::to_value(c.entries).unwrap_or(Value::Null))
                .unwrap_or_else(|| json!([]));
            let failure = json!({
                "at": Timestamp::now().to_rfc3339(),
                "reason": reason,
                "entries": entries,
            });
            tx.releases().unlock_all(id).await?;
            r.status = ReleaseStatus::Failed;
            r.failure = Some(failure.clone());
            r.updated_at = Timestamp::now();
            let r = tx.releases().update(&r).await?;
            tx.audit()
                .append(&audit(None, "release.failed", &r, failure.clone()))
                .await?;
            webhooks::emit(
                tx.as_ref(),
                NewWebhookEvent {
                    kind: "release.failed",
                    content_type: None,
                    data: json!({"release": release_ref(&r), "reason": reason,
                                 "entries": failure["entries"]}),
                },
            )
            .await?;
            tx.jobs()
                .enqueue(&editorial_notify_job("release.failed", None, Some(id)))
                .await?;
            Ok(())
        }
        .await;
        crate::tx::finish(tx, done).await
    }

    /// Applies every scheduled release due by `now` (the `release.apply_due` periodic);
    /// returns how many were applied.
    pub async fn apply_due(&self, now: Timestamp) -> Result<u32, CoreError> {
        let due = self.store.read().releases().due(now, DUE_BATCH).await?;
        let mut n = 0;
        for r in due {
            let Some(at) = r.scheduled_at else { continue };
            match self.apply_scheduled(r.id, at).await {
                Ok(true) => n += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!(release = %r.id, error = %e, "apply_due: skipping"),
            }
        }
        Ok(n)
    }

    /// Applies release `id` in a sandbox as `principal` (a preview, §5.2): every entry
    /// in its own savepoint over the current version, without the approval gates;
    /// entries the principal may not apply are skipped and listed. The caller renders
    /// through the returned store and drops it (rollback).
    pub async fn sandbox(
        &self,
        principal: &Actor,
        id: ReleaseId,
    ) -> Result<(SandboxStore, SandboxReport, Release), CoreError> {
        require_read(principal, Area::Content)?;
        let rules = self.permalinks.pin();
        let sandbox = SandboxStore::begin(self.store.clone()).await?;
        let done = async {
            let tx = sandbox.tx();
            let r = tx.releases().get(id).await?.ok_or(CoreError::NotFound)?;
            if !may_see(principal, &r) {
                return Err(CoreError::NotFound);
            }
            let entries = tx.releases().entries(id).await?;
            let opts = Opts {
                strict: false,
                gates: false,
                override_reason: None,
            };
            let mut report = SandboxReport::default();
            for e in &entries {
                let nested = tx.begin_nested().await?;
                match apply_entry(
                    nested.as_ref(),
                    rules.as_ref(),
                    &self.content,
                    principal,
                    &r,
                    e,
                    opts,
                )
                .await
                {
                    Ok(a) => {
                        nested.commit().await?;
                        report.applied += 1;
                        if a.conflict {
                            report.conflicts.push(e.item_id);
                        }
                    }
                    Err(err) => {
                        if let Err(rb) = nested.rollback().await {
                            tracing::debug!(error = %rb, "preview: savepoint rollback");
                        }
                        let why = match outcome_of(err)? {
                            EntryOutcome::Forbidden { capability } => {
                                format!("missing {capability}")
                            }
                            EntryOutcome::Invalid { message, .. } => message,
                            other => format!("{other:?}"),
                        };
                        report.skipped.push((e.item_id, why));
                    }
                }
            }
            Ok((report, r))
        }
        .await;
        match done {
            Ok((report, r)) => Ok((sandbox, report, r)),
            Err(e) => {
                if let Err(rb) = sandbox.finish().await {
                    tracing::debug!(error = %rb, "preview: sandbox rollback");
                }
                Err(e)
            }
        }
    }
}
