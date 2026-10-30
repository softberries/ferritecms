//! The import run (P62): a scanned folder written through an [`ImportService`] run.
//!
//! [`run`] scans the folder (on a blocking thread), converts every page once to find
//! its images and to report what it cannot import, then writes in three passes:
//!
//! 1. **media:** every image a page names inside the folder becomes a published media
//!    item (map kind `media`, key [`map_id`] of the image's path), staged as an upload is
//!    (type from the content, SVG sanitised, opened without following symlinks). A
//!    mapped image is not read again: one whose bytes changed keeps its file and is
//!    reported ([`Problem::ImageChanged`]), since core does not replace an item's file;
//! 2. **content:** every page, parents first, through [`ImportBatch::content`]. A folder's
//!    page is keyed by its folder (`<folder>/`), index file or synthesized, so a new or
//!    removed index updates it in place (ruling R-b9); other pages by their file path.
//!    The map hash covers the file's bytes, [`FORMAT`] and the parent page's key, so a
//!    file moved under another parent is an update (R-b6). Relative links are kept as
//!    written;
//!    `--front-page` then sets `reading.front_page` under the import's option rule;
//! 3. **links:** every page written or unchanged (not one kept as edited here) is
//!    converted again with its links rewritten to the target pages' paths, and stored
//!    through [`ImportBatch::set_body_and_fields`] only when that body differs from the
//!    stored one. That write moves the map row's version with the item's, so the next
//!    run still sees the page as the import left it (P4).
//!
//! Each pass writes in committed batches of at most [`ImportBatch::BATCH_ITEMS`] items.
//! A dry run keeps one batch open for the whole run and rolls it back at the end, so
//! later passes see what earlier ones wrote; its images are staged into a throwaway
//! store under `MdOptions::work_dir`, deleted afterwards.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ferrite_core::content::SeoPatch;
use ferrite_core::import::{
    ImportBatch, ImportContent, ImportHandle, ImportMedia, ImportService, MD_ID_META, Written,
};
use ferrite_core::{NewUpload, check_file_name};
use ferrite_domain::{
    Artifact, ArtifactKind, ContentId, ContentStatus, ImportMapKind, Timestamp, UserId,
};
use ferrite_import_wp::convert::{ImageRef, Notes, Rewrite};
use ferrite_media::ingest::stage_stream;
use ferrite_media::{Backend, MediaConfig, MediaError, MediaStore, PublicUrlMode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::convert::{Converted, convert};
use crate::error::MdError;
use crate::scan::{FORMAT, MdDoc, Miss, Problem, hex, locate, map_id, open_nofollow, scan};

/// The option `--front-page` sets.
const FRONT_PAGE_OPTION: &str = "reading.front_page";
/// The map id (kind `option`) of the front page the import set.
const FRONT_PAGE_MAP_ID: i64 = 1;
/// How often a long run of image reads proves the run is alive (stale after 120 s).
const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);
/// The dry run's throwaway media store, in its run folder.
const DRY_MEDIA: &str = "dry-run-media";
/// Map rows read per page when listing pages no longer in the folder.
const MAP_PAGE: u32 = 200;

/// How to import.
#[derive(Debug, Clone)]
pub struct MdOptions {
    /// The content type of the pages (`--type`, default `page`).
    pub kind: String,
    /// Set the root `index.md` (or `README.md`) as `reading.front_page`.
    pub front_page: bool,
    /// Run everything, then roll every write back.
    pub dry_run: bool,
    /// The largest image accepted (`media.max_upload`).
    pub max_upload: u64,
    /// Strip EXIF and XMP from imported JPEG, PNG and WebP files (`media.strip_metadata`).
    pub strip_metadata: bool,
    /// Where a dry run stages its images (`<work_dir>/<run-id>/dry-run-media`, deleted
    /// when the run ends): `<data>/import`.
    pub work_dir: PathBuf,
}

/// What a run did (P62): counts per outcome, then every problem with its file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MdReport {
    pub created: u32,
    pub updated: u32,
    pub unchanged: u32,
    /// Changed in the folder but edited here since the import wrote it: kept.
    pub kept_edited: u32,
    /// Images imported as new media items.
    pub media: u32,
    pub problems: Vec<Problem>,
    /// The file set (a dry run: that would be set) as the front page.
    pub front_page: Option<String>,
}

