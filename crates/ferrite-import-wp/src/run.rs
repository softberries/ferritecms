//! The run driver: [`run`] imports one WXR file.
//!
//! It reads the file's channel, starts a fenced run (P5), parses the file into the IR at
//! `<work_dir>/<run-id>/ir.sqlite` (P21), runs passes 1–4 and the fix-up, and finishes
//! the run `done` (deleting the folder) or `failed` (keeping it). A later run of the same
//! file (same SHA-256) takes over the folder of a run that did not finish: it skips the
//! parse and every pass that ended there; inside a pass `import_map` makes redone batches
//! harmless (P3).
//!
//! Passes write through [`Session`]: its batches commit, or roll back on a dry run.
//! [`ImportOptions::on_step`] hears each step as it begins; a run that fails after it
//! started still returns its report, in [`RunFailed`] (P29).

use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use ferrite_core::import::{
    ImportBatch, ImportHandle, ImportService, STEP_FIXUP, STEP_PARSE as PARSE, STEP_PASS1,
    STEP_PASS2, STEP_PASS3, STEP_PASS4, STEP_REPORT as REPORT,
};
use ferrite_core::{Actor, SwappablePermalinks};
use ferrite_domain::{ImportRunState, JobId, Timestamp};
use ferrite_media::MediaStore;
use futures::Stream;
use sha2::{Digest, Sha256};

use crate::ImportError;
use crate::ir::Ir;
use crate::model::{Channel, Record};
use crate::wxr;

/// The IR file in a run's folder.
const IR_FILE: &str = "ir.sqlite";
/// The IR `meta` key holding the hex SHA-256 of the WXR file.
const DIGEST_KEY: &str = "wxr_sha256";
/// The `DIGEST_KEY` of a finished run's work file: no later run resumes it.
const FINISHED: &str = "finished";
/// The `DIGEST_KEY` of a dry run's work file: never resumed, even when deleting it failed.
const DRY_RUN: &str = "dry-run";
/// Records in flight between the parser thread and the IR writer. An item may hold
/// up to 64 MiB of text, so the queue is short: memory stays near a few items.
const PARSE_QUEUE: usize = 8;
/// How often the parse proves the run is alive (runs go stale after 120 s, P5).
const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);

/// The bytes of a media file.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

/// Where attachment files come from: an uploads folder ([`DirSource`]) or the old site.
#[async_trait]
pub trait MediaSource: Send + Sync {
    /// The file at `rel_path` (relative to `wp-content/uploads`, from
    /// `_wp_attached_file`), whose URL was `url`; `None` when it is missing. `max` is the
    /// most bytes the import accepts; a source may refuse a larger file early.
    async fn open(
        &self,
        rel_path: &str,
        url: &str,
        max: u64,
    ) -> Result<Option<ByteStream>, ImportError>;
}

/// A local copy of `wp-content/uploads`.
#[derive(Debug, Clone)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl MediaSource for DirSource {
    /// Opens `root/rel_path`. A path that is not plainly relative (`..`, absolute, empty)
    /// cannot be inside the folder, so it is missing too; `max` is left to the caller.
    async fn open(
        &self,
        rel_path: &str,
        _url: &str,
        _max: u64,
    ) -> Result<Option<ByteStream>, ImportError> {
        let rel = Path::new(rel_path);
        let plain = rel.components().next().is_some()
            && rel.components().all(|c| matches!(c, Component::Normal(_)));
        if !plain {
            return Ok(None);
        }
        let path = self.root.join(rel);
        let file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ImportError::File { path, source }),
        };
        match file.metadata().await {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Ok(None),
            Err(source) => return Err(ImportError::File { path, source }),
        }
        Ok(Some(Box::pin(tokio_util::io::ReaderStream::new(file))))
    }
}

/// Called with a step's name as it begins.
pub type OnStep = Arc<dyn Fn(&str) + Send + Sync>;

/// How to import.
#[derive(Clone)]
pub struct ImportOptions {
    /// Run everything, then roll every batch back.
    pub dry_run: bool,
    /// Adopt the WP permalink structure (P17).
    pub keep_permalinks: bool,
    /// Import the site title and description (P20).
    pub site_options: bool,
    /// The WP category base, for redirects.
    pub category_base: String,
    /// The WP tag base, for redirects.
    pub tag_base: String,
    /// Take over a stale running run (P5).
    pub takeover: bool,
    /// The largest attachment file accepted (`media.max_upload`, P13).
    pub max_upload: u64,
    /// Called as each step that runs begins: `parse`, `pass1`…`pass4`, `fixup`, then
    /// `report` (P28). Steps an earlier run finished are skipped without a call.
    pub on_step: Option<OnStep>,
    /// The `import.wordpress` job running the import, recorded on its run (P27).
    pub job_id: Option<JobId>,
    /// Items per batch instead of [`ImportBatch::BATCH_ITEMS`].
    #[cfg(feature = "test-seams")]
    pub batch_items: Option<usize>,
    /// Fail with [`ImportError::Stopped`] once this many batches have ended.
    #[cfg(feature = "test-seams")]
    pub fail_after_batches: Option<u32>,
    /// Called before each step (`parse`, `pass1`…`pass4`, `fixup`) that runs.
    #[cfg(feature = "test-seams")]
    pub step_hook: Option<StepHook>,
    /// Ends the run `done` right after this step, as if the later ones had nothing to
    /// do.
    #[cfg(feature = "test-seams")]
    pub stop_after: Option<&'static str>,
}

