//! The artifacts GC (design 03 §6, M4-1 P10–P12): one pass over the `artifacts` rows and
//! the media store, run by the daily `artifacts.gc` job ([`crate::jobs::ArtifactsGcHandler`])
//! and by `ferrite media gc`.
//!
//! A pass, under the `artifacts.gc` lock ([`LOCK_GC`], `try_lock`: a second run is refused):
//!
//! 0. **Protected hashes** are computed here: every 64-lowercase-hex string anywhere in
//!    the payload of a `queued` or `running` job, and the `json`/`html` hashes named by
//!    every import report manifest (`import_runs.report_hash`). A manifest that cannot be
//!    read protects every `import`-kind row for this pass. The store checks the other
//!    references itself (`media`, `media_variants`, `plugins`, `themes`,
//!    `import_runs.report_hash`; [`ArtifactRepo::mark_orphans`]).
//! 1. **Mark:** unreferenced, unprotected rows get `orphaned_at = now`; marked rows that
//!    are referenced or protected again lose their mark.
//! 2. **Delete rows** marked before `now − grace`, one by one with
//!    [`ArtifactRepo::delete_if_orphaned`] (which re-checks the references); only when the
//!    row is gone is its blob deleted (R74's order). A foreign-key refusal (a reference
//!    that raced in) keeps the row.
//! 3. **Delete row-less blobs:** objects under `artifacts/` older than `now − grace`, not
//!    protected and with no row ([`ArtifactRepo::existing`], asked right before each
//!    batch of deletes).
//! 4. **Clean `tmp/`:** staging objects (probes included) older than [`TMP_MAX_AGE`].
//!
//! A dry run marks and deletes nothing: steps 1 and 2 run in a transaction that is rolled
//! back (so their counts are exact), steps 3 and 4 only count.

use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::time::Duration;

use anyhow::Context;
use ferrite_domain::{ArtifactHash, ArtifactKind, JobQuery, JobStatus, MAX_PAGE, Timestamp};
use ferrite_media::{MediaStore, StoredObject};
use ferrite_store::{ArtifactRepo, LOCK_GC, MAX_EXISTING_HASHES, Store, StoreError};
use futures::StreamExt;
use serde_json::Value;

/// `tmp/` objects older than this are deleted, whatever the grace period.
pub const TMP_MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// The largest report manifest read (it holds two hashes).
const MAX_MANIFEST_BYTES: u64 = 1024;

/// What one pass does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcOptions {
    /// How long a row stays marked, and a row-less blob stays after it was written,
    /// before it is deleted (`media.gc_grace_days`, `--grace-days`).
    pub grace: Duration,
    /// Count only: mark nothing, delete nothing.
    pub dry_run: bool,
    /// The pass's clock: marks are set to it and ages are measured from it.
    pub now: Timestamp,
}

/// What one pass did (or, for a dry run, would do).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GcReport {
    pub marked: u64,
    pub unmarked: u64,
    pub deleted_rows: u64,
    /// The rows' recorded sizes.
    pub deleted_row_bytes: u64,
    /// Blobs under `artifacts/` with no row.
    pub deleted_blobs: u64,
    pub deleted_blob_bytes: u64,
    pub deleted_tmp: u64,
}

impl fmt::Display for GcReport {
    /// The P12 output line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "marked {}, unmarked {}, deleted rows {} ({} bytes), deleted unrecorded blobs {} ({} bytes), deleted tmp {}",
            self.marked,
            self.unmarked,
            self.deleted_rows,
            self.deleted_row_bytes,
            self.deleted_blobs,
            self.deleted_blob_bytes,
            self.deleted_tmp
        )
    }
}