impl MdReport {
    /// Whether the run exits 2: a file was skipped, an image is missing or refused, or a
    /// link is broken (any problem but a notice, [`Problem::is_notice`]).
    pub fn has_problems(&self) -> bool {
        self.problems.iter().any(|p| !p.is_notice())
    }

    /// The run's counters, for `ImportHandle::finish`.
    pub fn stats(&self) -> Value {
        json!({
            "created": self.created,
            "updated": self.updated,
            "unchanged": self.unchanged,
            "kept_edited": self.kept_edited,
            "media": self.media,
            "problems": self.problems.len(),
        })
    }
}

/// One page of the run.
struct Doc {
    md: MdDoc,
    /// The `import_map` key: [`map_id`] of [`page_key`].
    wp_id: i64,
    /// The map hash: the file's hash ([`FORMAT`] included) and the parent's path.
    hash: String,
    id: Option<ContentId>,
    /// What pass 2 did; `None` when it skipped the page.
    written: Option<Written>,
}

/// The pages by file path and by the folder they stand for.
#[derive(Default)]
struct Index {
    by_rel: HashMap<String, usize>,
    by_folder: HashMap<String, usize>,
}

impl Index {
    /// The page a target path names: the file itself, or the page of a folder.
    fn page(&self, rel: &str) -> Option<usize> {
        self.by_rel
            .get(rel)
            .or_else(|| self.by_folder.get(rel))
            .copied()
    }
}

/// What every pass reads: the canonical folder, its pages and the run's facts.
struct Cx<'a> {
    root: &'a Path,
    index: &'a Index,
    /// The importing admin: author of every item.
    actor: UserId,
    /// When the run started: the dates of items whose file gives none.
    start: Timestamp,
    opts: &'a MdOptions,
}

/// Imports the folder `dir` under the started run `handle` (see the module docs).
/// Writes made before an error stay (each batch commits), except in a dry run. The caller
/// finishes the run.
pub async fn run(
    svc: &ImportService,
    handle: &ImportHandle,
    media: &MediaStore,
    dir: &Path,
    opts: &MdOptions,
) -> Result<MdReport, MdError> {
    let owned = dir.to_owned();
    let scanned = tokio::task::spawn_blocking(move || scan(&owned))
        .await
        .map_err(|e| MdError::Stopped(e.to_string()))??;
    let root = dir.canonicalize().map_err(|source| MdError::Io {
        path: dir.to_owned(),
        source,
    })?;
    let mut report = MdReport {
        problems: scanned.problems,
        ..MdReport::default()
    };
    // Each page's key by its file path, for its children's hashes.
    let keys: HashMap<&str, String> = scanned
        .docs
        .iter()
        .map(|md| (md.rel.as_str(), page_key(md)))
        .collect();
    let hashes: Vec<String> = scanned
        .docs
        .iter()
        .map(|md| {
            let parent = md.parent.as_deref().and_then(|p| keys.get(p));
            map_hash(md, parent.map_or("", String::as_str))
        })
        .collect();
    drop(keys);
    let mut docs: Vec<Doc> = scanned
        .docs
        .into_iter()
        .zip(hashes)
        .map(|(md, hash)| Doc {
            wp_id: map_id(&page_key(&md)),
            hash,
            md,
            id: None,
            written: None,
        })
        .collect();
    let mut index = Index::default();
    for (i, d) in docs.iter().enumerate() {
        index.by_rel.insert(d.md.rel.clone(), i);
        if let Some(folder) = &d.md.folder {
            index.by_folder.insert(folder.clone(), i);
        }
    }
    // Every page once: its images, and what it cannot import.
    let mut images: BTreeMap<String, PathBuf> = BTreeMap::new();
    let (no_media, no_paths) = (HashMap::new(), HashMap::new());
    for d in &docs {
        let Some(source) = &d.md.source else {
            continue;
        };
        let rw = Rw::new(&root, &d.md.rel, &index, &no_media, &no_paths);
        convert(source, &rw, &mut Notes::default());
        let found = rw.found.into_inner();
        images.extend(found.images);
        for p in found.problems {
            if !report.problems.contains(&p) {
                report.problems.push(p);
            }
        }
    }

    let mut w = Writer {
        svc,
        handle,
        dry: None,
        dry_run: opts.dry_run,
    };
    let cx = Cx {
        root: &root,
        index: &index,
        actor: handle.run().started_by,
        start: handle.run().started_at,
        opts,
    };
    let done = async {
        let media_ids = if opts.dry_run {
            let store_dir = opts
                .work_dir
                .join(handle.run().id.to_string())
                .join(DRY_MEDIA);
            let staging = MediaStore::open(MediaConfig {
                backend: Backend::Local {
                    root: store_dir.clone(),
                },
                prefix: String::new(),
                public: PublicUrlMode::ServeViaApp {
                    path_prefix: "/_media".into(),
                },
            })?;
            let done = media_pass(&mut w, &cx, &staging, &images, &mut report).await;
            drop(staging);
            remove_dir(store_dir.parent().unwrap_or(&store_dir)).await;
            done?
        } else {
            media_pass(&mut w, &cx, media, &images, &mut report).await?
        };
        content_pass(&mut w, &cx, &mut docs, &media_ids, &mut report).await?;
        if opts.front_page {
            front_page(&mut w, &cx, &docs, &mut report).await?;
        }
        let paths = paths(&mut w, &docs).await?;
        link_pass(&mut w, &cx, &docs, &media_ids, &paths).await?;
        not_in_source(&mut w, &docs, &mut report).await
    }
    .await;
    let ended = w.end().await;
    done?;
    ended?;
    Ok(report)
}