/// The default of [`ImportOptions::max_upload`], as `media.max_upload`'s.
pub const DEFAULT_MAX_UPLOAD: u64 = 256 << 20;

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            keep_permalinks: true,
            site_options: true,
            category_base: "category".into(),
            tag_base: "tag".into(),
            takeover: false,
            max_upload: DEFAULT_MAX_UPLOAD,
            on_step: None,
            job_id: None,
            #[cfg(feature = "test-seams")]
            batch_items: None,
            #[cfg(feature = "test-seams")]
            fail_after_batches: None,
            #[cfg(feature = "test-seams")]
            step_hook: None,
            #[cfg(feature = "test-seams")]
            stop_after: None,
        }
    }
}

impl std::fmt::Debug for ImportOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("ImportOptions");
        d.field("dry_run", &self.dry_run)
            .field("keep_permalinks", &self.keep_permalinks)
            .field("site_options", &self.site_options)
            .field("category_base", &self.category_base)
            .field("tag_base", &self.tag_base)
            .field("takeover", &self.takeover)
            .field("max_upload", &self.max_upload)
            .field("on_step", &self.on_step.as_ref().map(|_| "Fn"))
            .field("job_id", &self.job_id);
        #[cfg(feature = "test-seams")]
        d.field("batch_items", &self.batch_items)
            .field("fail_after_batches", &self.fail_after_batches)
            .field("step_hook", &self.step_hook)
            .field("stop_after", &self.stop_after);
        d.finish()
    }
}

/// The function a [`StepHook`] calls.
#[cfg(feature = "test-seams")]
type StepFn = dyn Fn(&str) -> Result<(), ImportError> + Send + Sync;

/// A test sink called with each step's name before it runs; an `Err` fails the run.
#[cfg(feature = "test-seams")]
#[derive(Clone)]
pub struct StepHook(Arc<StepFn>);

#[cfg(feature = "test-seams")]
impl StepHook {
    pub fn new(f: impl Fn(&str) -> Result<(), ImportError> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
}

#[cfg(feature = "test-seams")]
impl std::fmt::Debug for StepHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StepHook")
    }
}

/// What a run works with.
#[derive(Clone)]
pub struct ImportContext {
    /// Built with [`permalinks`](Self::permalinks) as its rules.
    pub import: Arc<ImportService>,
    /// The rules given to [`ImportService::new`] (the same `Arc`): [`Session::end`]
    /// swaps them after a batch wrote `permalink.*`, so later batches pin the new ones.
    /// A dry run swaps in the rules of its rolled-back rows, so give a run its own
    /// rules, never a serving node's.
    pub permalinks: Arc<SwappablePermalinks>,
    pub media_store: Arc<MediaStore>,
    /// Who imports; needs the `import` capability.
    pub actor: Actor,
    /// Where run folders go (`<data>/import`).
    pub work_dir: PathBuf,
    pub media_source: Arc<dyn MediaSource>,
    /// The new site's URL.
    pub site_url: String,
}

pub use crate::report::{Counts, Failure, MissingMedia, MustReset, Report, UnmappedAcf};

/// A run that failed. `report` is set once the run had started (P29): what was done until
/// the failure, with [`Report::failed`] naming the step; errors before that (an unreadable
/// file, a busy run) have none. The report is boxed: it is large, and errors travel by value.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct RunFailed {
    pub error: ImportError,
    pub report: Option<Box<Report>>,
}

impl From<ImportError> for RunFailed {
    fn from(error: ImportError) -> Self {
        Self {
            error,
            report: None,
        }
    }
}

/// A started run as the passes see it: the IR, the options, and batches that end as the
/// run requires.
pub struct Session<'a> {
    ctx: &'a ImportContext,
    opts: &'a ImportOptions,
    handle: &'a ImportHandle,
    ir: &'a Ir,
    ended: AtomicU32,
}

