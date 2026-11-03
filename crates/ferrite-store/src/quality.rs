//! The content quality repository (`0019_content_health`, design 16 §7): suppressions,
//! the health job's item pages, runs, findings and the external link cache.

use async_trait::async_trait;
use ferrite_domain::{
    ContentId, ContentItem, Cursor, FindingQuery, HealthCounters, HealthRun, HealthRunId,
    HealthRunStatus, LinkCheck, NewFinding, Page, StoredFinding, Suppression, Timestamp,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{StoreError, validate_json, validate_text};

/// Cursor tag of a run's findings (by `seq`).
pub const FINDING_CURSOR_TAG: &str = "hf:seq";

/// Most findings one `insert_findings` call takes.
pub const MAX_FINDING_BATCH: usize = 1_000;

/// Most URLs one `link_get` call takes.
pub const MAX_LINK_BATCH: usize = 200;

/// The `link_checks` key of `url`: its SHA-256.
pub fn link_url_hash(url: &str) -> Vec<u8> {
    Sha256::digest(url.as_bytes()).to_vec()
}

/// Encodes the findings cursor pointing just after `seq` of `run`.
pub fn encode_finding_cursor(run: HealthRunId, seq: i64) -> String {
    Cursor::encode(FINDING_CURSOR_TAG, &json!(seq), run.as_uuid())
}

/// Decodes a findings cursor into (`run`, `seq`).
pub fn finding_cursor(cursor: &str) -> Result<(HealthRunId, i64), StoreError> {
    match Cursor::decode(cursor, FINDING_CURSOR_TAG)? {
        (serde_json::Value::Number(n), id) => n
            .as_i64()
            .map(|seq| (HealthRunId::from_uuid(id), seq))
            .ok_or_else(|| StoreError::Invalid("cursor does not match the query order".into())),
        _ => Err(StoreError::Invalid(
            "cursor does not match the query order".into(),
        )),
    }
}

/// NUL-checks a suppression.
pub fn check_suppression(s: &Suppression) -> Result<(), StoreError> {
    validate_text("suppression rule", &s.rule)?;
    validate_text("suppression key", &s.key)?;
    validate_text("suppression reason", &s.reason)
}

/// NUL-checks a finding and its `detail`.
pub fn check_finding(f: &NewFinding) -> Result<(), StoreError> {
    for (what, v) in [
        ("finding rule", f.rule.as_str()),
        ("finding severity", &f.severity),
        ("finding category", &f.category),
        ("finding key", &f.key),
    ] {
        validate_text(what, v)?;
    }
    for v in [&f.item_type, &f.locale, &f.context].into_iter().flatten() {
        validate_text("finding text", v)?;
    }
    validate_json(&f.detail)
}

/// NUL-checks a run's texts.
pub fn check_run(r: &HealthRun) -> Result<(), StoreError> {
    validate_text("health run node", &r.node)?;
    if let Some(e) = &r.error {
        validate_text("health run error", e)?;
    }
    validate_json(&r.totals)
}

/// NUL-checks a link check row.
pub fn check_link(l: &LinkCheck) -> Result<(), StoreError> {
    validate_text("link url", &l.url)?;
    validate_text("link host", &l.host)?;
    if let Some(d) = &l.detail {
        validate_text("link detail", d)?;
    }
    Ok(())
}

/// How a run ends ([`QualityRepo::finish_run`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RunEnd {
    pub status: HealthRunStatus,
    pub finished_at: Timestamp,
    pub counters: HealthCounters,
    pub totals: serde_json::Value,
    pub truncated: bool,
    pub error: Option<String>,
}

/// Suppressions, health runs, findings and the link cache. Every text bound is
/// NUL-checked (`Invalid`).
#[async_trait]
pub trait QualityRepo: Send + Sync {
    /// The suppressions of these items (at most `MAX_ID_FILTER` ids), by item, rule, key.
    async fn suppressions_for(&self, items: &[ContentId]) -> Result<Vec<Suppression>, StoreError>;
    /// Inserts or replaces the suppression `(item, rule, key)`.
    async fn suppress(&self, s: &Suppression) -> Result<(), StoreError>;
    /// Deletes a suppression; whether one was there.
    async fn unsuppress(&self, item: ContentId, rule: &str, key: &str) -> Result<bool, StoreError>;
    /// Published and scheduled items of every type but `media`, by id, after `after`.
    async fn published_page(
        &self,
        after: Option<ContentId>,
        limit: u32,
    ) -> Result<Vec<ContentItem>, StoreError>;
    /// The newest revision time of each item that has one (at most `MAX_ID_FILTER` ids).
    async fn last_revision_at(
        &self,
        items: &[ContentId],
    ) -> Result<Vec<(ContentId, Timestamp)>, StoreError>;
    async fn insert_run(&self, run: &HealthRun) -> Result<(), StoreError>;
    /// Ends run `id` (a missing run is `NotFound`).
    async fn finish_run(&self, id: HealthRunId, end: &RunEnd) -> Result<(), StoreError>;
    /// Marks every `running` run `abandoned` at `now`; how many.
    async fn abandon_running(&self, now: Timestamp) -> Result<u64, StoreError>;
    async fn get_run(&self, id: HealthRunId) -> Result<Option<HealthRun>, StoreError>;
    /// The newest run (by `started_at`) in `status`.
    async fn latest_run(&self, status: HealthRunStatus) -> Result<Option<HealthRun>, StoreError>;
    /// Deletes runs started before `before`, except `keep`; how many.
    async fn prune_runs(
        &self,
        before: Timestamp,
        keep: Option<HealthRunId>,
    ) -> Result<u64, StoreError>;
    /// Inserts findings of `run` numbered from `first_seq` (at most [`MAX_FINDING_BATCH`]).
    async fn insert_findings(
        &self,
        run: HealthRunId,
        first_seq: i64,
        findings: &[NewFinding],
    ) -> Result<(), StoreError>;
    /// Deletes the findings of every run but `run`; how many.
    async fn delete_findings_except(&self, run: HealthRunId) -> Result<u64, StoreError>;
    /// A page of findings of `query.run` (required: `Invalid` without one), by `seq`.
    async fn findings(&self, query: &FindingQuery) -> Result<Page<StoredFinding>, StoreError>;
    /// The cached checks of these URLs (at most [`MAX_LINK_BATCH`]).
    async fn link_get(&self, urls: &[String]) -> Result<Vec<LinkCheck>, StoreError>;
    /// Inserts or replaces the check of `check.url`.
    async fn link_put(&self, check: &LinkCheck) -> Result<(), StoreError>;
    /// Deletes checks last seen before `unseen_before`; how many.
    async fn link_prune(&self, unseen_before: Timestamp) -> Result<u64, StoreError>;
}

/// The run id of a cursor, checked against the query's run.
pub fn check_finding_query(q: &FindingQuery) -> Result<(HealthRunId, i64), StoreError> {
    let run = q
        .run
        .ok_or_else(|| StoreError::Invalid("findings need a run".into()))?;
    crate::check_limit(q.limit)?;
    for v in [&q.category, &q.rule, &q.severity, &q.item_type, &q.locale]
        .into_iter()
        .flatten()
    {
        validate_text("finding filter", v)?;
    }
    let after = match &q.cursor {
        None => -1,
        Some(c) => {
            let (r, seq) = finding_cursor(c)?;
            if r != run {
                return Err(StoreError::Invalid(
                    "cursor does not match the query order".into(),
                ));
            }
            seq
        }
    };
    Ok((run, after))
}