/// The string a page's `import_map` key is [`map_id`] of (ruling R-b9): a folder's page
/// is identified by its folder, `<folder>/` (`/` for the root), whether an `index.md` or
/// `README.md` fills it or it is synthesized, so adding or removing the index updates
/// that page in place; any other page by its file path.
fn page_key(md: &MdDoc) -> String {
    match &md.folder {
        Some(folder) => format!("{folder}/"),
        None => md.rel.clone(),
    }
}

/// The map hash of a page (rulings R-b6, R-b9): its file hash (bytes and [`FORMAT`]; for
/// a synthesized page [`FORMAT`] and the folder) and its parent page's key, so a page
/// whose parent page changed is written again, while a folder page that a new index
/// fills stays the same parent.
fn map_hash(md: &MdDoc, parent_key: &str) -> String {
    let mut h = Sha256::new();
    h.update(md.hash.as_bytes());
    h.update(b"\nparent:");
    h.update(parent_key.as_bytes());
    hex(&h.finalize())
}

/// Opens and ends batches: committed one by one, or (a dry run) one batch for the whole
/// run, rolled back by [`end`](Self::end).
struct Writer<'a> {
    svc: &'a ImportService,
    handle: &'a ImportHandle,
    dry: Option<ImportBatch<'a>>,
    dry_run: bool,
}

impl<'a> Writer<'a> {
    async fn open(&mut self) -> Result<ImportBatch<'a>, MdError> {
        match self.dry.take() {
            Some(batch) => Ok(batch),
            None => Ok(self.handle.batch(self.svc).await?),
        }
    }

    /// Commits `batch` (a dry run keeps it open) when `result` is `Ok`, else rolls it
    /// back (the batch error contract) and hands back the error.
    async fn close<T>(
        &mut self,
        batch: ImportBatch<'a>,
        result: Result<T, MdError>,
    ) -> Result<T, MdError> {
        match result {
            Ok(value) => {
                if self.dry_run {
                    self.dry = Some(batch);
                } else {
                    batch.commit().await?;
                }
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

    /// Proves the run is alive between batches; a dry run, whose batch stays open,
    /// fenced when it opened it.
    async fn heartbeat(&self) -> Result<(), MdError> {
        if self.dry_run {
            return Ok(());
        }
        Ok(self.handle.heartbeat(self.svc).await?)
    }

    /// Rolls back a dry run's batch.
    async fn end(self) -> Result<(), MdError> {
        if let Some(batch) = self.dry {
            batch.rollback().await?;
        }
        Ok(())
    }
}

async fn remove_dir(dir: &Path) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "removing the dry run's media failed")
        }
    }
}

/// What converting a page found: the images it names inside the folder (path to the
/// canonical file) and the problems.
#[derive(Default)]
struct Found {
    images: BTreeMap<String, PathBuf>,
    problems: Vec<Problem>,
}