impl<'a> Session<'a> {
    pub fn ctx(&self) -> &'a ImportContext {
        self.ctx
    }

    pub fn opts(&self) -> &'a ImportOptions {
        self.opts
    }

    pub fn ir(&self) -> &'a Ir {
        self.ir
    }

    /// The run's work folder (`<work_dir>/<run-id>`), deleted when the run ends `done`.
    pub fn run_dir(&self) -> PathBuf {
        self.ctx.work_dir.join(self.handle.run().id.to_string())
    }

    /// The most items a batch should hold.
    pub fn batch_items(&self) -> usize {
        #[cfg(feature = "test-seams")]
        if let Some(n) = self.opts.batch_items {
            return n.max(1);
        }
        ImportBatch::BATCH_ITEMS
    }

    /// Opens a content batch. Any `Err` from a batch method means the batch must be
    /// rolled back (dropping it does that too); [`in_batch`](Self::in_batch) does it.
    pub async fn batch(&self) -> Result<ImportBatch<'a>, ImportError> {
        Ok(self.handle.batch(&self.ctx.import).await?)
    }

    /// Opens a schema batch (types, taxonomies, `permalink.*` options).
    pub async fn schema_batch(&self) -> Result<ImportBatch<'a>, ImportError> {
        Ok(self.handle.schema_batch(&self.ctx.import).await?)
    }

    /// Runs `f` on a new content batch, then [`end`](Self::end)s it, or rolls it back
    /// when `f` fails (the batch error contract).
    pub async fn in_batch<T>(
        &self,
        f: impl AsyncFnOnce(&mut ImportBatch<'a>) -> Result<T, ImportError>,
    ) -> Result<T, ImportError> {
        let batch = self.batch().await?;
        self.run_batch(batch, f).await
    }

    /// [`in_batch`](Self::in_batch) on a schema batch.
    pub async fn in_schema_batch<T>(
        &self,
        f: impl AsyncFnOnce(&mut ImportBatch<'a>) -> Result<T, ImportError>,
    ) -> Result<T, ImportError> {
        let batch = self.schema_batch().await?;
        self.run_batch(batch, f).await
    }

    /// Ends `batch` when `result` is `Ok` and hands back its value, or rolls the batch
    /// back and hands back the error: [`in_batch`](Self::in_batch) for a batch the
    /// caller wrote with plain async fns. Passes use this form, because the future of
    /// an async closure over `&mut ImportBatch` is not `Send` for every lifetime
    /// (rustc's higher-ranked check), and [`run`] must be `Send`.
    pub async fn finish_batch<T>(
        &self,
        batch: ImportBatch<'a>,
        result: Result<T, ImportError>,
    ) -> Result<T, ImportError> {
        match result {
            Ok(value) => {
                self.end(batch).await?;
                Ok(value)
            }
            Err(e) => {
                if let Err(r) = batch.rollback().await {
                    tracing::warn!(error = %r, "rolling back a failed import batch failed");
                }
                Err(e)
            }
        }
    }

    async fn run_batch<T>(
        &self,
        mut batch: ImportBatch<'a>,
        f: impl AsyncFnOnce(&mut ImportBatch<'a>) -> Result<T, ImportError>,
    ) -> Result<T, ImportError> {
        let result = f(&mut batch).await;
        self.finish_batch(batch, result).await
    }

    /// Ends `batch`: commits it, or rolls it back on a dry run. When it wrote
    /// `permalink.*`, the rules are rebuilt (from the stored options; on a dry run from
    /// the rolled-back rows) and swapped in for the batches after it.
    pub async fn end(&self, batch: ImportBatch<'_>) -> Result<(), ImportError> {
        let changes = batch.permalink_changes().to_vec();
        if self.opts.dry_run {
            batch.rollback().await?;
        } else {
            batch.commit().await?;
        }
        if !changes.is_empty() {
            let overrides: Vec<(&str, Option<&str>)> = if self.opts.dry_run {
                changes
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_ref().and_then(|v| v.as_str())))
                    .collect()
            } else {
                Vec::new()
            };
            let rules = self.ctx.import.permalink_rules(&overrides).await?;
            self.ctx.permalinks.swap(rules);
        }
        let ended = self.ended.fetch_add(1, Ordering::Relaxed) + 1;
        #[cfg(feature = "test-seams")]
        if self.opts.fail_after_batches.is_some_and(|n| ended >= n) {
            return Err(ImportError::Stopped);
        }
        tracing::trace!(batches = ended, "import batch ended");
        Ok(())
    }

    /// Proves the run is still this one's and moves its heartbeat, for long steps between
    /// batches (a run goes stale after 120 s). Only with no batch open: on SQLite the
    /// fence's write would wait for that batch's transaction (one writer).
    pub async fn heartbeat(&self) -> Result<(), ImportError> {
        Ok(self.handle.heartbeat(&self.ctx.import).await?)
    }
}

/// Imports the WXR file `wxr` (see the module docs). Errors before the run starts (an
/// unreadable file, a busy run) leave no run and no report; later ones finish it `failed`
/// and come with the report of what was done.
pub async fn run(ctx: ImportContext, wxr: &Path, opts: ImportOptions) -> Result<Report, RunFailed> {
    let (digest, channel) = inspect(wxr).await?;
    let source = normalise_source(&channel.base_blog_url)?;
    let handle = ctx
        .import
        .start(
            &ctx.actor,
            &source,
            opts.dry_run,
            opts.takeover,
            opts.job_id,
        )
        .await
        .map_err(ImportError::from)?;
    let dir = ctx.work_dir.join(handle.run().id.to_string());
    let mut report = Report::default();
    report.dry_run = opts.dry_run;
    let outcome = drive(&ctx, &opts, &handle, wxr, &digest, &dir, &mut report).await;
    // A dry run's work file holds rolled-back work: it goes on every exit (a real
    // run's stays when it fails, for the next run to resume).
    if opts.dry_run || outcome.is_ok() {
        remove_work_dir(&dir).await;
    }
    report.run_id = handle.run().id.to_string();
    report.source.clone_from(&handle.run().source);
    report.started_at = Some(handle.run().started_at);
    report.finished_at = Some(Timestamp::now());
    report.site_url.clone_from(&ctx.site_url);
    let error = match outcome {
        Ok(()) => {
            let stats = serde_json::to_value(&report.counts).map_err(ImportError::from);
            let done = match stats {
                Ok(stats) => handle
                    .finish(&ctx.import, ImportRunState::Done, stats)
                    .await
                    .map_err(ImportError::from),
                Err(e) => Err(e),
            };
            match done {
                Ok(()) => return Ok(report),
                // The work is done (its work file marked finished and removed) but the
                // run could not be closed (taken over): a re-run starts over.
                Err(error) => {
                    crate::report::mark_failed(
                        &mut report,
                        REPORT,
                        &error,
                        crate::report::Rerun::StartsOver,
                    );
                    return Err(RunFailed {
                        error,
                        report: Some(Box::new(report)),
                    });
                }
            }
        }
        Err(error) => error,
    };
    let stats = serde_json::json!({ "error": error.to_string() });
    if let Err(f) = handle
        .finish(&ctx.import, ImportRunState::Failed, stats)
        .await
    {
        tracing::warn!(error = %f, "marking the import run failed did not work");
    }
    Err(RunFailed {
        error,
        report: Some(Box::new(report)),
    })
}

