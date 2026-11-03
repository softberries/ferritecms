//! The folder walk: Markdown files and folders to a page tree (P62).

use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use ferrite_core::content::seo::MAX_SEO_DESCRIPTION;
use ferrite_core::slug::slugify;
use sha2::{Digest, Sha256};

use crate::error::MdError;
use crate::front::{FrontMatter, split_front_matter};

/// The import format version, hashed with every file so a new format re-imports.
pub const FORMAT: &str = "md1";
/// The largest Markdown file an import reads (1 MiB).
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// The most Markdown files, and the most folders holding pages, one import takes.
pub const MAX_FILES: usize = 5000;
/// The most bytes of Markdown one import reads in all (256 MiB): the sources stay in
/// memory for the whole run (P78).
pub const MAX_TOTAL_BYTES: u64 = 256 << 20;
/// The most folders one scan visits, with pages or without (P78).
pub const MAX_VISITED_FOLDERS: usize = 50_000;
/// The deepest folder a scan walks (the folder itself at depth 1): core's parent chain
/// limit, so a file in the deepest folder still has a valid parent chain.
pub const MAX_DEPTH: usize = 64;

/// One page of the import: a Markdown file, or a folder without an index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdDoc {
    /// The file's path relative to the folder, `/`-separated; for a synthesized folder
    /// page, the folder's path.
    pub rel: String,
    /// The `rel` of the parent page; `None` at the top level.
    pub parent: Option<String>,
    /// The folder this page stands for (`/`-separated, `""` for the import's root): set
    /// for a folder's index or synthesized page, `None` for other files.
    pub folder: Option<String>,
    /// A synthesized folder page whose `index.md`/`README.md` exists but was skipped (and
    /// reported): it stands in only so the folder's files have a parent, and a run must
    /// not write it over a page the index filled earlier.
    pub index_skipped: bool,
    pub slug: String,
    pub order: i32,
    pub front: FrontMatter,
    /// The Markdown after the front matter; `None` for a synthesized folder page.
    pub source: Option<String>,
    /// SHA-256 (hex) of the file's bytes then [`FORMAT`]; for a synthesized page, of
    /// [`FORMAT`] then the folder's `rel`.
    pub hash: String,
    /// The title when neither the front matter nor a first `# H1` gives one: the file
    /// stem, or the folder name for a folder's page, as "Getting started"; "Home" for the
    /// root index.
    pub fallback_title: String,
}

/// The pages of a folder and what was skipped or reported on the way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// Parents before children.
    pub docs: Vec<MdDoc>,
    pub problems: Vec<Problem>,
}

/// Something an import reports. Every `file` and `dir` is a `/`-separated path relative to
/// the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The file was skipped: its front matter is invalid (`line` is 1-based).
    BadFrontMatter {
        file: String,
        line: usize,
        message: String,
    },
    /// The file was skipped: it is larger than [`MAX_FILE_BYTES`].
    TooLarge { file: String },
    /// The front matter `description` was longer than the SEO description's limit and
    /// was cut to it (it is the excerpt and the SEO description).
    DescriptionCut { file: String },
    /// A symlink (never followed), or an image or link leaving the folder.
    OutsideDir { file: String, target: String },
    /// A relative image source not found inside the folder.
    MissingImage { file: String, src: String },
    /// A link to a `.md` file or folder that is not in the import.
    BrokenLink { file: String, href: String },
    /// A folder without `index.md` or `README.md` got a page with an empty body.
    SynthesizedSection { dir: String },
    /// The file was skipped: it is not UTF-8 (or its name is not).
    NotUtf8 { file: String },
    /// The file or folder was skipped: it could not be read.
    Unreadable { file: String, message: String },
    /// The folder was skipped: it is more than [`MAX_DEPTH`] folders deep.
    TooDeep { dir: String },
    /// The image `file` was not imported: the upload rules refused it (size or type).
    RefusedImage { file: String, message: String },
    /// The image `file` changed since the last run; the media item keeps the file it
    /// was imported with.
    ImageChanged { file: String },
    /// The page of `file` got slug `slug`, not the one its name or front matter gives:
    /// a sibling already had it (or it is reserved).
    SlugChanged { file: String, slug: String },
    /// The file was skipped: an earlier run imported it as type `kind`, and a page
    /// cannot change its type.
    OtherType { file: String, kind: String },
    /// A page an earlier run of this source imported whose file is no longer in the
    /// folder (renamed or deleted): it stays on the site. `page` is its path.
    NotInSource { page: String },
    /// `--front-page` was given but the folder has no `index.md` or `README.md`.
    NoFrontPage,
    /// The site's front page was changed here since the import set it: kept.
    FrontPageKept { file: String },
    /// `--front-page`: the root index `file` is not public after the run (a draft, or
    /// scheduled for later), so `reading.front_page` was left as it was (ruling R-b12).
    FrontPageNotPublic { file: String },
    /// The file was skipped: its locale (front matter or `--locale`) is not one of the
    /// site's (M6-1).
    Locale { file: String, locale: String },
    /// The page's `translation_of` was not applied: `message` says why (not in the
    /// import, a cycle, the group has the locale already, edited here). M6-1.
    TranslationOf { file: String, message: String },
}