/// The [`Rewrite`] of one page: images to media items, links to page paths (when
/// `paths` has them; else kept as written).
struct Rw<'a> {
    root: &'a Path,
    from: &'a str,
    index: &'a Index,
    media: &'a HashMap<String, ContentId>,
    paths: &'a HashMap<usize, String>,
    found: RefCell<Found>,
}

impl<'a> Rw<'a> {
    fn new(
        root: &'a Path,
        from: &'a str,
        index: &'a Index,
        media: &'a HashMap<String, ContentId>,
        paths: &'a HashMap<usize, String>,
    ) -> Self {
        Self {
            root,
            from,
            index,
            media,
            paths,
            found: RefCell::new(Found::default()),
        }
    }

    fn problem(&self, p: Problem) {
        let mut found = self.found.borrow_mut();
        if !found.problems.contains(&p) {
            found.problems.push(p);
        }
    }
}

/// `src` shortened for a report line (a `data:` URI can be long).
fn shown(src: &str) -> String {
    const MAX: usize = 120;
    match src.char_indices().nth(MAX) {
        Some((at, _)) => format!("{}…", &src[..at]),
        None => src.to_owned(),
    }
}

/// Whether a link's path part names a Markdown file or a folder (`…/`).
fn is_page_link(href: &str) -> bool {
    let path = href.split(['?', '#']).next().unwrap_or("");
    path.ends_with('/') || path.to_ascii_lowercase().ends_with(".md")
}

impl Rewrite for Rw<'_> {
    fn image(&self, src: &str) -> ImageRef {
        let lower = src.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") || src.starts_with("//") {
            return ImageRef::Url(src.to_owned());
        }
        let file = self.from.to_owned();
        match locate(self.root, self.from, src) {
            Ok(t) => {
                if let Some(id) = self.media.get(&t.rel) {
                    return ImageRef::Media(id.as_uuid());
                }
                self.found.borrow_mut().images.insert(t.rel, t.path);
            }
            Err(Miss::Missing) => self.problem(Problem::MissingImage {
                file,
                src: shown(src),
            }),
            // A rooted or absolute path, another scheme, or a path leaving the folder:
            // never read.
            Err(Miss::NotRelative | Miss::Outside) => self.problem(Problem::OutsideDir {
                file,
                target: shown(src),
            }),
        }
        // Not imported: a relative source is left out of the body (counted by the
        // converter, not reported again), a rooted one kept as written.
        ImageRef::Missing(src.to_owned())
    }

    fn attachment(&self, _wp_id: i64) -> Option<Uuid> {
        None
    }

    fn href(&self, href: &str) -> String {
        let broken = || {
            self.problem(Problem::BrokenLink {
                file: self.from.to_owned(),
                href: shown(href),
            })
        };
        match locate(self.root, self.from, href) {
            Ok(t) => match self.index.page(&t.rel) {
                Some(i) => match self.paths.get(&i) {
                    Some(path) => match t.fragment {
                        Some(f) => format!("{path}#{f}"),
                        None => path.clone(),
                    },
                    None => href.to_owned(),
                },
                None => {
                    if t.path.is_dir() || t.rel.to_ascii_lowercase().ends_with(".md") {
                        broken();
                    }
                    href.to_owned()
                }
            },
            Err(Miss::NotRelative) => href.to_owned(),
            Err(Miss::Missing | Miss::Outside) => {
                if is_page_link(href) {
                    broken();
                }
                href.to_owned()
            }
        }
    }
}

