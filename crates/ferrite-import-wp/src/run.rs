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

use std::collections::BTreeMap;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use ferrite_core::import::{ImportBatch, ImportHandle, ImportService};
use ferrite_core::{Actor, SwappablePermalinks};
use ferrite_domain::ImportRunState;
use ferrite_media::MediaStore;
use futures::Stream;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ImportError;
use crate::ir::Ir;
use crate::model::{Channel, Record};
use crate::wxr;

/// The IR file in a run's folder.
const IR_FILE: &str = "ir.sqlite";
/// The IR `meta` key holding the hex SHA-256 of the WXR file.
const DIGEST_KEY: &str = "wxr_sha256";
/// The step that fills the IR.
const PARSE: &str = "parse";
/// Records in flight between the parser thread and the IR writer.
const PARSE_QUEUE: usize = 256;
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

/// How to import.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// A local copy of `wp-content/uploads`.
    pub uploads: Option<PathBuf>,
    /// Download attachments missing from `uploads` from the old site.
    pub fetch_media: bool,
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
    /// Items per batch instead of [`ImportBatch::BATCH_ITEMS`].
    #[cfg(feature = "test-seams")]
    pub batch_items: Option<usize>,
    /// Fail with [`ImportError::Stopped`] once this many batches have ended.
    #[cfg(feature = "test-seams")]
    pub fail_after_batches: Option<u32>,
    /// Called before each step (`parse`, `pass1`…`pass4`, `fixup`) that runs.
    #[cfg(feature = "test-seams")]
    pub step_hook: Option<StepHook>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            uploads: None,
            fetch_media: false,
            dry_run: false,
            keep_permalinks: true,
            site_options: true,
            category_base: "category".into(),
            tag_base: "tag".into(),
            takeover: false,
            #[cfg(feature = "test-seams")]
            batch_items: None,
            #[cfg(feature = "test-seams")]
            fail_after_batches: None,
            #[cfg(feature = "test-seams")]
            step_hook: None,
        }
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

/// What a run did (Task 16 adds the rest).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// Per kind (`users`, `terms`, `media`, a post type, `comments`, …).
    pub counts: BTreeMap<String, Counts>,
}

