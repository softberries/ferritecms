//! Theme bundles (plan ruling P17): a theme's files as one deterministic ustar archive,
//! the artifact a theme is installed as (kind `theme`, mime `application/x-tar`).
//!
//! [`pack`] writes regular-file entries only, sorted by path, with mode `0644`, mtime 0,
//! uid/gid 0 and empty user/group names, so the same files always give the same bytes
//! (and the same artifact hash) whatever the folder's metadata.
//!
//! [`unpack`] parses untrusted bytes in memory; nothing is ever written to disk. It
//! reads the raw entry stream (no PAX or GNU extension processing, so no header can
//! rename, resize or re-own the next entry) and accepts only ustar regular files with
//! a valid relative path, refusing everything else by name: symlinks, hard links,
//! devices, FIFOs, directories, extension headers, absolute paths, `..`/`.`/empty
//! segments, backslashes, duplicate paths, paths over 255 bytes and nesting deeper
//! than a folder may be. The folder limits apply while streaming: an entry whose header
//! claims more than what is left of the 20 MiB budget is refused before its data is
//! read, so a tar bomb costs at most its own (capped) size. The archive must end with
//! exactly the two zero blocks `pack` writes and nothing after them, so a bundle has one
//! canonical form and no bytes ride along unparsed.

use std::io::Read;

use bytes::Bytes;
use tar::{Archive, Builder, EntryType, Header};

use crate::{
    ThemeError, ThemeSource,
    package::{Files, MAX_DEPTH, MAX_FILES, MAX_TOTAL_BYTES, is_hidden, read_embedded, valid_path},
};

/// Longest entry path in a bundle (what a ustar header holds, prefix + name).
pub(crate) const MAX_BUNDLE_PATH: usize = 255;

/// Largest bundle accepted: the folder limits plus one header (512 bytes) and at most
/// 511 bytes of padding per file, plus the two end-of-archive blocks.
pub const MAX_BUNDLE_BYTES: u64 = MAX_TOTAL_BYTES + MAX_FILES as u64 * 1024 + 1024;

/// Packs the theme at `source` into a deterministic ustar bundle (see the module docs).
///
/// The files are read and validated as [`Theme::load`](crate::Theme::load) reads them
/// (hidden files left out); the folder limits apply (2000 files, 20 MiB, depth 16) and
/// every path must fit a ustar header (at most 100 bytes after the last `/` that splits
/// it, 255 in all). The theme is not compiled: load the bundle to check it.
pub fn pack(source: &ThemeSource) -> Result<Vec<u8>, ThemeError> {
    pack_files(&source.files()?)
}

/// [`pack`] of the embedded starter theme `name`, for names known only at runtime: the
/// same bytes `pack(&ThemeSource::Embedded(name))` gives. An unknown name is
/// [`ThemeError::NotFound`].
pub fn pack_embedded(name: &str) -> Result<Vec<u8>, ThemeError> {
    pack_files(&read_embedded(name)?)
}