/// Runs one pass; `Ok(None)` when another GC holds the lock (nothing was done).
///
/// A blob that cannot be deleted is logged and left (a later pass finds it again as a
/// row-less blob); a store or listing error ends the pass with the error.
pub async fn run(
    store: &dyn Store,
    media: &MediaStore,
    opts: &GcOptions,
) -> anyhow::Result<Option<GcReport>> {
    let Some(guard) = store.try_lock(LOCK_GC).await? else {
        return Ok(None);
    };
    let done = pass(store, media, opts).await;
    if let Err(e) = guard.release().await {
        tracing::warn!(error = %e, "cannot release the artifacts GC lock");
    }
    let report = done?;
    tracing::info!(
        dry_run = opts.dry_run,
        marked = report.marked,
        unmarked = report.unmarked,
        deleted_rows = report.deleted_rows,
        deleted_row_bytes = report.deleted_row_bytes,
        deleted_blobs = report.deleted_blobs,
        deleted_blob_bytes = report.deleted_blob_bytes,
        deleted_tmp = report.deleted_tmp,
        "artifacts GC pass"
    );
    Ok(Some(report))
}

async fn pass(store: &dyn Store, media: &MediaStore, opts: &GcOptions) -> anyhow::Result<GcReport> {
    let protected = protected(store, media).await?;
    let protected_list: Vec<ArtifactHash> = protected.iter().cloned().collect();
    let cutoff = before(opts.now, opts.grace)?;
    let mut report = GcReport::default();

    if opts.dry_run {
        let tx = store.begin().await?;
        let rows = rows(
            tx.artifacts(),
            None,
            opts.now,
            cutoff,
            &protected_list,
            &mut report,
        )
        .await;
        tx.rollback().await?;
        rows?;
    } else {
        rows(
            store.read().artifacts(),
            Some(media),
            opts.now,
            cutoff,
            &protected_list,
            &mut report,
        )
        .await?;
    }
    unrecorded(store, media, opts.dry_run, cutoff, &protected, &mut report).await?;
    tmp(
        media,
        opts.dry_run,
        before(opts.now, TMP_MAX_AGE)?,
        &mut report,
    )
    .await?;
    Ok(report)
}

/// A grace period of `days` days (`media.gc_grace_days`, `--grace-days`).
pub fn grace_days(days: u32) -> Duration {
    Duration::from_secs(u64::from(days) * 86_400)
}

/// `now - by`.
fn before(now: Timestamp, by: Duration) -> anyhow::Result<Timestamp> {
    let by = i64::try_from(by.as_micros()).context("the grace period is too long")?;
    Ok(Timestamp::from_micros(now.as_micros().saturating_sub(by))?)
}