/// Pass 1: every image found becomes a media item (see the module docs); the media
/// item of each image path that has one.
async fn media_pass(
    w: &mut Writer<'_>,
    cx: &Cx<'_>,
    staging: &MediaStore,
    images: &BTreeMap<String, PathBuf>,
    report: &mut MdReport,
) -> Result<HashMap<String, ContentId>, MdError> {
    let opts = cx.opts;
    let mut ids = HashMap::new();
    let all: Vec<(&String, &PathBuf)> = images.iter().collect();
    for chunk in all.chunks(ImportBatch::BATCH_ITEMS) {
        // The hash of each file, read without following a symlink.
        let mut hashed = Vec::with_capacity(chunk.len());
        let mut beat = Instant::now();
        for (rel, path) in chunk {
            match hash_file(path, opts.max_upload).await? {
                Ok(hash) => hashed.push((*rel, *path, hash)),
                Err(p) => report.problems.push(p.with_file(rel)),
            }
            if beat.elapsed() >= HEARTBEAT_EVERY {
                w.heartbeat().await?;
                beat = Instant::now();
            }
        }
        // Mapped images are not read again.
        let batch = w.open().await?;
        let checked = check_mapped(&batch, &hashed).await;
        let mapped = w.close(batch, checked).await?;
        let mut todo = Vec::new();
        for ((rel, path, hash), m) in hashed.into_iter().zip(mapped) {
            match m {
                Some((id, same)) => {
                    ids.insert(rel.clone(), id);
                    if !same {
                        report
                            .problems
                            .push(Problem::ImageChanged { file: rel.clone() });
                    }
                }
                None => todo.push((rel, path, hash)),
            }
        }
        let mut staged = Vec::with_capacity(todo.len());
        for (rel, path, hash) in todo {
            match stage(staging, rel, path, opts).await? {
                Ok(upload) => staged.push((rel, hash, upload)),
                Err(p) => report.problems.push(p),
            }
            if beat.elapsed() >= HEARTBEAT_EVERY {
                w.heartbeat().await?;
                beat = Instant::now();
            }
        }
        if staged.is_empty() {
            continue;
        }
        let mut batch = w.open().await?;
        let written = write_media(&mut batch, &staged, cx.actor, cx.start).await;
        let written = w.close(batch, written).await?;
        for (rel, id, result) in written {
            if matches!(result, Written::Created) {
                report.media += 1;
            }
            ids.insert(rel.clone(), id);
        }
    }
    Ok(ids)
}

/// A problem about an image before its path is known.
enum ImageProblem {
    Refused(String),
    Unreadable(String),
}

impl ImageProblem {
    fn with_file(self, rel: &str) -> Problem {
        let file = rel.to_owned();
        match self {
            Self::Refused(message) => Problem::RefusedImage { file, message },
            Self::Unreadable(message) => Problem::Unreadable { file, message },
        }
    }
}

/// The SHA-256 (hex) of the file at `path` and [`FORMAT`], read on a blocking thread
/// without following a symlink; a file over `max` bytes is refused unread.
async fn hash_file(path: &Path, max: u64) -> Result<Result<String, ImageProblem>, MdError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut file = match open_nofollow(&path) {
            Ok(file) => file,
            Err(e) => return Ok(Err(ImageProblem::Unreadable(e.to_string()))),
        };
        match file.metadata() {
            Ok(m) if m.len() > max => {
                return Ok(Err(ImageProblem::Refused(format!(
                    "larger than {max} bytes"
                ))));
            }
            Ok(_) => {}
            Err(e) => return Ok(Err(ImageProblem::Unreadable(e.to_string()))),
        }
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => h.update(&buf[..n]),
                Err(e) => return Ok(Err(ImageProblem::Unreadable(e.to_string()))),
            }
        }
        h.update(FORMAT.as_bytes());
        Ok(Ok(hex(&h.finalize())))
    })
    .await
    .map_err(|e| MdError::Stopped(e.to_string()))?
}

/// For each hashed image: its media item when mapped (and still there), and whether the
/// mapped hash is the same.
async fn check_mapped(
    b: &ImportBatch<'_>,
    hashed: &[(&String, &PathBuf, String)],
) -> Result<Vec<Option<(ContentId, bool)>>, MdError> {
    let mut out = Vec::with_capacity(hashed.len());
    for (rel, _, hash) in hashed {
        let mapped = b.lookup(ImportMapKind::Media, map_id(rel)).await?;
        let found = match mapped {
            Some(m) => {
                let id = ContentId::from_uuid(m.id);
                b.media_row(id).await?.map(|_| (id, m.hash == *hash))
            }
            None => None,
        };
        out.push(found);
    }
    Ok(out)
}