impl Problem {
    /// A short name of the kind of problem, for the printed report.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BadFrontMatter { .. } => "bad-front-matter",
            Self::TooLarge { .. } => "too-large",
            Self::DescriptionCut { .. } => "description-cut",
            Self::OutsideDir { .. } => "outside",
            Self::MissingImage { .. } => "missing-image",
            Self::BrokenLink { .. } => "broken-link",
            Self::SynthesizedSection { .. } => "synthesized",
            Self::NotUtf8 { .. } => "not-utf8",
            Self::Unreadable { .. } => "unreadable",
            Self::TooDeep { .. } => "too-deep",
            Self::RefusedImage { .. } => "refused-image",
            Self::ImageChanged { .. } => "image-changed",
            Self::SlugChanged { .. } => "slug-changed",
            Self::OtherType { .. } => "other-type",
            Self::NotInSource { .. } => "not-in-source",
            Self::NoFrontPage => "no-front-page",
            Self::FrontPageKept { .. } => "front-page-kept",
            Self::FrontPageNotPublic { .. } => "front-page-not-public",
            Self::Locale { .. } => "locale",
            Self::TranslationOf { .. } => "translation-of",
        }
    }

    /// The file, folder or page the problem is about.
    pub fn file(&self) -> &str {
        match self {
            Self::BadFrontMatter { file, .. }
            | Self::TooLarge { file }
            | Self::DescriptionCut { file }
            | Self::OutsideDir { file, .. }
            | Self::MissingImage { file, .. }
            | Self::BrokenLink { file, .. }
            | Self::NotUtf8 { file }
            | Self::Unreadable { file, .. }
            | Self::RefusedImage { file, .. }
            | Self::ImageChanged { file }
            | Self::SlugChanged { file, .. }
            | Self::OtherType { file, .. }
            | Self::FrontPageKept { file }
            | Self::FrontPageNotPublic { file }
            | Self::Locale { file, .. }
            | Self::TranslationOf { file, .. } => file,
            Self::SynthesizedSection { dir } | Self::TooDeep { dir } => dir,
            Self::NotInSource { page } => page,
            Self::NoFrontPage => "index.md",
        }
    }

    /// What happened, without the file.
    pub fn detail(&self) -> String {
        match self {
            Self::BadFrontMatter { line, message, .. } => {
                format!("line {line}: skipped, {message}")
            }
            Self::TooLarge { .. } => format!("skipped, larger than {MAX_FILE_BYTES} bytes"),
            Self::DescriptionCut { .. } => format!(
                "the description is longer than {MAX_SEO_DESCRIPTION} characters; cut to \
                 {MAX_SEO_DESCRIPTION}"
            ),
            Self::OutsideDir { target, .. } => {
                format!("{target} is outside the folder or a symlink, not read")
            }
            Self::MissingImage { src, .. } => format!("image {src} not found in the folder"),
            Self::BrokenLink { href, .. } => {
                format!("link {href} is not in the import, kept as is")
            }
            Self::SynthesizedSection { .. } => {
                "no index.md or README.md, an empty page was made".to_owned()
            }
            Self::NotUtf8 { .. } => "skipped, not UTF-8".to_owned(),
            Self::Unreadable { message, .. } => format!("skipped, cannot be read: {message}"),
            Self::TooDeep { .. } => {
                format!("skipped, nested deeper than {MAX_DEPTH} folders")
            }
            Self::RefusedImage { message, .. } => format!("image not imported: {message}"),
            Self::ImageChanged { .. } => {
                "the image changed; the media item keeps the file first imported, a re-run \
                 never replaces it (replace it in the media library)"
                    .to_owned()
            }
            Self::SlugChanged { slug, .. } => {
                format!("its slug was taken, the page got slug {slug}")
            }
            Self::OtherType { kind, .. } => {
                format!("skipped, an earlier run imported it as type {kind}")
            }
            Self::NotInSource { .. } => {
                "imported by an earlier run, no longer in the folder; kept".to_owned()
            }
            Self::NoFrontPage => "--front-page: the folder has no index.md or README.md".to_owned(),
            Self::FrontPageKept { .. } => {
                "the front page was changed here since the import set it; kept".to_owned()
            }
            Self::FrontPageNotPublic { .. } => {
                "--front-page: the page is not public (a draft, or scheduled for later); \
                 reading.front_page left unchanged"
                    .to_owned()
            }
            Self::Locale { locale, .. } => {
                format!("skipped, {locale} is not one of the site's locales (Settings → Languages)")
            }
            Self::TranslationOf { message, .. } => {
                format!("translation_of not applied: {message}")
            }
        }
    }

    /// Whether the problem is only a notice: the run still exits 0 with it (a
    /// synthesized section, a changed slug, a page no longer in the folder, a changed
    /// image, a front page kept, a description cut). The others make the exit code 2
    /// (P62).
    pub fn is_notice(&self) -> bool {
        matches!(
            self,
            Self::SynthesizedSection { .. }
                | Self::DescriptionCut { .. }
                | Self::SlugChanged { .. }
                | Self::NotInSource { .. }
                | Self::ImageChanged { .. }
                | Self::FrontPageKept { .. }
        )
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file(), self.detail())
    }
}