/// Removes a run's work folder; a failure is logged (the folder's IR is marked so no run
/// resumes it).
async fn remove_work_dir(dir: &Path) {
    match tokio::fs::remove_dir_all(dir).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(error = %e, dir = %dir.display(), "removing the import's work folder failed");
        }
        _ => {}
    }
}

/// Opens the IR (reused or new), runs the steps into `report` and closes it. On a
/// failure `report` is still finished, from what the IR holds, with the failed step.
async fn drive(
    ctx: &ImportContext,
    opts: &ImportOptions,
    handle: &ImportHandle,
    wxr: &Path,
    digest: &str,
    dir: &Path,
    report: &mut Report,
) -> Result<(), ImportError> {
    let mut progress = Progress {
        current: PARSE,
        ..Progress::default()
    };
    let mut ir = match prepare_ir(&ctx.work_dir, dir, digest, opts.dry_run).await {
        Ok(ir) => ir,
        Err(e) => {
            crate::report::finish_failed(report, None, PARSE, &e).await;
            return Err(e);
        }
    };
    let mut done = run_steps(ctx, opts, handle, wxr, &mut ir, &mut progress, report).await;
    if progress.skipped {
        report.resumed = Some(std::mem::take(&mut progress.run));
    }
    if done.is_ok() {
        progress.current = REPORT;
        on_step(opts, REPORT);
        done = match handle.set_step(&ctx.import, REPORT).await {
            Ok(()) => crate::report::finish(report, &ir).await,
            Err(e) => Err(e.into()),
        };
    }
    match &done {
        // A finished run's work file is never resumed, even if deleting its folder
        // fails: it no longer names the file it holds.
        Ok(()) => {
            if let Err(e) = ir.set_meta(DIGEST_KEY, FINISHED).await {
                tracing::warn!(error = %e, "marking the import's work file finished failed");
            }
        }
        Err(e) => crate::report::finish_failed(report, Some(&ir), progress.current, e).await,
    }
    ir.close().await;
    done
}

/// The steps this process ran, whether any was skipped as done by an earlier run, and
/// the step running now.
#[derive(Default)]
struct Progress {
    run: Vec<String>,
    skipped: bool,
    current: &'static str,
}

/// [`begin_step`] for a pass, recording in `progress` whether it runs.
async fn begin_pass(
    progress: &mut Progress,
    run: Fence<'_>,
    opts: &ImportOptions,
    ir: &Ir,
    name: &'static str,
) -> Result<bool, ImportError> {
    let runs = begin_step(progress, run, opts, ir, name).await?;
    if runs {
        progress.run.push(name.to_owned());
    } else {
        progress.skipped = true;
    }
    Ok(runs)
}

async fn run_steps(
    ctx: &ImportContext,
    opts: &ImportOptions,
    handle: &ImportHandle,
    wxr: &Path,
    ir: &mut Ir,
    progress: &mut Progress,
    report: &mut Report,
) -> Result<(), ImportError> {
    let fence = Fence { ctx, handle };
    if begin_step(progress, fence, opts, ir, PARSE).await? {
        parse_into(ir, wxr, ctx, handle).await?;
        ir.mark_pass(PARSE).await?;
    }
    let session = Session {
        ctx,
        opts,
        handle,
        ir,
        ended: AtomicU32::new(0),
    };
    if begin_pass(progress, fence, opts, ir, STEP_PASS1).await? {
        crate::pass::pass1(&session, report).await?;
        end_step(opts, ir, STEP_PASS1).await?;
    }
    if stops_after(opts, STEP_PASS1) {
        return Ok(());
    }
    if begin_pass(progress, fence, opts, ir, STEP_PASS2).await? {
        crate::pass::pass2(&session, report).await?;
        end_step(opts, ir, STEP_PASS2).await?;
    }
    if stops_after(opts, STEP_PASS2) {
        return Ok(());
    }
    if begin_pass(progress, fence, opts, ir, STEP_PASS3).await? {
        crate::pass::content::import(&session, report).await?;
        end_step(opts, ir, STEP_PASS3).await?;
    }
    if begin_pass(progress, fence, opts, ir, STEP_PASS4).await? {
        crate::pass::pass4(&session, report).await?;
        end_step(opts, ir, STEP_PASS4).await?;
    }
    if begin_pass(progress, fence, opts, ir, STEP_FIXUP).await? {
        crate::pass::fixup::run(&session, report).await?;
        end_step(opts, ir, STEP_FIXUP).await?;
    }
    Ok(())
}