/// Stages one image as an upload, or the problem that refused it.
async fn stage(
    staging: &MediaStore,
    rel: &str,
    path: &Path,
    opts: &MdOptions,
) -> Result<Result<NewUpload, Problem>, MdError> {
    let refused = |message: String| {
        Ok(Err(Problem::RefusedImage {
            file: rel.to_owned(),
            message,
        }))
    };
    let unreadable = |message: String| {
        Ok(Err(Problem::Unreadable {
            file: rel.to_owned(),
            message,
        }))
    };
    let file_name = match check_file_name(rel) {
        Ok(name) => name,
        Err(e) => return refused(e.to_string()),
    };
    let file = match open_nofollow(path) {
        Ok(file) => tokio::fs::File::from_std(file),
        Err(e) => return unreadable(e.to_string()),
    };
    let stream = tokio_util::io::ReaderStream::new(file);
    let staged = match stage_stream(
        staging,
        &file_name,
        stream,
        opts.max_upload,
        opts.strip_metadata,
    )
    .await
    {
        Ok(staged) => staged,
        Err(
            e @ (MediaError::TooLarge { .. }
            | MediaError::Unsupported { .. }
            | MediaError::Svg(_)
            | MediaError::Metadata(_)),
        ) => return refused(e.to_string()),
        Err(MediaError::Io(why)) => return unreadable(why.to_string()),
        Err(e) => return Err(e.into()),
    };
    Ok(Ok(NewUpload {
        artifact: Artifact {
            hash: staged.artifact.hash,
            kind: ArtifactKind::Media,
            bytes: i64::try_from(staged.bytes).unwrap_or(i64::MAX),
            mime: staged.mime.clone(),
            created_at: Timestamp::now(),
            orphaned_at: None,
        },
        mime: staged.mime,
        bytes: staged.bytes,
        original_name: file_name,
        title: None,
        alt: String::new(),
        sanitized: staged.sanitized,
        raster: staged.raster,
    }))
}