/// The `import_map` key of `rel` (P62): the first 8 bytes of SHA-256 of the path as a
/// big-endian `i64`, masked to be non-negative.
pub fn map_id(rel: &str) -> i64 {
    let d = Sha256::digest(rel.as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&d[..8]);
    i64::from_be_bytes(first) & i64::MAX
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn file_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.update(FORMAT.as_bytes());
    hex(&h.finalize())
}

fn folder_hash(rel: &str) -> String {
    let mut h = Sha256::new();
    h.update(FORMAT.as_bytes());
    h.update(rel.as_bytes());
    hex(&h.finalize())
}

/// Opens `path` for reading without following a final symlink (unix: `O_NOFOLLOW |
/// O_NONBLOCK`, so a link fails with `ELOOP` and a FIFO does not block), and checks that
/// the opened handle is a regular file: the path was checked without following links,
/// and this closes the window in which it could be swapped for a link.
pub(crate) fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = opts.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

/// A relative image source or link resolved inside the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The canonical path of the file or folder.
    pub path: PathBuf,
    /// Its path relative to the folder, `/`-separated (empty for the folder itself).
    pub rel: String,
    /// The `#fragment`, without `#`.
    pub fragment: Option<String>,
}

/// Why [`locate`] found no target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// Not a relative path: a URL with a scheme, a rooted path, or only a fragment or
    /// query. Nothing to look up.
    NotRelative,
    /// The path leaves the folder (climbs above it, passes a symlink, or holds `\` or
    /// NUL). Nothing outside the folder is read.
    Outside,
    /// Inside the folder, but there is no such entry (or it is hidden, or the path is
    /// not UTF-8 once decoded).
    Missing,
}

/// Resolves `target` (an image `src` or a link `href` as the HTML holds it, percent-encoded)
/// from the page at `from_rel` inside `dir`; see [`locate`] for why it may find nothing.
pub fn resolve(dir: &Path, from_rel: &str, target: &str) -> Option<Target> {
    locate(dir, from_rel, target).ok()
}

