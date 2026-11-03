//! The health report reads and "Run now" (design 16 §10): [`QualityService::summary`],
//! [`QualityService::findings`] and [`QualityService::request_run`]. All need
//! `edit_others`; reads `content:read`, a run `content:write`.

use std::collections::HashMap;

use ferrite_domain::{
    ContentId, ContentStatus, FindingQuery, HealthRun, HealthRunStatus, HealthTrigger, JobId,
    JobQuery, JobStatus, NewAudit, Redirect, RedirectId, StoredFinding, Suppression, Timestamp,
};
use ferrite_store::MAX_ID_FILTER;
use serde_json::json;

use super::{
    QualityService, rule,
    rules::{Category, Severity},
};
use crate::{
    Actor, CoreError,
    actor::{Capability, require, require_reader},
    jobs::{CONTENT_HEALTH_KIND, content_health_job, next_content_health},
    settings,
    tx::finish,
};

/// What the health screen's tiles show.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthSummary {
    /// The newest finished run.
    pub last: Option<HealthRun>,
    /// The run going on now, if any.
    pub running: Option<HealthRun>,
    /// The next periodic run.
    pub next_at: Timestamp,
    /// `health.external_links`.
    pub external_links: bool,
}

/// The item a finding is on, as the report shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRef {
    pub id: ContentId,
    pub kind: String,
    pub title: String,
    pub status: ContentStatus,
    pub locale: String,
}

/// One finding of the report with what it is on.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthFinding {
    pub stored: StoredFinding,
    pub item: Option<ItemRef>,
    pub redirect: Option<Redirect>,
    pub suppressed: Option<Suppression>,
}

/// A page of the newest run's findings.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthFindings {
    pub items: Vec<HealthFinding>,
    pub next_cursor: Option<String>,
}

impl QualityService {
    /// The summary (needs `edit_others` and `content:read`).
    pub async fn summary(&self, actor: &Actor) -> Result<HealthSummary, CoreError> {
        require_reader(actor, Capability::EditOthers)?;
        let repos = self.store.read();
        let (external_links, _) = settings::health_settings(repos).await?;
        Ok(HealthSummary {
            last: repos.quality().latest_run(HealthRunStatus::Done).await?,
            running: repos.quality().latest_run(HealthRunStatus::Running).await?,
            next_at: next_content_health(Timestamp::now()),
            external_links,
        })
    }

    /// A page of the newest done run's findings, filtered by `query` (its `run` is
    /// ignored): each with its item (or redirect) and suppression. No run yet: empty.
    /// An unknown `category` or `severity` is `InvalidField`.
    pub async fn findings(
        &self,
        actor: &Actor,
        mut query: FindingQuery,
    ) -> Result<HealthFindings, CoreError> {
        require_reader(actor, Capability::EditOthers)?;
        if let Some(c) = &query.category
            && Category::parse(c).is_none()
        {
            return Err(CoreError::invalid_field("category", "unknown category"));
        }
        if let Some(s) = &query.severity
            && Severity::parse(s).is_none()
        {
            return Err(CoreError::invalid_field("severity", "unknown severity"));
        }
        if let Some(r) = &query.rule
            && rule(r).is_none()
        {
            return Err(CoreError::invalid_field("rule", "unknown rule"));
        }
        let repos = self.store.read();
        let Some(run) = repos.quality().latest_run(HealthRunStatus::Done).await? else {
            return Ok(HealthFindings {
                items: Vec::new(),
                next_cursor: None,
            });
        };
        query.run = Some(run.id);
        let page = repos.quality().findings(&query).await?;
        let mut ids: Vec<ContentId> = page
            .items
            .iter()
            .filter_map(|f| f.finding.item_id)
            .collect();
        ids.sort();
        ids.dedup();
        let mut items = HashMap::new();
        let mut suppressions: HashMap<ContentId, Vec<Suppression>> = HashMap::new();
        for chunk in ids.chunks(MAX_ID_FILTER) {
            for i in repos.content().get_many(chunk).await? {
                items.insert(
                    i.id,
                    ItemRef {
                        id: i.id,
                        kind: i.kind,
                        title: i.title,
                        status: i.status,
                        locale: i.locale,
                    },
                );
            }
            for s in repos.quality().suppressions_for(chunk).await? {
                suppressions.entry(s.item_id).or_default().push(s);
            }
        }
        let mut redirects: HashMap<RedirectId, Redirect> = HashMap::new();
        for id in page.items.iter().filter_map(|f| f.finding.redirect_id) {
            if let std::collections::hash_map::Entry::Vacant(e) = redirects.entry(id)
                && let Some(r) = repos.redirects().get(id).await?
            {
                e.insert(r);
            }
        }
        let out = page
            .items
            .into_iter()
            .map(|stored| {
                let suppressed = stored.finding.item_id.and_then(|id| {
                    let list = suppressions.get(&id)?;
                    list.iter()
                        .find(|s| {
                            s.rule == stored.finding.rule
                                && (s.key.is_empty() || s.key == stored.finding.key)
                        })
                        .cloned()
                });
                HealthFinding {
                    item: stored
                        .finding
                        .item_id
                        .and_then(|id| items.get(&id).cloned()),
                    redirect: stored
                        .finding
                        .redirect_id
                        .and_then(|id| redirects.get(&id).cloned()),
                    suppressed,
                    stored,
                }
            })
            .collect();
        Ok(HealthFindings {
            items: out,
            next_cursor: page.next_cursor,
        })
    }

    /// "Run now" (needs `edit_others` and `content:write`): enqueues a manual
    /// `content.health` job unless one is queued or running ([`CoreError::InUse`],
    /// `409`). Audited as `health.run`.
    pub async fn request_run(&self, actor: &Actor) -> Result<JobId, CoreError> {
        require(actor, Capability::EditOthers)?;
        let tx = self.store.begin().await?;
        let done = async {
            for status in [JobStatus::Queued, JobStatus::Running] {
                let q = JobQuery {
                    status: Some(status),
                    kind: Some(CONTENT_HEALTH_KIND.to_owned()),
                    limit: 1,
                    cursor: None,
                };
                if !tx.jobs().list(&q).await?.items.is_empty() {
                    return Err(CoreError::InUse(
                        "a content health run is queued or running".into(),
                    ));
                }
            }
            let id = tx
                .jobs()
                .enqueue(&content_health_job(HealthTrigger::Manual, Some(actor.user)))
                .await?
                .ok_or(CoreError::Internal("health job key taken"))?;
            tx.audit()
                .append(&NewAudit {
                    actor_id: Some(actor.user),
                    action: "health.run".into(),
                    target_kind: "job".into(),
                    target_id: Some(id.to_string()),
                    details: json!({}),
                })
                .await?;
            Ok(id)
        }
        .await;
        finish(tx, done).await
    }
}