async fn write_media<'r>(
    b: &mut ImportBatch<'_>,
    staged: &[(&'r String, String, NewUpload)],
    actor: UserId,
    start: Timestamp,
) -> Result<Vec<(&'r String, ContentId, Written)>, MdError> {
    let mut out = Vec::with_capacity(staged.len());
    for (rel, hash, upload) in staged {
        let outcome = b
            .media(ImportMedia {
                wp_id: map_id(rel),
                id_meta: MD_ID_META,
                hash: hash.clone(),
                upload: upload.clone(),
                author: actor,
                created_at: start,
                parent: None,
            })
            .await?;
        out.push((*rel, outcome.id, outcome.result));
    }
    Ok(out)
}

/// The page's body with images rewritten (and links, when `paths` has them).
fn body_of(
    d: &Doc,
    cx: &Cx<'_>,
    media: &HashMap<String, ContentId>,
    paths: &HashMap<usize, String>,
) -> (Value, Option<String>) {
    match &d.md.source {
        Some(source) => {
            let rw = Rw::new(cx.root, &d.md.rel, cx.index, media, paths);
            let Converted { title, body, .. } = convert(source, &rw, &mut Notes::default());
            (body, title)
        }
        None => (json!({"type": "doc", "content": []}), None),
    }
}

/// Pass 2: every page, parents first.
async fn content_pass(
    w: &mut Writer<'_>,
    cx: &Cx<'_>,
    docs: &mut [Doc],
    media: &HashMap<String, ContentId>,
    report: &mut MdReport,
) -> Result<(), MdError> {
    let mut at = 0;
    while at < docs.len() {
        let end = (at + ImportBatch::BATCH_ITEMS).min(docs.len());
        let mut batch = w.open().await?;
        let written = write_pages(&mut batch, cx, docs, at..end, media).await;
        for p in w.close(batch, written).await? {
            report.problems.push(p);
        }
        at = end;
    }
    for d in docs.iter() {
        match d.written {
            Some(Written::Created) => report.created += 1,
            Some(Written::Updated) => report.updated += 1,
            Some(Written::Unchanged) => report.unchanged += 1,
            Some(Written::KeptEdited) => report.kept_edited += 1,
            None => {}
        }
    }
    Ok(())
}

/// Writes `docs[range]` in batch `b` (links kept as written); the problems met
/// (changed slugs, other types).
async fn write_pages(
    b: &mut ImportBatch<'_>,
    cx: &Cx<'_>,
    docs: &mut [Doc],
    range: std::ops::Range<usize>,
    media: &HashMap<String, ContentId>,
) -> Result<Vec<Problem>, MdError> {
    let (index, start, opts) = (cx.index, cx.start, cx.opts);
    let no_paths = HashMap::new();
    let mut problems = Vec::new();
    for i in range {
        let parent = docs[i]
            .md
            .parent
            .as_deref()
            .and_then(|p| index.by_rel.get(p))
            .and_then(|&p| docs[p].id);
        let d = &docs[i];
        // The dates an earlier run gave the page stay when the file has none.
        let mapped = match b.lookup(ImportMapKind::Content, d.wp_id).await? {
            Some(m) => b.get_item(ContentId::from_uuid(m.id)).await?,
            None => None,
        };
        if let Some(item) = &mapped
            && item.kind != opts.kind
        {
            problems.push(Problem::OtherType {
                file: d.md.rel.clone(),
                kind: item.kind.clone(),
            });
            docs[i].id = Some(item.id);
            continue;
        }
        let (body, h1) = body_of(d, cx, media, &no_paths);
        let front = &d.md.front;
        let title = front
            .title
            .clone()
            .or(h1)
            .unwrap_or_else(|| d.md.fallback_title.clone());
        let date = front.date.map(jiff_to_timestamp).transpose()?;
        let created_at = date
            .or_else(|| mapped.as_ref().map(|m| m.created_at))
            .unwrap_or(start);
        let (status, published_at) = if front.draft {
            (ContentStatus::Draft, None)
        } else {
            let published = date
                .or_else(|| mapped.as_ref().and_then(|m| m.published_at))
                .unwrap_or(start);
            (ContentStatus::Published, Some(published))
        };
        let description = front
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let outcome = b
            .content(ImportContent {
                wp_id: d.wp_id,
                id_meta: MD_ID_META,
                hash: d.hash.clone(),
                kind: opts.kind.clone(),
                title,
                slug: Some(d.md.slug.clone()),
                parent,
                author: cx.actor,
                status,
                published_at,
                created_at,
                updated_at: start,
                excerpt: description.map(str::to_owned),
                body,
                fields: json!({}),
                menu_order: d.md.order,
                terms: Vec::new(),
                seo: description.map(|s| SeoPatch {
                    description: Some(Some(s.to_owned())),
                    ..SeoPatch::default()
                }),
                meta: Vec::new(),
            })
            .await?;
        if let Some(slug) = outcome.slug_changed {
            problems.push(Problem::SlugChanged {
                file: d.md.rel.clone(),
                slug,
            });
        }
        docs[i].id = Some(outcome.id);
        docs[i].written = Some(outcome.result);
    }
    Ok(problems)
}

fn jiff_to_timestamp(t: jiff::Timestamp) -> Result<Timestamp, MdError> {
    Timestamp::from_micros(t.as_microsecond())
        .map_err(|_| MdError::Stopped(format!("date {t} is out of range")))
}

/// Sets `reading.front_page` to the root index (P62) under the import's option rule:
/// written on the first run; on a re-run only while the site still holds what the
/// import last set, else the site's choice is kept and reported. A dry run only
/// reports it.
async fn front_page(
    w: &mut Writer<'_>,
    cx: &Cx<'_>,
    docs: &[Doc],
    report: &mut MdReport,
) -> Result<(), MdError> {
    let Some(d) = cx.index.by_folder.get("").map(|&i| &docs[i]) else {
        report.problems.push(Problem::NoFrontPage);
        return Ok(());
    };
    let Some(id) = d.id.filter(|_| d.written.is_some()) else {
        // Skipped (another type): its problem is reported.
        return Ok(());
    };
    report.front_page = Some(d.md.rel.clone());
    if cx.opts.dry_run {
        return Ok(());
    }
    let mut batch = w.open().await?;
    let set = set_front_page(&mut batch, id).await;
    if !w.close(batch, set).await? {
        report.problems.push(Problem::FrontPageKept {
            file: d.md.rel.clone(),
        });
    }
    Ok(())
}

/// `false` when the site's own front page was kept.
async fn set_front_page(b: &mut ImportBatch<'_>, id: ContentId) -> Result<bool, MdError> {
    let value = json!(id.to_string());
    let wanted = value_hash(&value);
    let current = b.get_option(FRONT_PAGE_OPTION).await?;
    let write = match b.lookup(ImportMapKind::Option, FRONT_PAGE_MAP_ID).await? {
        None => true,
        Some(m) if m.hash == wanted => {
            // Nothing new to set; kept unless the site still has it.
            return Ok(current.as_ref() == Some(&value));
        }
        Some(m) => current.as_ref().map(value_hash).as_deref() == Some(m.hash.as_str()),
    };
    if write {
        b.options(&[(FRONT_PAGE_OPTION.to_owned(), Some(value))])
            .await?;
        b.remember(
            ImportMapKind::Option,
            FRONT_PAGE_MAP_ID,
            Uuid::nil(),
            &wanted,
            0,
        )
        .await?;
    }
    Ok(write)
}

/// The SHA-256 (hex) of a JSON value.
fn value_hash(value: &Value) -> String {
    hex(&Sha256::digest(value.to_string().as_bytes()))
}

/// The public path of every page that has an item, under the rules a batch pins.
async fn paths(w: &mut Writer<'_>, docs: &[Doc]) -> Result<HashMap<usize, String>, MdError> {
    let batch = w.open().await?;
    let found = read_paths(&batch, docs).await;
    w.close(batch, found).await
}

async fn read_paths(b: &ImportBatch<'_>, docs: &[Doc]) -> Result<HashMap<usize, String>, MdError> {
    let mut out = HashMap::new();
    for (i, d) in docs.iter().enumerate() {
        if let Some(id) = d.id
            && let Some(path) = b.path(id).await?
        {
            out.insert(i, path);
        }
    }
    Ok(out)
}

/// Pass 3: links rewritten to page paths (see the module docs).
async fn link_pass(
    w: &mut Writer<'_>,
    cx: &Cx<'_>,
    docs: &[Doc],
    media: &HashMap<String, ContentId>,
    paths: &HashMap<usize, String>,
) -> Result<(), MdError> {
    let todo: Vec<usize> = (0..docs.len())
        .filter(|&i| {
            docs[i].md.source.is_some()
                && matches!(
                    docs[i].written,
                    Some(Written::Created | Written::Updated | Written::Unchanged)
                )
        })
        .collect();
    for chunk in todo.chunks(ImportBatch::BATCH_ITEMS) {
        let mut batch = w.open().await?;
        let fixed = fix_links(&mut batch, cx, docs, chunk, media, paths).await;
        w.close(batch, fixed).await?;
    }
    Ok(())
}

async fn fix_links(
    b: &mut ImportBatch<'_>,
    cx: &Cx<'_>,
    docs: &[Doc],
    chunk: &[usize],
    media: &HashMap<String, ContentId>,
    paths: &HashMap<usize, String>,
) -> Result<(), MdError> {
    for &i in chunk {
        let d = &docs[i];
        let Some(id) = d.id else {
            continue;
        };
        let (Some(item), Some(mapped)) = (
            b.get_item(id).await?,
            b.lookup(ImportMapKind::Content, d.wp_id).await?,
        ) else {
            continue;
        };
        // Edited here since the import wrote it (an unchanged file's page too): kept.
        if mapped.id != id.as_uuid() || mapped.version != item.version {
            continue;
        }
        let (body, _) = body_of(d, cx, media, paths);
        if body != item.body {
            b.set_body_and_fields(id, d.wp_id, body, item.fields)
                .await?;
        }
    }
    Ok(())
}

/// Reports every page an earlier run of this source imported whose file is gone from
/// the folder (renamed or deleted) and was not skipped by this run.
async fn not_in_source(
    w: &mut Writer<'_>,
    docs: &[Doc],
    report: &mut MdReport,
) -> Result<(), MdError> {
    let mut here: HashSet<i64> = docs.iter().map(|d| d.wp_id).collect();
    here.extend(report.problems.iter().map(|p| map_id(p.file())));
    let batch = w.open().await?;
    let gone = list_gone(&batch, &here).await;
    let gone = w.close(batch, gone).await?;
    report
        .problems
        .extend(gone.into_iter().map(|page| Problem::NotInSource { page }));
    Ok(())
}

async fn list_gone(b: &ImportBatch<'_>, here: &HashSet<i64>) -> Result<Vec<String>, MdError> {
    let mut gone = Vec::new();
    let mut after = None;
    loop {
        let (rows, next) = b.mapped(ImportMapKind::Content, after, MAP_PAGE).await?;
        for (wp_id, m) in rows {
            if !here.contains(&wp_id)
                && let Some(path) = b.path(ContentId::from_uuid(m.id)).await?
            {
                gone.push(path);
            }
        }
        match next {
            Some(n) => after = Some(n),
            None => return Ok(gone),
        }
    }
}