/// Resolves `target` (an image `src` or a link `href` as the HTML holds it, percent-encoded)
/// from the page at `from_rel` inside `dir`, or says why it cannot ([`Miss`]). The path
/// is [`lexical`]ly normalised first, then each component is checked with `lstat` (a
/// symlink is outside, never followed) and the result must stay under `dir`'s canonical
/// path. Nothing outside `dir` is read.
pub fn locate(dir: &Path, from_rel: &str, target: &str) -> Result<Target, Miss> {
    let (rel, fragment) = lexical(from_rel, target)?;
    let root = dir.canonicalize().map_err(|_| Miss::Missing)?;
    // No component may be a symlink: checked one by one, never followed.
    let mut at = root.clone();
    for s in rel.split('/').filter(|s| !s.is_empty()) {
        at.push(s);
        let meta = std::fs::symlink_metadata(&at).map_err(|_| Miss::Missing)?;
        if meta.file_type().is_symlink() {
            return Err(Miss::Outside);
        }
    }
    let path = at.canonicalize().map_err(|_| Miss::Missing)?;
    if !path.starts_with(&root) {
        return Err(Miss::Outside);
    }
    Ok(Target {
        path,
        rel,
        fragment,
    })
}

/// The path `target` names from the page at `from_rel`, relative to the folder, and its
/// `#fragment`, without touching the file system: percent-decoded and normalised. A URL
/// with a scheme, a rooted path or a bare fragment is [`Miss::NotRelative`]; a `..` above
/// the folder, `\` or NUL is [`Miss::Outside`]; a hidden segment or a path that is not
/// UTF-8 once decoded is [`Miss::Missing`].
pub fn lexical(from_rel: &str, target: &str) -> Result<(String, Option<String>), Miss> {
    let (path, fragment) = match target.split_once('#') {
        Some((p, f)) => (p, Some(f.to_owned())),
        None => (target, None),
    };
    let path = path.split('?').next().unwrap_or("");
    let Some(first) = path.chars().next() else {
        return Err(Miss::NotRelative);
    };
    if matches!(first, '/') || path.split('/').next().is_some_and(|h| h.contains(':')) {
        return Err(Miss::NotRelative);
    }
    let path = percent_decode(path).ok_or(Miss::Missing)?;
    if path.starts_with('/') || path.split('/').next().is_some_and(|h| h.contains(':')) {
        return Err(Miss::NotRelative);
    }
    if path.contains(['\\', '\0']) {
        return Err(Miss::Outside);
    }
    // The page's folder, then the target's segments.
    let mut segs: Vec<&str> = from_rel.split('/').collect();
    segs.pop();
    segs.retain(|s| !s.is_empty());
    let mut hidden = false;
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop().ok_or(Miss::Outside)?;
            }
            s => {
                hidden |= s.starts_with('.');
                segs.push(s);
            }
        }
    }
    if hidden {
        return Err(Miss::Missing);
    }
    Ok((segs.join("/"), fragment))
}

/// `%XX` escapes decoded; `None` when the result is not UTF-8. An invalid escape is kept.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex_at = |j: usize| b.get(j).and_then(|c| (*c as char).to_digit(16));
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (hex_at(i + 1), hex_at(i + 2))
        {
            out.push(u8::try_from(h * 16 + l).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Walks `dir` into pages (P62). Hidden entries are skipped; symlinks are never followed
/// (one named `*.md` or pointing at a folder is reported); files over [`MAX_FILE_BYTES`],
/// not UTF-8, unreadable or with invalid front matter, unreadable folders and folders
/// deeper than [`MAX_DEPTH`] are skipped and reported. A folder's page is its `index.md`
/// (else `README.md`; both matched ignoring case), or a synthesized page when it has
/// neither but holds pages; a folder without pages gets none. Fails when `dir` is not a
/// readable folder, or holds more than [`MAX_FILES`] Markdown files or folders with
/// pages, more than [`MAX_VISITED_FOLDERS`] folders in all, or Markdown files of more
/// than [`MAX_TOTAL_BYTES`] in all (P78).
pub fn scan(dir: &Path) -> Result<Scan, MdError> {
    scan_with(dir, Limits::DEFAULT)
}

/// The scan's limits; tests use small ones.
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// Markdown files.
    files: usize,
    /// Folders holding pages.
    folders: usize,
    /// Folders seen, with pages or without.
    visited: usize,
    /// Bytes of Markdown read in all.
    total_bytes: u64,
}

impl Limits {
    const DEFAULT: Limits = Limits {
        files: MAX_FILES,
        folders: MAX_FILES,
        visited: MAX_VISITED_FOLDERS,
        total_bytes: MAX_TOTAL_BYTES,
    };
}

/// [`scan`] with `limits`.
fn scan_with(dir: &Path, limits: Limits) -> Result<Scan, MdError> {
    let io = |source| MdError::Io {
        path: dir.to_path_buf(),
        source,
    };
    let root = dir.canonicalize().map_err(io)?;
    if !root.metadata().map_err(io)?.is_dir() {
        return Err(MdError::NotADirectory {
            path: dir.to_path_buf(),
        });
    }
    let mut w = Walker {
        limits,
        files: 0,
        folders: 0,
        visited: 0,
        bytes: 0,
        scan: Scan::default(),
    };
    let entries = std::fs::read_dir(&root).map_err(io)?;
    w.folder(&root, "", "", None, Some(entries))?;
    Ok(w.scan)
}

/// An entry of a folder, by kind.
enum Entry {
    Md(String),
    Folder(String),
}

struct Walker {
    limits: Limits,
    files: usize,
    /// Folders holding pages.
    folders: usize,
    /// Folders seen.
    visited: usize,
    /// Bytes of Markdown read.
    bytes: u64,
    scan: Scan,
}

/// `rel` joined under `base` (`""` is the root).
fn join(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_owned()
    } else {
        format!("{base}/{name}")
    }
}

