//! Theme packages (06 §1): reading the files, validating, hashing, compiling.
//!
//! Every file is read into memory by [`Theme::load`]; nothing touches the filesystem
//! afterwards. Templates are compiled into the theme's MiniJinja environment at load.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use minijinja::Environment;
use rust_embed::Embed;
use sha2::{Digest, Sha256};

use crate::{
    ThemeError,
    manifest::{self, ThemeManifest},
    render,
};

/// Most files a theme may contain.
pub const MAX_FILES: usize = 2000;
/// Most bytes all files of a theme may hold together (20 MiB).
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
/// Deepest directory nesting read from a theme folder.
const MAX_DEPTH: usize = 16;

const MANIFEST: &str = "theme.toml";
const TEMPLATES: &str = "templates/";
const ASSETS: &str = "assets/";
const INDEX: &str = "index.html";

/// Starter themes compiled into the binary (`themes/` at the workspace root).
#[derive(Embed)]
#[folder = "../../themes/"]
struct Embedded;

/// Where a theme's files come from (plan ruling P1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeSource {
    /// A starter theme embedded in the binary, by folder name under `themes/`.
    Embedded(&'static str),
    /// A theme folder on disk (`data/themes/<name>/`). Symlinks are refused.
    Dir(PathBuf),
}

/// A loaded, validated theme with its templates compiled.
pub struct Theme {
    manifest: ThemeManifest,
    hash: String,
    templates: BTreeSet<String>,
    assets: BTreeMap<String, Cow<'static, [u8]>>,
    pub(crate) env: Environment<'static>,
}

impl std::fmt::Debug for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Theme")
            .field("name", &self.manifest.name)
            .field("hash", &self.hash)
            .field("templates", &self.templates)
            .field("assets", &self.assets.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Theme {
    /// Reads every file, validates `theme.toml` and the layout (`templates/index.html`
    /// required), and compiles all templates. Template errors are collected, each with
    /// its file and line.
    pub fn load(source: &ThemeSource) -> Result<Theme, ThemeError> {
        let files = match source {
            ThemeSource::Embedded(name) => read_embedded(name)?,
            ThemeSource::Dir(dir) => read_dir(dir)?,
        };
        Self::from_files(files)
    }

    /// Names of the embedded starter themes.
    pub fn embedded_names() -> Vec<String> {
        let names: BTreeSet<String> = Embedded::iter()
            .filter_map(|p| p.split_once('/').map(|(n, _)| n.to_owned()))
            .collect();
        names.into_iter().collect()
    }

    fn from_files(files: BTreeMap<String, Cow<'static, [u8]>>) -> Result<Theme, ThemeError> {
        let mut problems = Vec::new();
        let total: u64 = files.values().map(|d| d.len() as u64).sum();
        if files.len() > MAX_FILES {
            problems.push(format!("more than {MAX_FILES} files"));
        }
        if total > MAX_TOTAL_BYTES {
            problems.push(format!("files exceed {MAX_TOTAL_BYTES} bytes in total"));
        }
        for path in files.keys() {
            if !valid_path(path) {
                problems.push(format!("bad file path {path:?}"));
            }
        }
        if !problems.is_empty() {
            return Err(ThemeError::Invalid(problems));
        }
        let manifest = match files.get(MANIFEST) {
            None => {
                problems.push(format!("{MANIFEST} is missing"));
                None
            }
            Some(raw) => match std::str::from_utf8(raw) {
                Err(_) => {
                    problems.push(format!("{MANIFEST} is not UTF-8"));
                    None
                }
                Ok(src) => manifest::parse(src).map_err(|p| problems.extend(p)).ok(),
            },
        };
        let mut sources = BTreeMap::new();
        let mut assets = BTreeMap::new();
        for (path, data) in &files {
            if let Some(name) = path.strip_prefix(TEMPLATES) {
                match String::from_utf8(data.to_vec()) {
                    Ok(src) => {
                        sources.insert(name.to_owned(), src);
                    }
                    Err(_) => problems.push(format!("{path} is not UTF-8")),
                }
            } else if let Some(name) = path.strip_prefix(ASSETS) {
                assets.insert(name.to_owned(), data.clone());
            }
        }
        if !sources.contains_key(INDEX) {
            problems.push(format!("{TEMPLATES}{INDEX} is required"));
        }
        let Some(manifest) = manifest.filter(|_| problems.is_empty()) else {
            return Err(ThemeError::Invalid(problems));
        };
        let templates: BTreeSet<String> = sources.keys().cloned().collect();
        let env = render::build_env(sources).map_err(ThemeError::Templates)?;
        Ok(Theme {
            manifest,
            hash: content_hash(&files),
            templates,
            assets,
            env,
        })
    }

    pub fn manifest(&self) -> &ThemeManifest {
        &self.manifest
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// First 16 hex digits of a SHA-256 over all files (sorted by path); part of every
    /// asset URL, so a changed theme gets fresh URLs.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Whether `templates/<name>` exists (`name` like `single-post.html`); the `has`
    /// argument of [`resolve_template`](crate::resolve_template).
    pub fn has_template(&self, name: &str) -> bool {
        self.templates.contains(name)
    }

    /// Template names, relative to `templates/`.
    pub fn templates(&self) -> impl Iterator<Item = &str> {
        self.templates.iter().map(String::as_str)
    }

    /// The asset at `path` (relative to `assets/`, as it follows
    /// `/_theme/<hash>/` in a URL) and its MIME type.
    ///
    /// `path` is matched exactly against the asset names; it is never decoded or
    /// normalised (the router decoded the URL once), so `..`, `%2e`, absolute paths and
    /// the like simply miss. Only files under `assets/` are served: templates and
    /// `theme.toml` are not reachable.
    pub fn asset(&self, path: &str) -> Option<(&[u8], &'static str)> {
        self.assets.get(path).map(|d| (d.as_ref(), mime_for(path)))
    }

    /// Public URL of an asset: `/_theme/<hash>/<path>`, each segment percent-encoded.
    pub fn asset_url(&self, path: &str) -> String {
        asset_url(&self.hash, path)
    }
}

pub(crate) fn asset_url(hash: &str, path: &str) -> String {
    let mut url = format!("/_theme/{hash}");
    for seg in path.trim_start_matches('/').split('/') {
        url.push('/');
        url.extend(percent_encoding::utf8_percent_encode(seg, SEGMENT));
    }
    url
}

/// Bytes escaped in an asset URL path segment: controls, space and the characters
/// that would end or restructure the path.
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'\\')
    .add(b'\'');

fn content_hash(files: &BTreeMap<String, Cow<'static, [u8]>>) -> String {
    let mut h = Sha256::new();
    for (path, data) in files {
        h.update((path.len() as u64).to_le_bytes());
        h.update(path.as_bytes());
        h.update((data.len() as u64).to_le_bytes());
        h.update(data);
    }
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Relative, `/`-separated, no empty, `.` or `..` segments, no backslash or control
/// characters.
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}

fn is_hidden(path: &str) -> bool {
    path.split('/').any(|s| s.starts_with('.'))
}

fn read_embedded(name: &str) -> Result<BTreeMap<String, Cow<'static, [u8]>>, ThemeError> {
    let prefix = format!("{name}/");
    let mut files = BTreeMap::new();
    if name.contains('/') || !valid_path(name) {
        return Err(ThemeError::NotFound(name.to_owned()));
    }
    for path in Embedded::iter() {
        let Some(rel) = path.strip_prefix(&prefix) else {
            continue;
        };
        if is_hidden(rel) {
            continue;
        }
        if let Some(file) = Embedded::get(&path) {
            files.insert(rel.to_owned(), file.data);
        }
        if files.len() > MAX_FILES {
            break;
        }
    }
    if files.is_empty() {
        return Err(ThemeError::NotFound(name.to_owned()));
    }
    Ok(files)
}