/// The outcomes of one kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub created: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub kept: u64,
    pub skipped: u64,
    pub failed: u64,
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

    async fn run_batch<T>(
        &self,
        mut batch: ImportBatch<'a>,
        f: impl AsyncFnOnce(&mut ImportBatch<'a>) -> Result<T, ImportError>,
    ) -> Result<T, ImportError> {
        match f(&mut batch).await {
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
/// unreadable file, a busy run) leave no run; later ones finish it `failed`.
pub async fn run(
    ctx: ImportContext,
    wxr: &Path,
    opts: ImportOptions,
) -> Result<Report, ImportError> {
    let (digest, channel) = inspect(wxr).await?;
    let source = normalise_source(&channel.base_blog_url)?;
    let handle = ctx
        .import
        .start(&ctx.actor, &source, opts.dry_run, opts.takeover)
        .await?;
    let dir = ctx.work_dir.join(handle.run.id.to_string());
    let outcome = drive(&ctx, &opts, &handle, wxr, &digest, &dir).await;
    match outcome {
        Ok(report) => {
            let stats = serde_json::to_value(&report.counts)?;
            handle
                .finish(&ctx.import, ImportRunState::Done, stats)
                .await?;
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                tracing::warn!(error = %e, dir = %dir.display(), "removing the import's work folder failed");
            }
            Ok(report)
        }
        Err(e) => {
            let stats = serde_json::json!({ "error": e.to_string() });
            if let Err(f) = handle
                .finish(&ctx.import, ImportRunState::Failed, stats)
                .await
            {
                tracing::warn!(error = %f, "marking the import run failed did not work");
            }
            Err(e)
        }
    }
}

/// Opens the IR (reused or new), runs the steps and closes it.
async fn drive(
    ctx: &ImportContext,
    opts: &ImportOptions,
    handle: &ImportHandle,
    wxr: &Path,
    digest: &str,
    dir: &Path,
) -> Result<Report, ImportError> {
    let mut ir = prepare_ir(&ctx.work_dir, dir, digest).await?;
    let done = steps(ctx, opts, handle, wxr, &mut ir).await;
    ir.close().await;
    done
}

async fn steps(
    ctx: &ImportContext,
    opts: &ImportOptions,
    handle: &ImportHandle,
    wxr: &Path,
    ir: &mut Ir,
) -> Result<Report, ImportError> {
    if begin_step(opts, ir, PARSE).await? {
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
    let mut report = Report::default();
    if begin_step(opts, ir, "pass1").await? {
        pass1(&session, &mut report).await?;
        end_step(opts, ir, "pass1").await?;
    }
    if begin_step(opts, ir, "pass2").await? {
        pass2(&session, &mut report).await?;
        end_step(opts, ir, "pass2").await?;
    }
    if begin_step(opts, ir, "pass3").await? {
        pass3(&session, &mut report).await?;
        end_step(opts, ir, "pass3").await?;
    }
    if begin_step(opts, ir, "pass4").await? {
        pass4(&session, &mut report).await?;
        end_step(opts, ir, "pass4").await?;
    }
    if begin_step(opts, ir, "fixup").await? {
        fixup(&session, &mut report).await?;
        end_step(opts, ir, "fixup").await?;
    }
    Ok(report)
}

/// Whether step `name` still has to run (it did not finish on this IR); calls the test
/// hook before one that does.
async fn begin_step(opts: &ImportOptions, ir: &Ir, name: &str) -> Result<bool, ImportError> {
    if ir.pass_done(name).await? {
        tracing::debug!(step = name, "import step already done; skipped");
        return Ok(false);
    }
    #[cfg(feature = "test-seams")]
    if let Some(hook) = &opts.step_hook {
        (hook.0)(name)?;
    }
    #[cfg(not(feature = "test-seams"))]
    let _ = opts;
    Ok(true)
}

/// Records a finished pass. A dry run rolled its writes back, so it records none.
async fn end_step(opts: &ImportOptions, ir: &Ir, name: &str) -> Result<(), ImportError> {
    if !opts.dry_run {
        ir.mark_pass(name).await?;
    }
    Ok(())
}

/// Pass 1: authors, taxonomies, terms, types (Task 13).
async fn pass1(_session: &Session<'_>, _report: &mut Report) -> Result<(), ImportError> {
    Ok(())
}

/// Pass 2: attachments (Task 13).
async fn pass2(_session: &Session<'_>, _report: &mut Report) -> Result<(), ImportError> {
    Ok(())
}

/// Pass 3: content (Task 14).
async fn pass3(_session: &Session<'_>, _report: &mut Report) -> Result<(), ImportError> {
    Ok(())
}

/// Pass 4: comments, menus, redirects, options (Task 15).
async fn pass4(_session: &Session<'_>, _report: &mut Report) -> Result<(), ImportError> {
    Ok(())
}

/// The fix-up: links to items created after the item linking to them (Task 14).
async fn fixup(_session: &Session<'_>, _report: &mut Report) -> Result<(), ImportError> {
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
/// did not finish are deleted; other folders are left alone.
async fn prepare_ir(work_dir: &Path, dir: &Path, digest: &str) -> Result<Ir, ImportError> {
    let file_err = |path: &Path| {
        let path = path.to_owned();
        move |source| ImportError::File { path, source }
    };
    if let Some(found) = find_ir(work_dir, digest).await? {
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
    ir.set_meta(DIGEST_KEY, digest).await?;
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
    let mut found = None;
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
        ir.close().await;
        if parsed && found.is_none() {
            found = Some(candidate);
        } else if same
            && !parsed
            && let Err(e) = tokio::fs::remove_dir_all(&candidate).await
        {
            tracing::warn!(error = %e, dir = %candidate.display(), "removing an unfinished import work folder failed");
        }
    }
    Ok(found)
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
        let make = |name: &'static str, digest: &'static str, parsed: bool| {
            let dir = work.path().join(name);
            async move {
                std::fs::create_dir_all(&dir).unwrap();
                let ir = Ir::create(&dir.join(IR_FILE)).await.unwrap();
                ir.set_meta(DIGEST_KEY, digest).await.unwrap();
                if parsed {
                    ir.mark_pass(PARSE).await.unwrap();
                }
                ir.close().await;
                dir
            }
        };
        let other = make("other", "bbb", true).await;
        let partial = make("partial", "aaa", false).await;
        assert_eq!(find_ir(work.path(), "aaa").await.unwrap(), None);
        assert!(!partial.exists());
        assert!(other.exists());
        let parsed = make("parsed", "aaa", true).await;
        assert_eq!(find_ir(work.path(), "aaa").await.unwrap(), Some(parsed));
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
            .start(&ctx.actor, "http://wp.test", true, false)
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
            .start(&ctx.actor, "http://wp.test", false, false)
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
                .start(&ctx.actor, "http://wp.test", dry_run, false)
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