/// P10's hashes that only Rust can see: unfinished job payloads and report manifests.
async fn protected(
    store: &dyn Store,
    media: &MediaStore,
) -> anyhow::Result<BTreeSet<ArtifactHash>> {
    let mut out = BTreeSet::new();
    // Queued first, then running: a job claimed while the queued pages are read is
    // still found among the running ones.
    for status in [JobStatus::Queued, JobStatus::Running] {
        let mut cursor = None;
        loop {
            let page = store
                .read()
                .jobs()
                .list(&JobQuery {
                    status: Some(status),
                    kind: None,
                    limit: MAX_PAGE,
                    cursor,
                })
                .await?;
            for job in &page.items {
                hashes_in(&job.payload, &mut out);
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
    }

    let mut unreadable = false;
    let mut cursor = None;
    loop {
        let runs = store
            .read()
            .imports()
            .list_runs_page(MAX_PAGE, cursor)
            .await?;
        for run in &runs {
            let Some(hash) = &run.report_hash else {
                continue;
            };
            match manifest_hashes(media, hash).await {
                Ok(named) => out.extend(named),
                Err(why) => {
                    tracing::warn!(run = %run.id, manifest = %hash, error = %why, "cannot read an import report manifest; every import artifact is kept this pass");
                    unreadable = true;
                }
            }
        }
        match runs.last() {
            Some(last) if runs.len() == MAX_PAGE as usize => {
                cursor = Some((last.started_at, last.id.as_uuid()));
            }
            _ => break,
        }
    }
    if unreadable {
        let mut after: Option<ArtifactHash> = None;
        loop {
            let page = store
                .read()
                .artifacts()
                .list_page(after.as_ref(), MAX_PAGE)
                .await?;
            out.extend(
                page.iter()
                    .filter(|a| a.kind == ArtifactKind::Import)
                    .map(|a| a.hash.clone()),
            );
            match page.last() {
                Some(last) if page.len() == MAX_PAGE as usize => after = Some(last.hash.clone()),
                _ => break,
            }
        }
    }
    Ok(out)
}

/// Every string (object keys included) in `value` that is a SHA-256 hex hash.
fn hashes_in(value: &Value, out: &mut BTreeSet<ArtifactHash>) {
    let mut add = |s: &str| {
        if let Ok(hash) = ArtifactHash::parse(s) {
            out.insert(hash);
        }
    };
    match value {
        Value::String(s) => add(s),
        Value::Array(items) => items.iter().for_each(|v| hashes_in(v, out)),
        Value::Object(map) => {
            for (k, v) in map {
                if let Ok(hash) = ArtifactHash::parse(k) {
                    out.insert(hash);
                }
                hashes_in(v, out);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// The `json` and `html` hashes of report manifest `hash` (`{"v":1,"json":…,"html":…}`).
async fn manifest_hashes(media: &MediaStore, hash: &str) -> anyhow::Result<[ArtifactHash; 2]> {
    let hash = ArtifactHash::parse(hash)?;
    match media.head(&hash).await? {
        None => anyhow::bail!("the manifest is missing"),
        Some(n) if n > MAX_MANIFEST_BYTES => anyhow::bail!("the manifest is {n} bytes"),
        Some(_) => {}
    }
    let manifest: Value = serde_json::from_slice(&media.get(&hash).await?)?;
    let field = |name: &str| -> anyhow::Result<ArtifactHash> {
        let s = manifest
            .get(name)
            .and_then(Value::as_str)
            .with_context(|| format!("the manifest has no {name:?}"))?;
        Ok(ArtifactHash::parse(s)?)
    };
    Ok([field("json")?, field("html")?])
}

/// Steps 1 and 2 on `artifacts` (the store's, or a dry run's transaction's); blobs are
/// deleted only with `media`.
async fn rows(
    artifacts: &dyn ArtifactRepo,
    media: Option<&MediaStore>,
    now: Timestamp,
    cutoff: Timestamp,
    protected: &[ArtifactHash],
    report: &mut GcReport,
) -> anyhow::Result<()> {
    let counts = artifacts.mark_orphans(now, protected).await?;
    report.marked = counts.marked;
    report.unmarked = counts.unmarked;
    loop {
        let batch = artifacts.orphaned_before(cutoff, MAX_PAGE).await?;
        let mut progressed = false;
        for row in &batch {
            let deleted = match artifacts
                .delete_if_orphaned(&row.hash, cutoff, protected)
                .await
            {
                Ok(deleted) => deleted,
                // A reference with a foreign key raced in: the row stays.
                Err(StoreError::ForeignKey { .. }) => false,
                Err(e) => return Err(e.into()),
            };
            if !deleted {
                continue;
            }
            progressed = true;
            report.deleted_rows += 1;
            report.deleted_row_bytes += u64::try_from(row.bytes).unwrap_or(0);
            if let Some(media) = media {
                delete_blob(media, &row.hash).await;
            }
        }
        // A short batch is the last one; a batch where every row was kept would come
        // back unchanged, so the pass stops there (the next pass sees them again).
        if batch.len() < MAX_PAGE as usize || !progressed {
            return Ok(());
        }
    }
}

async fn delete_blob(media: &MediaStore, hash: &ArtifactHash) -> bool {
    match media.delete(hash).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(artifact = %hash, error = %e, "cannot delete an artifact blob; a later GC pass retries");
            false
        }
    }
}

/// Step 3: blobs with no row, older than `cutoff` and not protected.
async fn unrecorded(
    store: &dyn Store,
    media: &MediaStore,
    dry_run: bool,
    cutoff: Timestamp,
    protected: &BTreeSet<ArtifactHash>,
    report: &mut GcReport,
) -> anyhow::Result<()> {
    let mut listing = media.list_artifacts();
    let mut batch: Vec<(ArtifactHash, u64)> = Vec::new();
    while let Some(object) = listing.next().await {
        let object = object.context("listing the media store's artifacts")?;
        let StoredObject {
            hash: Some(hash),
            size,
            last_modified,
            ..
        } = object
        else {
            continue;
        };
        if last_modified.as_micros() >= cutoff.as_micros() || protected.contains(&hash) {
            continue;
        }
        batch.push((hash, size));
        if batch.len() == MAX_EXISTING_HASHES {
            unrecorded_batch(store, media, dry_run, &mut batch, report).await?;
        }
    }
    unrecorded_batch(store, media, dry_run, &mut batch, report).await
}

async fn unrecorded_batch(
    store: &dyn Store,
    media: &MediaStore,
    dry_run: bool,
    batch: &mut Vec<(ArtifactHash, u64)>,
    report: &mut GcReport,
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let hashes: Vec<ArtifactHash> = batch.iter().map(|(h, _)| h.clone()).collect();
    let recorded: HashSet<ArtifactHash> = store
        .read()
        .artifacts()
        .existing(&hashes)
        .await?
        .into_iter()
        .collect();
    for (hash, size) in batch.drain(..) {
        if recorded.contains(&hash) {
            continue;
        }
        if dry_run || delete_blob(media, &hash).await {
            report.deleted_blobs += 1;
            report.deleted_blob_bytes += size;
        }
    }
    Ok(())
}

/// Step 4: `tmp/` objects last modified before `cutoff`.
async fn tmp(
    media: &MediaStore,
    dry_run: bool,
    cutoff: Timestamp,
    report: &mut GcReport,
) -> anyhow::Result<()> {
    let mut listing = media.list_tmp();
    while let Some(object) = listing.next().await {
        let object = object.context("listing the media store's tmp/")?;
        if object.last_modified.as_micros() >= cutoff.as_micros() {
            continue;
        }
        if dry_run {
            report.deleted_tmp += 1;
            continue;
        }
        match media.delete_key(&object.key).await {
            Ok(()) => report.deleted_tmp += 1,
            Err(e) => {
                tracing::warn!(key = %object.key, error = %e, "cannot delete a staging object; a later GC pass retries");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hashes_anywhere_in_a_payload() {
        let (a, b, c) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let payload = json!({
            "v": 1,
            "wxr": a,
            "deep": [1, {"x": b}, null, true],
            c.clone(): "key",
            "upper": "A".repeat(64),
            "short": "a".repeat(63),
        });
        let mut out = BTreeSet::new();
        hashes_in(&payload, &mut out);
        let got: Vec<&str> = out.iter().map(ArtifactHash::as_str).collect();
        assert_eq!(got, [a.as_str(), b.as_str(), c.as_str()]);
    }

    #[test]
    fn report_line() {
        let r = GcReport {
            marked: 1,
            unmarked: 2,
            deleted_rows: 3,
            deleted_row_bytes: 4,
            deleted_blobs: 5,
            deleted_blob_bytes: 6,
            deleted_tmp: 7,
        };
        assert_eq!(
            r.to_string(),
            "marked 1, unmarked 2, deleted rows 3 (4 bytes), deleted unrecorded blobs 5 (6 bytes), deleted tmp 7"
        );
    }

    #[test]
    fn before_subtracts() {
        let now = Timestamp::from_micros(10_000_000).unwrap();
        assert_eq!(
            before(now, Duration::from_secs(4)).unwrap().as_micros(),
            6_000_000
        );
        assert!(before(now, Duration::from_secs(u64::MAX)).is_err());
    }
}