/// Whether the run ends after step `name` (the `stop_after` test seam).
fn stops_after(opts: &ImportOptions, name: &str) -> bool {
    #[cfg(feature = "test-seams")]
    return opts.stop_after == Some(name);
    #[cfg(not(feature = "test-seams"))]
    {
        let _ = (opts, name);
        false
    }
}

/// What records a run's step: its service and handle.
#[derive(Clone, Copy)]
struct Fence<'a> {
    ctx: &'a ImportContext,
    handle: &'a ImportHandle,
}

/// Whether step `name` still has to run (it did not finish on this IR); it becomes
/// `progress.current`, and when it runs it is written to the run (fenced, P28; no batch
/// is open between steps, so SQLite's one writer is free) and `on_step` and the test
/// hook hear of it.
async fn begin_step(
    progress: &mut Progress,
    run: Fence<'_>,
    opts: &ImportOptions,
    ir: &Ir,
    name: &'static str,
) -> Result<bool, ImportError> {
    // Set first, so an error checking the step names it.
    progress.current = name;
    if ir.pass_done(name).await? {
        tracing::debug!(step = name, "import step already done; skipped");
        return Ok(false);
    }
    run.handle.set_step(&run.ctx.import, name).await?;
    on_step(opts, name);
    #[cfg(feature = "test-seams")]
    if let Some(hook) = &opts.step_hook {
        (hook.0)(name)?;
    }
    Ok(true)
}

/// Tells [`ImportOptions::on_step`] that step `name` begins.
fn on_step(opts: &ImportOptions, name: &str) {
    if let Some(f) = &opts.on_step {
        f(name);
    }
}

/// Records a finished pass. A dry run rolled its writes back, so it records none.
async fn end_step(opts: &ImportOptions, ir: &Ir, name: &str) -> Result<(), ImportError> {
    if !opts.dry_run {
        ir.mark_pass(name).await?;
    }
    Ok(())
}

/// The hex SHA-256 of the file and its channel, read on a blocking thread.
async fn inspect(wxr: &Path) -> Result<(String, Channel), ImportError> {
    let path = wxr.to_owned();
    tokio::task::spawn_blocking(move || {
        let digest = sha256_file(&path)?;
        let channel = read_channel(&path)?;
        Ok((digest, channel))
    })
    .await
    .map_err(|e| ImportError::Io {
        offset: 0,
        msg: format!("the file check stopped: {e}"),
    })?
}

fn open_file(path: &Path) -> Result<std::fs::File, ImportError> {
    std::fs::File::open(path).map_err(|source| ImportError::File {
        path: path.to_owned(),
        source,
    })
}