fn read_dir(root: &Path) -> Result<BTreeMap<String, Cow<'static, [u8]>>, ThemeError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ThemeError::Io { path, source }
    };
    let meta = fs::symlink_metadata(root).map_err(io(root))?;
    if !meta.is_dir() {
        return Err(ThemeError::Invalid(vec![format!(
            "{} is not a directory",
            root.display()
        )]));
    }
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    let mut entries = 0usize;
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
    while let Some((dir, rel, depth)) = stack.pop() {
        let listing = fs::read_dir(&dir).map_err(io(&dir))?;
        for entry in listing {
            let entry = entry.map_err(io(&dir))?;
            entries += 1;
            if entries > MAX_FILES {
                return Err(ThemeError::Invalid(vec![format!(
                    "more than {MAX_FILES} files"
                )]));
            }
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(ThemeError::Invalid(vec![format!(
                    "{}: file name is not UTF-8",
                    path.display()
                )]));
            };
            if name.starts_with('.') {
                continue;
            }
            let rel_path = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if !valid_path(&rel_path) {
                return Err(ThemeError::Invalid(vec![format!(
                    "bad file path {rel_path:?}"
                )]));
            }
            // symlink_metadata does not follow links, so a link is seen as a link.
            let meta = fs::symlink_metadata(&path).map_err(io(&path))?;
            let kind = meta.file_type();
            if kind.is_symlink() {
                return Err(ThemeError::Invalid(vec![format!(
                    "{rel_path}: symlinks are not allowed"
                )]));
            } else if kind.is_dir() {
                if depth + 1 > MAX_DEPTH {
                    return Err(ThemeError::Invalid(vec![format!(
                        "{rel_path}: nested deeper than {MAX_DEPTH} levels"
                    )]));
                }
                stack.push((path, rel_path, depth + 1));
            } else if kind.is_file() {
                let remaining = MAX_TOTAL_BYTES.saturating_sub(total);
                if meta.len() > remaining {
                    return Err(ThemeError::Invalid(vec![format!(
                        "files exceed {MAX_TOTAL_BYTES} bytes in total"
                    )]));
                }
                let mut data = Vec::new();
                // `take` guards against a file growing between stat and read.
                fs::File::open(&path)
                    .and_then(|f| f.take(remaining + 1).read_to_end(&mut data))
                    .map_err(io(&path))?;
                total += data.len() as u64;
                if total > MAX_TOTAL_BYTES {
                    return Err(ThemeError::Invalid(vec![format!(
                        "files exceed {MAX_TOTAL_BYTES} bytes in total"
                    )]));
                }
                files.insert(rel_path, Cow::Owned(data));
            } else {
                return Err(ThemeError::Invalid(vec![format!(
                    "{rel_path}: not a regular file"
                )]));
            }
        }
    }
    Ok(files)
}

/// MIME type by extension; unknown types are `application/octet-stream`.
fn mime_for(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_rules() {
        for ok in ["theme.toml", "templates/a.html", "assets/css/x y.css"] {
            assert!(valid_path(ok), "{ok}");
        }
        for bad in [
            "", "/abs", "a//b", "a/../b", "..", "./a", "a\\b", "a\nb", "a/",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn asset_urls_encode_segments() {
        assert_eq!(
            asset_url("0123456789abcdef", "css/site.css"),
            "/_theme/0123456789abcdef/css/site.css"
        );
        assert_eq!(
            asset_url("h", "/img/a b?#.png"),
            "/_theme/h/img/a%20b%3F%23.png"
        );
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_for("a/b.CSS"), "text/css; charset=utf-8");
        assert_eq!(mime_for("x.woff2"), "font/woff2");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }
}
