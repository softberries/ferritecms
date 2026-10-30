//! The folder walk: Markdown files and folders to a page tree (P62).

use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use ferrite_core::slug::slugify;
use sha2::{Digest, Sha256};

use crate::error::MdError;
use crate::front::{FrontMatter, split_front_matter};

/// The import format version, hashed with every file so a new format re-imports.
pub const FORMAT: &str = "md1";
/// The largest Markdown file an import reads (1 MiB).
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// The most Markdown files, and the most folders, one import walks.
pub const MAX_FILES: usize = 5000;

/// One page of the import: a Markdown file, or a folder without an index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdDoc {
    /// The file's path relative to the folder, `/`-separated; for a synthesized folder
    /// page, the folder's path.
    pub rel: String,
    /// The `rel` of the parent page; `None` at the top level.
    pub parent: Option<String>,
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
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadFrontMatter {
                file,
                line,
                message,
            } => write!(f, "{file}:{line}: skipped, {message}"),
            Self::TooLarge { file } => {
                write!(f, "{file}: skipped, larger than {} bytes", MAX_FILE_BYTES)
            }
            Self::OutsideDir { file, target } => {
                write!(
                    f,
                    "{file}: {target} is outside the folder or a symlink, not read"
                )
            }
            Self::MissingImage { file, src } => write!(f, "{file}: image {src} not found"),
            Self::BrokenLink { file, href } => {
                write!(f, "{file}: link {href} is not in the import, kept as is")
            }
            Self::SynthesizedSection { dir } => {
                write!(
                    f,
                    "{dir}/: no index.md or README.md, an empty page was made"
                )
            }
            Self::NotUtf8 { file } => write!(f, "{file}: skipped, not UTF-8"),
        }
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

fn hex(bytes: &[u8]) -> String {
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

/// Resolves `target` (an image `src` or a link `href` as the HTML holds it, percent-encoded)
/// from the page at `from_rel` inside `dir`. `None` for a URL with a scheme, a rooted or
/// fragment-only target, a path that leaves `dir`, passes a hidden or symlinked entry, or
/// does not exist. Nothing outside `dir` is read.
pub fn resolve(dir: &Path, from_rel: &str, target: &str) -> Option<Target> {
    let (path, fragment) = match target.split_once('#') {
        Some((p, f)) => (p, Some(f.to_owned())),
        None => (target, None),
    };
    let path = path.split('?').next().unwrap_or("");
    let path = percent_decode(path)?;
    let first = path.chars().next()?;
    if matches!(first, '/' | '\\')
        || path.contains(['\\', '\0'])
        || path.split('/').next().is_some_and(|h| h.contains(':'))
    {
        return None;
    }
    // The page's folder, then the target's segments, normalised without the file system.
    let mut segs: Vec<&str> = from_rel.split('/').collect();
    segs.pop();
    segs.retain(|s| !s.is_empty());
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop()?;
            }
            s if s.starts_with('.') => return None,
            s => segs.push(s),
        }
    }
    let root = dir.canonicalize().ok()?;
    // No component may be a symlink: checked one by one, never followed.
    let mut at = root.clone();
    for s in &segs {
        at.push(s);
        if std::fs::symlink_metadata(&at)
            .ok()?
            .file_type()
            .is_symlink()
        {
            return None;
        }
    }
    let path = at.canonicalize().ok()?;
    if !path.starts_with(&root) {
        return None;
    }
    Some(Target {
        path,
        rel: segs.join("/"),
        fragment,
    })
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
/// not UTF-8 or with invalid front matter are skipped and reported. A folder's page is its
/// `index.md` (else `README.md`), or a synthesized page when it has neither but holds
/// pages; a folder without pages gets none. Fails when `dir` is not a folder, cannot be
/// read, or holds more than [`MAX_FILES`] Markdown files or folders.
pub fn scan(dir: &Path) -> Result<Scan, MdError> {
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
        files: 0,
        folders: 0,
        scan: Scan::default(),
    };
    w.folder(&root, "", "", None)?;
    Ok(w.scan)
}

/// An entry of a folder, by kind.
enum Entry {
    Md(String),
    Folder(String),
}