fn sha256_file(path: &Path) -> Result<String, ImportError> {
    let mut file = open_file(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|source| ImportError::File {
            path: path.to_owned(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The channel, the parse's first record; the parse stops there.
fn read_channel(path: &Path) -> Result<Channel, ImportError> {
    let mut channel = None;
    let parsed = wxr::parse(BufReader::new(open_file(path)?), |record| {
        if let Record::Channel(c) = record {
            channel = Some(c);
        }
        Err(ImportError::Stopped)
    });
    match (parsed, channel) {
        (_, Some(c)) => Ok(c),
        (Err(ImportError::Stopped) | Ok(_), None) => Err(ImportError::Missing {
            what: "channel",
            offset: 0,
        }),
        (Err(e), None) => Err(e),
    }
}

/// The run source (P3): the `<wp:base_blog_url>` with a lower-case scheme and host and
/// no trailing slash.
fn normalise_source(base_blog_url: &str) -> Result<String, ImportError> {
    let url = url::Url::parse(base_blog_url.trim()).map_err(|_| ImportError::Missing {
        what: "a valid wp:base_blog_url",
        offset: 0,
    })?;
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// The IR for `dir`: the folder of an unfinished run of the same file (`digest`) with a
/// finished parse, moved to `dir`, or a new file. Folders of the same file whose parse
/// did not finish are deleted; other folders are left alone. A dry run always gets a new
/// file and touches no other folder.
async fn prepare_ir(
    work_dir: &Path,
    dir: &Path,
    digest: &str,
    dry_run: bool,
) -> Result<Ir, ImportError> {
    let file_err = |path: &Path| {
        let path = path.to_owned();
        move |source| ImportError::File { path, source }
    };
    // A dry run neither takes nor clears another run's work file: a failed real run's
    // fix-up queue must survive it. Its own file is marked so no run ever takes it.
    let found = if dry_run {
        None
    } else {
        find_ir(work_dir, digest).await?
    };
    if let Some(found) = found {
        tokio::fs::rename(&found, dir)
            .await
            .map_err(file_err(&found))?;
        tracing::info!(from = %found.display(), "resuming the import from an earlier run's work file");
        return Ir::open(&dir.join(IR_FILE)).await;
    }
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(file_err(dir))?;
    let ir = Ir::create(&dir.join(IR_FILE)).await?;
    ir.set_meta(DIGEST_KEY, if dry_run { DRY_RUN } else { digest })
        .await?;
    Ok(ir)
}

async fn find_ir(work_dir: &Path, digest: &str) -> Result<Option<PathBuf>, ImportError> {
    let mut entries = match tokio::fs::read_dir(work_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ImportError::File {
                path: work_dir.to_owned(),
                source,
            });
        }
    };
    // The parsed folder with the most finished passes (a deterministic pick when
    // several failed runs left one).
    let mut found: Option<(u64, PathBuf)> = None;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|source| ImportError::File {
            path: work_dir.to_owned(),
            source,
        })?
    {
        let candidate = entry.path();
        let file = candidate.join(IR_FILE);
        if !tokio::fs::try_exists(&file).await.unwrap_or(false) {
            continue;
        }
        let ir = match Ir::open(&file).await {
            Ok(ir) => ir,
            Err(e) => {
                tracing::debug!(error = %e, file = %file.display(), "not an import work file");
                continue;
            }
        };
        let same = ir.meta(DIGEST_KEY).await.ok().flatten().as_deref() == Some(digest);
        let parsed = same && ir.pass_done(PARSE).await.unwrap_or(false);
        let passes = if parsed {
            ir.passes_done().await.unwrap_or(0)
        } else {
            0
        };
        ir.close().await;
        if parsed {
            if found.as_ref().is_none_or(|(most, _)| passes > *most) {
                found = Some((passes, candidate));
            }
        } else if same
            && !parsed
            && let Err(e) = tokio::fs::remove_dir_all(&candidate).await
        {
            tracing::warn!(error = %e, dir = %candidate.display(), "removing an unfinished import work folder failed");
        }
    }
    Ok(found.map(|(_, dir)| dir))
}

/// Streams the WXR file into `ir`: the parser runs on a blocking thread and sends
/// records through a bounded queue, so memory stays bounded by the largest item. The
/// run's heartbeat moves while it goes.
async fn parse_into(
    ir: &mut Ir,
    wxr: &Path,
    ctx: &ImportContext,
    handle: &ImportHandle,
) -> Result<(), ImportError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Record>(PARSE_QUEUE);
    let path = wxr.to_owned();
    let parser = tokio::task::spawn_blocking(move || {
        wxr::parse(BufReader::new(open_file(&path)?), |record| {
            tx.blocking_send(record).map_err(|_| ImportError::Stopped)
        })
    });
    let pushed = async {
        let mut beat = Instant::now();
        while let Some(record) = rx.recv().await {
            ir.push(&record).await?;
            if beat.elapsed() >= HEARTBEAT_EVERY {
                handle.heartbeat(&ctx.import).await?;
                beat = Instant::now();
            }
        }
        ir.flush().await
    }
    .await;
    // A failed writer stops the parser at its next record.
    drop(rx);
    let parsed = parser.await.map_err(|e| ImportError::Io {
        offset: 0,
        msg: format!("the parser stopped: {e}"),
    })?;
    pushed?;
    parsed.map(|_| ())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use ferrite_core::{PermalinkInput, PermalinkRules, Permalinks};
    use ferrite_domain::{
        ContentId, ContentItem, ContentStatus, NewUser, Role, Timestamp, UserId, UserStatus,
    };
    use ferrite_store_tests::TestDb;
    use futures::TryStreamExt;
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn sources_are_normalised() {
        assert_eq!(
            normalise_source("HTTP://WP.Example.TEST/").unwrap(),
            "http://wp.example.test"
        );
        assert_eq!(
            normalise_source(" https://example.test/blog/ ").unwrap(),
            "https://example.test/blog"
        );
        assert_eq!(
            normalise_source("https://example.test:443").unwrap(),
            "https://example.test"
        );
        assert!(normalise_source("").is_err());
    }

    /// Only an unfinished run's folder of the same file with a finished parse is found;
    /// one of the same file whose parse did not finish is deleted, others stay.
    #[tokio::test]
    async fn find_ir_takes_a_parsed_folder_of_the_same_file() {
        let work = tempfile::tempdir().unwrap();
        let make_with = |name: &'static str, digest: &'static str, passes: &'static [&str]| {
            let dir = work.path().join(name);
            async move {
                std::fs::create_dir_all(&dir).unwrap();
                let ir = Ir::create(&dir.join(IR_FILE)).await.unwrap();
                ir.set_meta(DIGEST_KEY, digest).await.unwrap();
                for pass in passes {
                    ir.mark_pass(pass).await.unwrap();
                }
                ir.close().await;
                dir
            }
        };
        let make = |name, digest, parsed: bool| {
            make_with(name, digest, if parsed { &[PARSE] } else { &[] })
        };
        let other = make("other", "bbb", true).await;
        let partial = make("partial", "aaa", false).await;
        assert_eq!(find_ir(work.path(), "aaa").await.unwrap(), None);
        assert!(!partial.exists());
        assert!(other.exists());
        // B6: a finished run's file (its folder left behind) is never resumed.
        let finished = make("finished", FINISHED, true).await;
        assert_eq!(find_ir(work.path(), "aaa").await.unwrap(), None);
        assert!(finished.exists());
        // A dry run's file is never taken, whatever it holds.
        let dry = make("dry", DRY_RUN, true).await;
        assert_eq!(find_ir(work.path(), "aaa").await.unwrap(), None);
        assert!(dry.exists());
        let parsed = make("parsed", "aaa", true).await;
        assert_eq!(find_ir(work.path(), "aaa").await.unwrap(), Some(parsed));
        // Of several, the one with the most finished passes, whatever the listing order.
        for name in ["a-early", "z-early"] {
            make_with(name, "aaa", &[PARSE, STEP_PASS1]).await;
        }
        let furthest = make_with(
            "m-furthest",
            "aaa",
            &[PARSE, STEP_PASS1, STEP_PASS2, STEP_PASS3],
        )
        .await;
        assert_eq!(find_ir(work.path(), "aaa").await.unwrap(), Some(furthest));
        assert_eq!(
            find_ir(&work.path().join("none"), "aaa").await.unwrap(),
            None
        );
    }

    /// A site on `db` for [`Session`] tests: a context whose service shares its rules,
    /// an IR file and the folder holding both.
    async fn session_site(db: &TestDb) -> (ImportContext, Ir, tempfile::TempDir) {
        let user = UserId::new();
        let tx = db.store().begin().await.unwrap();
        tx.users()
            .insert(&NewUser {
                id: user,
                login: format!("importer-{user}"),
                email: format!("{user}@example.test"),
                display_name: "Importer".into(),
                password_hash: None,
                role: Role::Admin,
                status: UserStatus::Active,
                must_reset: false,
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let permalinks = Arc::new(SwappablePermalinks::new(PermalinkRules::default()));
        let ctx = ImportContext {
            import: Arc::new(ImportService::new(db.shared_store(), permalinks.clone())),
            permalinks,
            media_store: Arc::new(
                MediaStore::open(ferrite_media::MediaConfig {
                    backend: ferrite_media::Backend::Local {
                        root: dir.path().join("media"),
                    },
                    prefix: String::new(),
                    public: ferrite_media::PublicUrlMode::ServeViaApp {
                        path_prefix: "/_media".into(),
                    },
                })
                .unwrap(),
            ),
            actor: Actor::new(user, Role::Admin),
            work_dir: dir.path().join("import"),
            media_source: Arc::new(DirSource::new(dir.path())),
            site_url: "http://localhost:8080".into(),
        };
        let ir = Ir::create(&dir.path().join(IR_FILE)).await.unwrap();
        (ctx, ir, dir)
    }

    /// A published post for permalink paths.
    fn post(slug: &str, published: &str) -> ContentItem {
        let at: Timestamp = serde_json::from_value(json!(published)).unwrap();
        ContentItem {
            id: ContentId::new(),
            kind: "post".into(),
            status: ContentStatus::Published,
            slug: slug.into(),
            parent_id: None,
            author_id: UserId::new(),
            title: "Hello".into(),
            excerpt: String::new(),
            body: json!({}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: Some(at),
            scheduled_at: None,
            created_at: at,
            updated_at: at,
            version: 1,
        }
    }

    fn rows(key: &str, value: &str) -> Vec<(String, Option<Value>)> {
        vec![(key.to_owned(), Some(json!(value)))]
    }

    async fn stored(db: &TestDb, key: &str) -> Option<Value> {
        db.store()
            .read()
            .options()
            .get(key)
            .await
            .unwrap()
            .map(|o| o.value)
    }

    /// Batches commit, or roll back on a dry run; `batch_items` and `fail_after_batches`
    /// bend the run for tests; the heartbeat works between batches.
    pub async fn session_batches_end_as_the_run_requires(db: &TestDb) {
        let (ctx, ir, _dir) = session_site(db).await;
        let handle = ctx
            .import
            .start(&ctx.actor, "http://wp.test", true, false, None)
            .await
            .unwrap();
        let opts = ImportOptions {
            dry_run: true,
            batch_items: Some(0),
            fail_after_batches: Some(2),
            ..ImportOptions::default()
        };
        let session = Session {
            ctx: &ctx,
            opts: &opts,
            handle: &handle,
            ir: &ir,
            ended: AtomicU32::new(0),
        };
        assert_eq!(session.batch_items(), 1);
        let mut batch = session.batch().await.unwrap();
        batch.options(&rows("site.title", "Dry")).await.unwrap();
        session.end(batch).await.unwrap();
        assert_eq!(stored(db, "site.title").await, None);
        session.heartbeat().await.unwrap();
        let err = session
            .end(session.schema_batch().await.unwrap())
            .await
            .unwrap_err();
        assert!(matches!(err, ImportError::Stopped), "{err:?}");

        let defaults = ImportOptions::default();
        let session = Session {
            opts: &defaults,
            ..session
        };
        assert_eq!(session.batch_items(), ImportBatch::BATCH_ITEMS);
        let mut batch = session.batch().await.unwrap();
        batch.options(&rows("site.title", "Wet")).await.unwrap();
        session.end(batch).await.unwrap();
        assert_eq!(stored(db, "site.title").await, Some(json!("Wet")));
        session.heartbeat().await.unwrap();
        handle
            .finish(&ctx.import, ImportRunState::Done, json!({}))
            .await
            .unwrap();
        ir.close().await;
    }

    /// `in_batch` ends the batch when the closure succeeds and rolls it back when it
    /// fails; the next batch then opens (nothing left holding the writer).
    pub async fn in_batch_rolls_back_on_error(db: &TestDb) {
        let (ctx, ir, _dir) = session_site(db).await;
        let handle = ctx
            .import
            .start(&ctx.actor, "http://wp.test", false, false, None)
            .await
            .unwrap();
        let opts = ImportOptions::default();
        let session = Session {
            ctx: &ctx,
            opts: &opts,
            handle: &handle,
            ir: &ir,
            ended: AtomicU32::new(0),
        };
        let err = session
            .in_batch(async |b| {
                b.options(&rows("site.title", "Lost")).await?;
                Err::<(), _>(ImportError::Stopped)
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ImportError::Stopped), "{err:?}");
        assert_eq!(stored(db, "site.title").await, None);
        assert_eq!(session.ended.load(Ordering::Relaxed), 0);
        // A batch error (an unknown option) is rolled back the same way.
        let err = session
            .in_batch(async |b| Ok(b.options(&rows("no.such.option", "x")).await?))
            .await
            .unwrap_err();
        assert!(matches!(err, ImportError::Core(_)), "{err:?}");
        session.heartbeat().await.unwrap();

        let n = session
            .in_schema_batch(async |b| {
                b.options(&rows("site.title", "Kept")).await?;
                Ok(7)
            })
            .await
            .unwrap();
        assert_eq!(n, 7);
        assert_eq!(stored(db, "site.title").await, Some(json!("Kept")));
        assert_eq!(session.ended.load(Ordering::Relaxed), 1);
        handle
            .finish(&ctx.import, ImportRunState::Done, json!({}))
            .await
            .unwrap();
        ir.close().await;
    }

    /// A schema batch that writes `permalink.post` swaps the run's rules once it ends,
    /// so the next batch pins the new ones: from the stored options, or on a dry run from
    /// the rolled-back rows.
    pub async fn permalink_rules_are_swapped_after_a_schema_batch(db: &TestDb) {
        const STRUCTURE: &str = "/%year%/%slug%/";
        let expected = PermalinkRules::from_options(Some(STRUCTURE), None).unwrap();
        assert_ne!(expected, PermalinkRules::default());
        for dry_run in [true, false] {
            let (ctx, ir, _dir) = session_site(db).await;
            let handle = ctx
                .import
                .start(&ctx.actor, "http://wp.test", dry_run, false, None)
                .await
                .unwrap();
            let opts = ImportOptions {
                dry_run,
                ..ImportOptions::default()
            };
            let session = Session {
                ctx: &ctx,
                opts: &opts,
                handle: &handle,
                ir: &ir,
                ended: AtomicU32::new(0),
            };
            // A content batch does not write permalink options, and swaps nothing.
            session.in_batch(async |_| Ok(())).await.unwrap();
            assert_eq!(*ctx.permalinks.current(), PermalinkRules::default());
            session
                .in_schema_batch(async |b| {
                    b.options(&rows("permalink.post", "/news/%slug%/")).await?;
                    b.options(&rows("permalink.post", STRUCTURE)).await?;
                    assert_eq!(b.permalink_changes().len(), 1);
                    Ok(())
                })
                .await
                .unwrap();
            assert_eq!(*ctx.permalinks.current(), expected, "dry run {dry_run}");
            // The next batch pins the swapped rules: the service shares the `Arc`.
            let input = PermalinkInput {
                item: post("hello", "2024-03-15T10:30:00Z"),
                ancestors: Vec::new(),
                primary_term: None,
            };
            assert_eq!(ctx.permalinks.pin().path(&input), "/2024/hello/");
            let want = if dry_run {
                None
            } else {
                Some(json!(STRUCTURE))
            };
            assert_eq!(stored(db, "permalink.post").await, want);
            handle
                .finish(&ctx.import, ImportRunState::Done, json!({}))
                .await
                .unwrap();
            ir.close().await;
        }
    }

    ferrite_store_tests::backend_tests!(
        session_batches_end_as_the_run_requires,
        in_batch_rolls_back_on_error,
        permalink_rules_are_swapped_after_a_schema_batch,
    );

    /// The driver's future can run on a multi-threaded runtime (the M3-2 job).
    #[test]
    fn run_is_send() {
        fn send<T: Send>(_: &T) {}
        fn check(ctx: ImportContext, opts: ImportOptions) {
            send(&run(ctx, Path::new("x.xml"), opts));
        }
        let _ = check;
    }

    #[tokio::test]
    async fn dir_source_reads_files_and_misses_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("2024/03")).unwrap();
        std::fs::write(dir.path().join("2024/03/a.txt"), b"hello").unwrap();
        std::fs::write(dir.path().join("outside.txt"), b"no").unwrap();
        let src = DirSource::new(dir.path().join("2024"));

        let stream = src.open("03/a.txt", "", 10).await.unwrap().unwrap();
        let bytes: Vec<Bytes> = stream.try_collect().await.unwrap();
        assert_eq!(bytes.concat(), b"hello");
        for missing in ["03/b.txt", "../outside.txt", "/etc/hosts", "", "03"] {
            assert!(
                src.open(missing, "", 10).await.unwrap().is_none(),
                "{missing}"
            );
        }
    }
}