/// How deep folder `rel` is (the root is 0).
fn depth(rel: &str) -> usize {
    if rel.is_empty() {
        0
    } else {
        rel.split('/').count()
    }
}

/// A file read for the import: its front matter, the Markdown after it and its hash.
type ReadDoc = (FrontMatter, String, String);

impl Walker {
    /// The pages of folder `abs` (`rel`, called `name`; `""` for the root) and below it,
    /// under the page `parent`. `opened` is the root's listing, already opened (a root
    /// that cannot be read fails the scan; any other folder is reported and skipped).
    fn folder(
        &mut self,
        abs: &Path,
        rel: &str,
        name: &str,
        parent: Option<&str>,
        opened: Option<std::fs::ReadDir>,
    ) -> Result<(), MdError> {
        let reported = self.scan.problems.len();
        let Some(entries) = self.entries(abs, rel, opened)? else {
            return Ok(());
        };
        // An index the listing dropped and reported (a symlink, or one that could not be
        // `lstat`ed) is a skipped index, as one whose read fails.
        let dropped_index = self.scan.problems[reported..].iter().any(|p| match p {
            Problem::OutsideDir { file, .. } | Problem::Unreadable { file, .. } => file
                .strip_prefix(rel)
                .map(|f| f.trim_start_matches('/'))
                .is_some_and(is_index_name),
            _ => false,
        });
        let index = ["index.md", "readme.md"].into_iter().find_map(|i| {
            entries.iter().find_map(|e| match e {
                Entry::Md(n) if n.eq_ignore_ascii_case(i) => Some(n.clone()),
                _ => None,
            })
        });
        let start = self.scan.docs.len();
        let root = rel.is_empty();
        // The folder's page: its index when that reads, else (later) a synthesized one.
        let mut page: Option<String> = None;
        if let Some(index) = &index {
            let file = join(rel, index);
            if let Some(read) = self.read(&abs.join(index), &file)? {
                let (slug, order) = if root {
                    ("home".to_owned(), 0)
                } else {
                    slug_order(name)
                };
                let title = if root {
                    "Home".to_owned()
                } else {
                    title_from_name(name)
                };
                self.push(file.clone(), parent, Some(rel), (slug, order), read, title);
                page = Some(file);
            }
        }
        // The root has no page of its own: its index is a top-level page like the rest.
        let children_parent = if root {
            None
        } else {
            Some(page.clone().unwrap_or_else(|| rel.to_owned()))
        };
        for e in &entries {
            match e {
                Entry::Md(n) if Some(n) == index.as_ref() => {}
                Entry::Md(n) => {
                    let file = join(rel, n);
                    if let Some(read) = self.read(&abs.join(n), &file)? {
                        let stem = n.get(..n.len() - 3).unwrap_or(n);
                        self.push(
                            file,
                            children_parent.as_deref(),
                            None,
                            slug_order(stem),
                            read,
                            title_from_name(stem),
                        );
                    }
                }
                Entry::Folder(n) => {
                    let sub = join(rel, n);
                    if depth(&sub) > MAX_DEPTH {
                        self.scan.problems.push(Problem::TooDeep { dir: sub });
                        continue;
                    }
                    self.folder(&abs.join(n), &sub, n, children_parent.as_deref(), None)?;
                }
            }
        }
        if !root && page.is_none() && self.scan.docs.len() > start {
            let (slug, order) = slug_order(name);
            // The index exists but was skipped: its own problem line says so.
            let index_skipped = index.is_some() || dropped_index;
            self.scan.docs.insert(
                start,
                MdDoc {
                    rel: rel.to_owned(),
                    parent: parent.map(str::to_owned),
                    folder: Some(rel.to_owned()),
                    index_skipped,
                    slug,
                    order,
                    front: FrontMatter::default(),
                    source: None,
                    hash: folder_hash(rel),
                    fallback_title: title_from_name(name),
                },
            );
            if !index_skipped {
                self.scan.problems.push(Problem::SynthesizedSection {
                    dir: rel.to_owned(),
                });
            }
        }
        // A folder holding pages (its own, or one further down) counts; an asset tree
        // does not (P78).
        if !root && self.scan.docs.len() > start {
            self.folders += 1;
            if self.folders > self.limits.folders {
                return Err(MdError::TooManyFiles {
                    limit: self.limits.folders,
                });
            }
        }
        Ok(())
    }