fn pack_files(files: &Files) -> Result<Vec<u8>, ThemeError> {
    let problems = limit_problems(files);
    if !problems.is_empty() {
        return Err(ThemeError::Invalid(problems));
    }
    let size: usize = files
        .values()
        .map(|d| 512 + d.len().div_ceil(512) * 512)
        .sum();
    let mut builder = Builder::new(Vec::with_capacity(size + 1024));
    for (path, data) in files {
        let mut header = Header::new_ustar();
        header
            .set_path(path)
            .map_err(|e| invalid(path, &format!("does not fit a ustar header: {e}")))?;
        header.set_entry_type(EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header
            .set_username("")
            .and_then(|()| header.set_groupname(""))
            .map_err(|e| invalid(path, &e.to_string()))?;
        header.set_cksum();
        builder
            .append(&header, &data[..])
            .map_err(|e| invalid(path, &e.to_string()))?;
    }
    builder
        .into_inner()
        .map_err(|e| ThemeError::Invalid(vec![format!("finishing the bundle: {e}")]))
}

/// The limit violations of `files` as a bundle: count, total size, depth, path rules.
fn limit_problems(files: &Files) -> Vec<String> {
    let mut problems = Vec::new();
    if files.len() > MAX_FILES {
        problems.push(format!("more than {MAX_FILES} files"));
    }
    let total: u64 = files.values().map(|d| d.len() as u64).sum();
    if total > MAX_TOTAL_BYTES {
        problems.push(format!("files exceed {MAX_TOTAL_BYTES} bytes in total"));
    }
    for path in files.keys() {
        if let Some(problem) = path_problem(path) {
            problems.push(format!("{path:?}: {problem}"));
        }
    }
    problems
}

/// Why `path` cannot be a bundle entry, if it cannot.
fn path_problem(path: &str) -> Option<String> {
    if path.len() > MAX_BUNDLE_PATH {
        Some(format!("path longer than {MAX_BUNDLE_PATH} bytes"))
    } else if !valid_path(path) {
        Some("bad path (absolute, empty, `.` or `..` segment, backslash or control)".into())
    } else if path.matches('/').count() > MAX_DEPTH {
        Some(format!("nested deeper than {MAX_DEPTH} levels"))
    } else {
        None
    }
}

/// Size of a tar block; an archive ends with two zero blocks.
const BLOCK: u64 = 512;

/// Unpacks a bundle in memory (see the module docs). Hidden files are skipped as in a
/// folder, but count against the limits.
pub(crate) fn unpack(bundle: &Bytes) -> Result<Files, ThemeError> {
    if bundle.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(ThemeError::Invalid(vec![format!(
            "bundle is {} bytes, more than {MAX_BUNDLE_BYTES}",
            bundle.len()
        )]));
    }
    let bad = |e: std::io::Error| ThemeError::Invalid(vec![format!("malformed bundle: {e}")]);
    let mut archive = Archive::new(&bundle[..]);
    let entries = archive.entries().map_err(bad)?.raw(true);
    let mut files = Files::new();
    let mut total = 0u64;
    // Where the last entry's padded data ends: the end-of-archive blocks start here.
    let mut end = 0u64;
    for (count, entry) in entries.enumerate() {
        let mut entry = entry.map_err(bad)?;
        if count >= MAX_FILES {
            return Err(ThemeError::Invalid(vec![format!(
                "more than {MAX_FILES} files"
            )]));
        }
        let raw = entry.path_bytes().into_owned();
        let Ok(path) = String::from_utf8(raw) else {
            let lossy = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
            return Err(invalid(&lossy, "path is not UTF-8"));
        };
        let header = entry.header();
        if header.as_ustar().is_none() {
            return Err(invalid(&path, "not a ustar header"));
        }
        let kind = header.entry_type();
        if !kind.is_file() {
            return Err(invalid(&path, &kind_problem(kind)));
        }
        if let Some(problem) = path_problem(&path) {
            return Err(invalid(&path, &problem));
        }
        // Checked against the header before any data is read: a bomb stops here.
        let size = entry.size();
        if size > MAX_TOTAL_BYTES - total {
            return Err(invalid(
                &path,
                &format!("files exceed {MAX_TOTAL_BYTES} bytes in total"),
            ));
        }
        total += size;
        end = entry.raw_file_position() + size.div_ceil(BLOCK) * BLOCK;
        let mut data = Vec::new();
        (&mut entry)
            .take(size)
            .read_to_end(&mut data)
            .map_err(|e| invalid(&path, &e.to_string()))?;
        if data.len() as u64 != size {
            return Err(invalid(&path, "truncated entry"));
        }
        if is_hidden(&path) {
            continue;
        }
        if files.contains_key(&path) {
            return Err(invalid(&path, "duplicate path"));
        }
        files.insert(path, Bytes::from(data));
    }
    let tail = usize::try_from(end)
        .ok()
        .and_then(|end| bundle.get(end..))
        .unwrap_or_default();
    if tail.len() as u64 != 2 * BLOCK || tail.iter().any(|&b| b != 0) {
        return Err(ThemeError::Invalid(vec![
            "malformed bundle: it must end with exactly two zero blocks and nothing after them"
                .into(),
        ]));
    }
    Ok(files)
}

fn kind_problem(kind: EntryType) -> String {
    if kind.is_symlink() {
        "symlinks are not allowed".into()
    } else if kind.is_hard_link() {
        "hard links are not allowed".into()
    } else if kind.is_pax_local_extensions()
        || kind.is_pax_global_extensions()
        || kind.is_gnu_longname()
        || kind.is_gnu_longlink()
        || kind.is_gnu_sparse()
    {
        "PAX/GNU extension headers are not allowed".into()
    } else {
        format!("not a regular file (type {:?})", char::from(kind.as_byte()))
    }
}

fn invalid(path: &str, problem: &str) -> ThemeError {
    ThemeError::Invalid(vec![format!("bundle entry {path:?}: {problem}")])
}