struct Walker {
    files: usize,
    folders: usize,
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

/// A file read for the import: its front matter, the Markdown after it and its hash.
type ReadDoc = (FrontMatter, String, String);

impl Walker {
    /// The pages of folder `abs` (`rel`, called `name`; `""` for the root) and below it,
    /// under the page `parent`.
    fn folder(
        &mut self,
        abs: &Path,
        rel: &str,
        name: &str,
        parent: Option<&str>,
    ) -> Result<(), MdError> {
        let entries = self.entries(abs, rel)?;
        let index = ["index.md", "README.md"]
            .into_iter()
            .find(|i| entries.iter().any(|e| matches!(e, Entry::Md(n) if n == i)));
        let start = self.scan.docs.len();
        let root = rel.is_empty();
        // The folder's page: its index when that reads, else (later) a synthesized one.
        let mut page: Option<String> = None;
        if let Some(index) = index {
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
                self.push(file.clone(), parent, slug, order, read, title);
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
                Entry::Md(n) if Some(n.as_str()) == index => {}
                Entry::Md(n) => {
                    let file = join(rel, n);
                    if let Some(read) = self.read(&abs.join(n), &file)? {
                        let stem = n.get(..n.len() - 3).unwrap_or(n);
                        let (slug, order) = slug_order(stem);
                        let title = title_from_name(stem);
                        self.push(file, children_parent.as_deref(), slug, order, read, title);
                    }
                }
                Entry::Folder(n) => {
                    self.folder(&abs.join(n), &join(rel, n), n, children_parent.as_deref())?;
                }
            }
        }
        if !root && page.is_none() && self.scan.docs.len() > start {
            let (slug, order) = slug_order(name);
            self.scan.docs.insert(
                start,
                MdDoc {
                    rel: rel.to_owned(),
                    parent: parent.map(str::to_owned),
                    slug,
                    order,
                    front: FrontMatter::default(),
                    source: None,
                    hash: folder_hash(rel),
                    fallback_title: title_from_name(name),
                },
            );
            self.scan.problems.push(Problem::SynthesizedSection {
                dir: rel.to_owned(),
            });
        }
        Ok(())
    }

    fn push(
        &mut self,
        rel: String,
        parent: Option<&str>,
        slug: String,
        order: i32,
        read: ReadDoc,
        fallback_title: String,
    ) {
        let (front, source, hash) = read;
        self.scan.docs.push(MdDoc {
            rel,
            parent: parent.map(str::to_owned),
            slug: front.slug.clone().unwrap_or(slug),
            order: front.order.unwrap_or(order),
            front,
            source: Some(source),
            hash,
            fallback_title,
        });
    }

    /// The Markdown files and folders of `abs`, sorted by name; hidden entries skipped,
    /// symlinks reported (a `*.md` one or one to a folder) and never followed.
    fn entries(&mut self, abs: &Path, rel: &str) -> Result<Vec<Entry>, MdError> {
        let io = |source| MdError::Io {
            path: abs.to_path_buf(),
            source,
        };
        let mut found = Vec::new();
        for entry in std::fs::read_dir(abs).map_err(io)? {
            let entry = entry.map_err(io)?;
            found.push(entry.file_name());
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
            let meta = std::fs::symlink_metadata(&path).map_err(|source| MdError::Io {
                path: path.clone(),
                source,
            })?;
            let is_md = name.len() > 3 && name.to_ascii_lowercase().ends_with(".md");
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
                self.folders += 1;
                if self.folders > MAX_FILES {
                    return Err(MdError::TooManyFiles { limit: MAX_FILES });
                }
                out.push(Entry::Folder(name.to_owned()));
            } else if meta.is_file() && is_md {
                self.files += 1;
                if self.files > MAX_FILES {
                    return Err(MdError::TooManyFiles { limit: MAX_FILES });
                }
                out.push(Entry::Md(name.to_owned()));
            }
        }
        Ok(out)
    }

    /// Reads the Markdown file `abs` (`rel`): its front matter, Markdown and hash, or
    /// `None` when it was skipped (and reported).
    fn read(&mut self, abs: &Path, rel: &str) -> Result<Option<ReadDoc>, MdError> {
        let skip = |p: Problem, s: &mut Scan| {
            s.problems.push(p);
            None
        };
        let io = |source| MdError::Io {
            path: abs.to_path_buf(),
            source,
        };
        let file = std::fs::File::open(abs).map_err(io)?;
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Ok(skip(
                Problem::TooLarge {
                    file: rel.to_owned(),
                },
                &mut self.scan,
            ));
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
}