    fn push(
        &mut self,
        rel: String,
        parent: Option<&str>,
        folder: Option<&str>,
        (slug, order): (String, i32),
        read: ReadDoc,
        fallback_title: String,
    ) {
        let (front, source, hash) = read;
        if front.description_cut {
            self.scan
                .problems
                .push(Problem::DescriptionCut { file: rel.clone() });
        }
        self.scan.docs.push(MdDoc {
            rel,
            parent: parent.map(str::to_owned),
            folder: folder.map(str::to_owned),
            index_skipped: false,
            slug: front.slug.clone().unwrap_or(slug),
            order: front.order.unwrap_or(order),
            front,
            source: Some(source),
            hash,
            fallback_title,
        });
    }

    fn unreadable(&mut self, file: String, e: &std::io::Error) {
        self.scan.problems.push(Problem::Unreadable {
            file,
            message: e.to_string(),
        });
    }

    /// The Markdown files and folders of `abs`, sorted by name; hidden entries skipped,
    /// symlinks reported (a `*.md` one or one to a folder) and never followed. `None`
    /// (reported) when the folder cannot be listed.
    fn entries(
        &mut self,
        abs: &Path,
        rel: &str,
        opened: Option<std::fs::ReadDir>,
    ) -> Result<Option<Vec<Entry>>, MdError> {
        let listing = match opened.map_or_else(|| std::fs::read_dir(abs), Ok) {
            Ok(listing) => listing,
            Err(e) => {
                self.unreadable(rel.to_owned(), &e);
                return Ok(None);
            }
        };
        let mut found = Vec::new();
        for entry in listing {
            match entry {
                Ok(entry) => found.push(entry.file_name()),
                Err(e) => {
                    self.unreadable(rel.to_owned(), &e);
                    return Ok(None);
                }
            }
        }
        found.sort();
        let mut out = Vec::new();
        for os in found {
            let Some(name) = os.to_str() else {
                // Reported when it would have been a page: a `*.md` file or a folder.
                let lossy = os.to_string_lossy();
                let matters = lossy.to_ascii_lowercase().ends_with(".md")
                    || std::fs::symlink_metadata(abs.join(&os)).is_ok_and(|m| m.is_dir());
                if !lossy.starts_with('.') && matters {
                    self.scan.problems.push(Problem::NotUtf8 {
                        file: join(rel, &lossy),
                    });
                }
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let path = abs.join(name);
            let is_md = name.len() > 3 && name.to_ascii_lowercase().ends_with(".md");
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(e) => {
                    // Gone since the listing, or not even `lstat`-able.
                    self.unreadable(join(rel, name), &e);
                    continue;
                }
            };
            if meta.file_type().is_symlink() {
                // Only `stat` the link (never open it) to tell a folder from a file.
                if is_md || std::fs::metadata(&path).map_or(true, |m| m.is_dir()) {
                    let target = std::fs::read_link(&path)
                        .map(|t| t.display().to_string())
                        .unwrap_or_else(|_| "?".to_owned());
                    self.scan.problems.push(Problem::OutsideDir {
                        file: join(rel, name),
                        target,
                    });
                }
            } else if meta.is_dir() {
                // Every folder counts here; only those holding pages count toward
                // the folder limit (`folder`, P78).
                self.visited += 1;
                if self.visited > self.limits.visited {
                    return Err(MdError::TooManyFolders {
                        limit: self.limits.visited,
                    });
                }
                out.push(Entry::Folder(name.to_owned()));
            } else if meta.is_file() && is_md {
                self.files += 1;
                if self.files > self.limits.files {
                    return Err(MdError::TooManyFiles {
                        limit: self.limits.files,
                    });
                }
                out.push(Entry::Md(name.to_owned()));
            } else if is_md {
                // A FIFO, socket or device named `*.md` is never opened; reported, so
                // an index name among them counts as a skipped index.
                let e = std::io::Error::other("not a regular file");
                self.unreadable(join(rel, name), &e);
            }
        }
        Ok(Some(out))
    }

    /// Reads the Markdown file `abs` (`rel`): its front matter, Markdown and hash, or
    /// `None` when it was skipped (and reported).
    fn read(&mut self, abs: &Path, rel: &str) -> Result<Option<ReadDoc>, MdError> {
        let skip = |p: Problem, s: &mut Scan| {
            s.problems.push(p);
            None
        };
        let mut bytes = Vec::new();
        let read = open_nofollow(abs).and_then(|file| {
            file.take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map(|_| ())
        });
        if let Err(e) = read {
            self.unreadable(rel.to_owned(), &e);
            return Ok(None);
        }
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Ok(skip(
                Problem::TooLarge {
                    file: rel.to_owned(),
                },
                &mut self.scan,
            ));
        }
        // Every file read within the per-file limit counts, kept or skipped later: the
        // kept ones stay in memory for the whole run (P78).
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        if self.bytes > self.limits.total_bytes {
            return Err(MdError::TooMuchText {
                limit: self.limits.total_bytes,
            });
        }
        let hash = file_hash(&bytes);
        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(skip(
                Problem::NotUtf8 {
                    file: rel.to_owned(),
                },
                &mut self.scan,
            ));
        };
        match split_front_matter(&text, Path::new(rel)) {
            Ok((front, body)) => Ok(Some((front, body.to_owned(), hash))),
            Err(MdError::FrontMatter { line, message, .. }) => Ok(skip(
                Problem::BadFrontMatter {
                    file: rel.to_owned(),
                    line,
                    message,
                },
                &mut self.scan,
            )),
            Err(e) => Err(e),
        }
    }
}

/// Whether a file name is a folder's index: `index.md` or `README.md`, any case.
pub(crate) fn is_index_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("index.md") || name.eq_ignore_ascii_case("readme.md")
}

/// The slug and order a file stem or folder name gives: `NN-name` is `name` and `NN`.
fn slug_order(name: &str) -> (String, i32) {
    match split_number(name) {
        Some((n, rest)) => (slugify(rest), n),
        None => (slugify(name), 0),
    }
}

/// `NN-rest` as `(NN, rest)` when `NN` is digits that fit an `i32` and `rest` is not empty.
fn split_number(name: &str) -> Option<(i32, &str)> {
    let (digits, rest) = name.split_once('-')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || rest.is_empty() {
        return None;
    }
    Some((digits.parse().ok()?, rest))
}

/// A title from a file stem or folder name: "01-getting-started" is "Getting started".
fn title_from_name(name: &str) -> String {
    let base = split_number(name).map_or(name, |(_, rest)| rest);
    let words: Vec<&str> = base
        .split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .collect();
    let joined = words.join(" ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn map_id_is_pinned_non_negative_and_distinct() {
        assert_eq!(map_id("guides/writing.md"), 7_832_887_085_166_746_328);
        assert_eq!(map_id("index.md"), 2_632_150_344_495_023_335);
        assert_eq!(map_id("guides/writing.md"), map_id("guides/writing.md"));
        assert_ne!(map_id("guides/writing.md"), map_id("guides/README.md"));
        for rel in ["a.md", "b.md", "guides", "x/y/z.md", ""] {
            assert!(map_id(rel) >= 0);
        }
    }

    #[test]
    fn names_give_slug_and_order() {
        assert_eq!(slug_order("01-install"), ("install".to_owned(), 1));
        assert_eq!(
            slug_order("10-Getting Started"),
            ("getting-started".to_owned(), 10)
        );
        assert_eq!(slug_order("writing"), ("writing".to_owned(), 0));
        assert_eq!(slug_order("01-"), ("01".to_owned(), 0));
        assert_eq!(slug_order("-x"), ("x".to_owned(), 0));
        assert_eq!(slug_order("99999999999-x"), ("99999999999-x".to_owned(), 0));
    }

    #[test]
    fn titles_from_names() {
        assert_eq!(title_from_name("getting-started"), "Getting started");
        assert_eq!(
            title_from_name("02-configure_the_site"),
            "Configure the site"
        );
        assert_eq!(title_from_name("żółw"), "Żółw");
        assert_eq!(title_from_name("01-"), "01");
    }

    fn limits() -> Limits {
        Limits::DEFAULT
    }

    fn write(dir: &Path, rel: &str, body: &[u8]) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// P78: the files read add up; past the total cap the scan aborts, and the error
    /// names the cap.
    #[test]
    fn sources_over_the_total_cap_abort_naming_it() {
        assert_eq!(MAX_TOTAL_BYTES, 256 << 20);
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.md", b"123456");
        write(dir.path(), "b.md", b"1234");
        let at = |total_bytes| Limits {
            total_bytes,
            ..limits()
        };
        // Exactly at the cap: read.
        assert_eq!(scan_with(dir.path(), at(10)).unwrap().docs.len(), 2);
        write(dir.path(), "c.md", b"x");
        let err = scan_with(dir.path(), at(10)).unwrap_err();
        assert!(matches!(err, MdError::TooMuchText { limit: 10 }), "{err:?}");
        let message = err.to_string();
        assert!(message.contains("10 bytes"), "{message}");
        // The real cap is named in MiB too.
        let message = MdError::TooMuchText {
            limit: MAX_TOTAL_BYTES,
        }
        .to_string();
        assert!(message.contains("256 MiB"), "{message}");
    }

    /// P78: only folders that hold a page count toward the folder limit; asset trees
    /// (no Markdown below them) do not.
    #[test]
    fn folders_without_pages_do_not_count() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..=MAX_FILES {
            std::fs::create_dir_all(dir.path().join(format!("assets/{i}"))).unwrap();
        }
        write(dir.path(), "index.md", b"# Home");
        let scanned = scan(dir.path()).unwrap();
        assert_eq!(scanned.docs.len(), 1);
        // Folders with pages (an index, a file, or a page further down) count.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a/index.md", b"");
        write(dir.path(), "b/x.md", b"");
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();
        let at = |folders| Limits {
            folders,
            ..limits()
        };
        assert_eq!(scan_with(dir.path(), at(2)).unwrap().docs.len(), 3);
        write(dir.path(), "c/d/e.md", b"");
        let err = scan_with(dir.path(), at(3)).unwrap_err();
        assert!(matches!(err, MdError::TooManyFiles { limit: 3 }), "{err:?}");
    }

    /// P78: a folder tree with more folders than the visited cap aborts, pages or not,
    /// with a message naming the cap and what to do.
    #[test]
    fn visited_folder_cap_aborts() {
        assert_eq!(MAX_VISITED_FOLDERS, 50_000);
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            std::fs::create_dir_all(dir.path().join(format!("n/{i}"))).unwrap();
        }
        write(dir.path(), "index.md", b"");
        let at = |visited| Limits {
            visited,
            ..limits()
        };
        // `n` and its three: four folders.
        assert!(scan_with(dir.path(), at(4)).is_ok());
        let err = scan_with(dir.path(), at(3)).unwrap_err();
        assert!(
            matches!(err, MdError::TooManyFolders { limit: 3 }),
            "{err:?}"
        );
        let message = err.to_string();
        assert!(message.contains("more than 3 folders"), "{message}");
        assert!(message.contains("point the import at"), "{message}");
    }
}
