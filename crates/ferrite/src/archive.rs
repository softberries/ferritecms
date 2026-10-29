//! The `tar.zst` archive shared by `ferrite export` and `ferrite backup` (M4-1 P7).
//!
//! A tar stream compressed with zstd (level 3) whose first entry is `manifest.json`. An
//! export holds `tables/<name>.ndjson` in catalogue order and, with media,
//! `artifacts/<ab>/<hash>`; a backup holds the files its manifest lists plus the blobs.
//! Every entry is a regular file with mode 0600.
//!
//! The reader checks the manifest first (format and kind), then every entry before its
//! bytes are used: the path must be one the manifest announces (never absolute, never
//! with `..`), tables must come in manifest order, and each entry's sha256 (for a blob:
//! its own name) is compared when the caller [`finish`](ArchiveEntry::finish)es it. When
//! the stream ends, every table and file of the manifest must have been seen and the blob
//! count must match, so a truncated stream is refused even at an entry boundary.

use std::collections::HashSet;
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

use anyhow::{Context, bail};
use ferrite_domain::{ArtifactHash, to_hex};
use ferrite_store::catalogue::{self, TableSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The archive format this binary writes and the newest it reads.
pub const FORMAT: u32 = 1;
/// The first entry of every archive.
pub const MANIFEST: &str = "manifest.json";
/// The zstd level (P7).
const LEVEL: i32 = 3;
/// The largest manifest read.
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;

/// An archive this binary cannot use because a newer one wrote it (a newer format or
/// schema). Commands map it to exit code 3.
#[derive(Debug)]
pub struct Newer(pub String);

impl fmt::Display for Newer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Newer {}

/// What an archive holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Export,
    Backup,
}

/// One table file of an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableEntry {
    pub name: String,
    pub rows: u64,
    pub sha256: String,
}

/// One plain file of a backup (`ferrite.db`, `ferrite.toml`, `secret.key`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The blobs under `artifacts/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactsInfo {
    pub count: u64,
    pub bytes: u64,
}

/// `manifest.json` (P7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub kind: Kind,
    pub ferrite_version: String,
    /// The highest applied migration of the source.
    pub schema_version: i64,
    /// `sqlite` or `postgres`.
    pub source_backend: String,
    pub site_id: Option<String>,
    /// RFC 3339 UTC.
    pub created_at: String,
    /// Export only, in catalogue order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tables: Option<Vec<TableEntry>>,
    /// Backup only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileEntry>>,
    /// `null` when the archive holds no blobs.
    pub artifacts: Option<ArtifactsInfo>,
    /// Export only: artifacts that have a row but whose blob was missing from the source's
    /// media store, so the archive lacks them (an additive field, absent when empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

impl Manifest {
    /// Parses a manifest, refusing a newer format with [`Newer`] before anything else.
    fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).context("manifest.json is not valid JSON")?;
        let format = value
            .get("format")
            .and_then(serde_json::Value::as_u64)
            .context("manifest.json has no format")?;
        if format > u64::from(FORMAT) {
            return Err(Newer(format!(
                "the archive has format {format}; this binary reads format {FORMAT} (use a newer ferrite)"
            ))
            .into());
        }
        let manifest: Manifest =
            serde_json::from_value(value).context("manifest.json is not a valid manifest")?;
        manifest.check()?;
        Ok(manifest)
    }

    /// An export lists tables and no files, a backup files and no tables; the table list
    /// names exported catalogue tables in catalogue order, once each; file paths are plain
    /// names.
    fn check(&self) -> anyhow::Result<()> {
        match (self.kind, &self.tables, &self.files) {
            (Kind::Export, Some(_), None) | (Kind::Backup, None, Some(_)) => {}
            (Kind::Export, ..) => bail!("manifest.json of an export must list tables and no files"),
            (Kind::Backup, ..) => bail!("manifest.json of a backup must list files and no tables"),
        }
        if self.kind == Kind::Backup && !self.missing.is_empty() {
            bail!("manifest.json of a backup lists missing artifacts");
        }
        let mut last = None;
        for t in self.tables.iter().flatten() {
            let pos = catalogue::TABLES
                .iter()
                .position(|c| c.name == t.name && c.is_exported())
                .with_context(|| format!("manifest.json lists an unknown table {:?}", t.name))?;
            if last.is_some_and(|l| pos <= l) {
                bail!("manifest.json lists table {} out of order", t.name);
            }
            last = Some(pos);
        }
        for f in self.files.iter().flatten() {
            check_path(&f.path)?;
            if f.path.contains('/') || f.path == MANIFEST {
                bail!("manifest.json lists an invalid file path {:?}", f.path);
            }
        }
        Ok(())
    }
}

/// The path of table `name`'s file.
pub fn table_path(name: &str) -> String {
    format!("tables/{name}.ndjson")
}

/// The path of a blob.
pub fn artifact_path(hash: &ArtifactHash) -> String {
    let h = hash.as_str();
    format!("artifacts/{}/{h}", &h[..2])
}

/// Refuses absolute paths, `..`, `.`, empty components, backslashes and NUL.
fn check_path(path: &str) -> anyhow::Result<()> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..");
    if bad {
        bail!("refusing archive entry with path {path:?}");
    }
    Ok(())
}

/// Writes an archive: the manifest first ([`ArchiveWriter::new`]), then the entries.
pub struct ArchiveWriter<W: Write> {
    tar: tar::Builder<zstd::stream::write::Encoder<'static, W>>,
    mtime: u64,
}

impl<W: Write> ArchiveWriter<W> {
    /// Starts an archive on `out` and writes `manifest` as its first entry.
    pub fn new(out: W, manifest: &Manifest) -> anyhow::Result<Self> {
        let encoder = zstd::stream::write::Encoder::new(out, LEVEL)?;
        let tar = tar::Builder::new(encoder);
        let mtime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut this = Self { tar, mtime };
        let json = serde_json::to_vec_pretty(manifest)?;
        this.append_stream(MANIFEST, json.len() as u64, json.as_slice())?;
        Ok(this)
    }

    /// Appends the file at `src` as `path`.
    pub fn append_file(&mut self, path: &str, src: &Path) -> anyhow::Result<()> {
        let file =
            std::fs::File::open(src).with_context(|| format!("cannot open {}", src.display()))?;
        let size = file.metadata()?.len();
        self.append_stream(path, size, file)
    }

    /// Appends `size` bytes read from `data` as `path`; fails if `data` holds fewer.
    pub fn append_stream(&mut self, path: &str, size: u64, data: impl Read) -> anyhow::Result<()> {
        check_path(path)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(size);
        header.set_mode(0o600);
        header.set_mtime(self.mtime);
        header.set_entry_type(tar::EntryType::Regular);
        let exact = Exact {
            inner: data.take(size),
            left: size,
        };
        self.tar
            .append_data(&mut header, path, exact)
            .with_context(|| format!("cannot write {path} to the archive"))
    }

    /// Ends the tar stream and the zstd frame; returns the output.
    pub fn finish(self) -> anyhow::Result<W> {
        let encoder = self.tar.into_inner()?;
        Ok(encoder.finish()?)
    }
}

/// A reader that must yield exactly `left` more bytes.
struct Exact<R> {
    inner: io::Take<R>,
    left: u64,
}

impl<R: Read> Read for Exact<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n == 0 && self.left > 0 && !buf.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("source ended {} bytes early", self.left),
            ));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

type Zst<R> = zstd::stream::read::Decoder<'static, BufReader<R>>;

/// Reads an archive: [`entries`](Self::entries) gives the checked manifest and an entry
/// iterator.
pub struct ArchiveReader<R: Read> {
    archive: tar::Archive<Zst<R>>,
}

impl<R: Read> ArchiveReader<R> {
    pub fn new(input: R) -> anyhow::Result<Self> {
        let decoder =
            zstd::stream::read::Decoder::new(input).context("cannot start reading the archive")?;
        Ok(Self {
            archive: tar::Archive::new(decoder),
        })
    }

    /// Reads and checks `manifest.json` (it must be the first entry), then returns the
    /// iterator over the other entries. A newer format fails with [`Newer`].
    pub fn entries(&mut self) -> anyhow::Result<Entries<'_, R>> {
        let mut inner = self.archive.entries().context("cannot read the archive")?;
        let mut first = inner
            .next()
            .context("the archive is empty")?
            .context("cannot read the archive")?;
        let path = String::from_utf8_lossy(&first.path_bytes()).into_owned();
        if path != MANIFEST || first.header().entry_type() != tar::EntryType::Regular {
            bail!("the archive does not start with {MANIFEST} (found {path:?})");
        }
        let mut json = Vec::new();
        (&mut first)
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut json)
            .context("cannot read manifest.json")?;
        if json.len() as u64 > MAX_MANIFEST {
            bail!("manifest.json is larger than {MAX_MANIFEST} bytes");
        }
        drop(first);
        let manifest = Manifest::parse(&json)?;
        Ok(Entries {
            inner,
            manifest,
            next_table: 0,
            files: HashSet::new(),
            artifacts: HashSet::new(),
            artifact_bytes: 0,
        })
    }
}

/// What an entry is.
#[derive(Debug, Clone)]
pub enum EntryKind {
    /// A table file, with the row count the manifest gives.
    Table { spec: &'static TableSpec, rows: u64 },
    /// A file of a backup's manifest.
    File(String),
    /// A blob.
    Artifact(ArtifactHash),
}

/// The entries after the manifest.
pub struct Entries<'a, R: Read> {
    inner: tar::Entries<'a, Zst<R>>,
    manifest: Manifest,
    next_table: usize,
    files: HashSet<String>,
    artifacts: HashSet<ArtifactHash>,
    artifact_bytes: u64,
}

impl<'a, R: Read> Entries<'a, R> {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The next entry, checked against the manifest; `None` at the end, once the archive
    /// is known to be complete.
    pub fn next_entry(&mut self) -> anyhow::Result<Option<ArchiveEntry<'a, R>>> {
        let Some(entry) = self.inner.next() else {
            self.check_complete()?;
            return Ok(None);
        };
        let entry = entry.context("cannot read the archive")?;
        let raw = entry.path_bytes();
        let path = std::str::from_utf8(&raw)
            .map_err(|_| anyhow::anyhow!("refusing archive entry with a non-UTF-8 path"))?
            .to_owned();
        check_path(&path)?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            bail!("refusing archive entry {path}: not a regular file");
        }
        let size = entry.header().size()?;
        let (kind, sha256) = self.classify(&path, size)?;
        Ok(Some(ArchiveEntry {
            inner: entry,
            path,
            kind,
            sha256,
            hasher: Sha256::new(),
        }))
    }

    fn classify(&mut self, path: &str, size: u64) -> anyhow::Result<(EntryKind, String)> {
        if let Some(rest) = path.strip_prefix("tables/") {
            let tables = self.manifest.tables.as_deref().unwrap_or_default();
            let name = rest.strip_suffix(".ndjson").unwrap_or_default();
            let Some(i) = tables.iter().position(|t| t.name == name) else {
                bail!("refusing archive entry {path}: not in the manifest");
            };
            if i != self.next_table {
                bail!("refusing archive entry {path}: tables out of manifest order");
            }
            self.next_table += 1;
            let spec = catalogue::table(name).context("unknown table")?;
            let t = &tables[i];
            return Ok((EntryKind::Table { spec, rows: t.rows }, t.sha256.clone()));
        }
        if let Some(rest) = path.strip_prefix("artifacts/") {
            if self.manifest.artifacts.is_none() {
                bail!("refusing archive entry {path}: the manifest lists no artifacts");
            }
            let hash = rest
                .split_once('/')
                .and_then(|(ab, h)| (h.get(..2) == Some(ab)).then_some(h))
                .and_then(|h| ArtifactHash::parse(h).ok())
                .with_context(|| format!("refusing archive entry {path}: not an artifact path"))?;
            if !self.artifacts.insert(hash.clone()) {
                bail!("refusing archive entry {path}: listed twice");
            }
            self.artifact_bytes = self.artifact_bytes.saturating_add(size);
            if let Some(a) = self.manifest.artifacts
                && (self.artifact_bytes > a.bytes || self.artifacts.len() as u64 > a.count)
            {
                bail!(
                    "refusing archive entry {path}: more artifacts or artifact bytes than the \
                     manifest declares ({} artifacts, {} bytes)",
                    a.count,
                    a.bytes
                );
            }
            let sha = hash.as_str().to_owned();
            return Ok((EntryKind::Artifact(hash), sha));
        }
        let Some(file) = self
            .manifest
            .files
            .iter()
            .flatten()
            .find(|f| f.path == path)
        else {
            bail!("refusing archive entry {path}: not in the manifest");
        };
        if !self.files.insert(path.to_owned()) || size != file.bytes {
            bail!("refusing archive entry {path}: duplicate or wrong size");
        }
        Ok((EntryKind::File(path.to_owned()), file.sha256.clone()))
    }

    fn check_complete(&self) -> anyhow::Result<()> {
        let tables = self.manifest.tables.as_deref().unwrap_or_default();
        if let Some(t) = tables.get(self.next_table) {
            bail!("the archive is incomplete: table {} is missing", t.name);
        }
        if let Some(f) = self
            .manifest
            .files
            .iter()
            .flatten()
            .find(|f| !self.files.contains(&f.path))
        {
            bail!("the archive is incomplete: {} is missing", f.path);
        }
        if let Some(a) = self.manifest.artifacts {
            let got = self.artifacts.len() as u64;
            if got != a.count || self.artifact_bytes != a.bytes {
                bail!(
                    "the archive is incomplete: {got} artifacts ({} bytes), the manifest lists {} ({} bytes)",
                    self.artifact_bytes,
                    a.count,
                    a.bytes
                );
            }
        }
        Ok(())
    }
}

/// One entry: read it (the bytes are hashed on the way), then [`finish`](Self::finish)
/// it to compare the sha256 with the manifest.
pub struct ArchiveEntry<'a, R: Read> {
    inner: tar::Entry<'a, Zst<R>>,
    pub path: String,
    pub kind: EntryKind,
    sha256: String,
    hasher: Sha256,
}

impl<R: Read> ArchiveEntry<'_, R> {
    /// The entry's size in bytes.
    pub fn size(&self) -> u64 {
        self.inner.size()
    }

    /// Reads what is left and checks the sha256; returns the size.
    pub fn finish(mut self) -> anyhow::Result<u64> {
        io::copy(&mut self, &mut io::sink())
            .with_context(|| format!("cannot read {} from the archive", self.path))?;
        let got = to_hex(&self.hasher.finalize());
        if got != self.sha256 {
            bail!(
                "{}: sha256 mismatch (the manifest says {}, the content is {got}); the archive is damaged or was changed",
                self.path,
                self.sha256
            );
        }
        Ok(self.inner.size())
    }
}

impl<R: Read> Read for ArchiveEntry<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256_hex(data: &[u8]) -> String {
        to_hex(&Sha256::digest(data))
    }

    fn manifest(tables: Vec<TableEntry>, artifacts: Option<ArtifactsInfo>) -> Manifest {
        Manifest {
            format: FORMAT,
            kind: Kind::Export,
            ferrite_version: "0.1.0".into(),
            schema_version: 11,
            source_backend: "sqlite".into(),
            site_id: None,
            created_at: "2026-10-29T00:00:00.000000Z".into(),
            tables: Some(tables),
            files: None,
            artifacts,
            missing: Vec::new(),
        }
    }

    fn table(name: &str, data: &[u8], rows: u64) -> TableEntry {
        TableEntry {
            name: name.into(),
            rows,
            sha256: sha256_hex(data),
        }
    }

    fn read_all(bytes: &[u8]) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let mut reader = ArchiveReader::new(bytes)?;
        let mut entries = reader.entries()?;
        let mut out = Vec::new();
        while let Some(mut e) = entries.next_entry()? {
            let mut data = Vec::new();
            e.read_to_end(&mut data)?;
            let path = e.path.clone();
            e.finish()?;
            out.push((path, data));
        }
        Ok(out)
    }

    #[test]
    fn round_trip_with_blob() {
        let users = b"{\"id\":1}\n";
        let blob = b"hello";
        let hash = ArtifactHash::parse(&sha256_hex(blob)).unwrap();
        let m = manifest(
            vec![table("users", users, 1)],
            Some(ArtifactsInfo { count: 1, bytes: 5 }),
        );
        let mut w = ArchiveWriter::new(Vec::new(), &m).unwrap();
        w.append_stream("tables/users.ndjson", users.len() as u64, &users[..])
            .unwrap();
        w.append_stream(&artifact_path(&hash), 5, &blob[..])
            .unwrap();
        let bytes = w.finish().unwrap();
        let got = read_all(&bytes).unwrap();
        assert_eq!(got[0], ("tables/users.ndjson".into(), users.to_vec()));
        assert_eq!(got[1].1, blob);
    }

    #[test]
    fn short_source_is_an_error() {
        let m = manifest(vec![], None);
        let mut w = ArchiveWriter::new(Vec::new(), &m).unwrap();
        assert!(w.append_stream("x", 10, &b"abc"[..]).is_err());
    }

    #[test]
    fn refusals() {
        let users = b"{}\n";
        // Blob whose content does not match its name.
        let hash = ArtifactHash::parse(&sha256_hex(b"other")).unwrap();
        let m = manifest(
            vec![table("users", users, 1)],
            Some(ArtifactsInfo { count: 1, bytes: 5 }),
        );
        let mut w = ArchiveWriter::new(Vec::new(), &m).unwrap();
        w.append_stream("tables/users.ndjson", 3, &users[..])
            .unwrap();
        w.append_stream(&artifact_path(&hash), 5, &b"hello"[..])
            .unwrap();
        let e = read_all(&w.finish().unwrap()).unwrap_err();
        assert!(format!("{e:#}").contains("sha256"), "{e:#}");

        // A table the manifest lists but the archive lacks.
        let m = manifest(vec![table("users", users, 1)], None);
        let w = ArchiveWriter::new(Vec::new(), &m).unwrap();
        let e = read_all(&w.finish().unwrap()).unwrap_err();
        assert!(format!("{e:#}").contains("incomplete"), "{e:#}");

        // Tables out of catalogue order, or skipped tables, in the manifest.
        for tables in [
            vec![table("users", b"", 0), table("content_types", b"", 0)],
            vec![table("sessions", b"", 0)],
        ] {
            let w = ArchiveWriter::new(Vec::new(), &manifest(tables, None)).unwrap();
            assert!(read_all(&w.finish().unwrap()).is_err());
        }
    }

    #[test]
    fn newer_format_is_newer() {
        let mut m = manifest(vec![], None);
        m.format = FORMAT + 1;
        let w = ArchiveWriter::new(Vec::new(), &m).unwrap();
        let e = read_all(&w.finish().unwrap()).unwrap_err();
        assert!(e.downcast_ref::<Newer>().is_some(), "{e:#}");
    }

    #[test]
    fn paths() {
        for bad in ["", "/a", "../a", "a/../b", "a//b", "./a", "a\\b"] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
        check_path("tables/users.ndjson").unwrap();
    }

    /// A tar.zst of raw entries `(path, type, data)`, the manifest first; no checks.
    fn raw(manifest: &Manifest, entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap());
        let json = serde_json::to_vec(manifest).unwrap();
        let mut all = vec![(MANIFEST, tar::EntryType::Regular, json.as_slice())];
        all.extend(entries.iter().copied());
        for (path, kind, data) in all {
            let mut h = tar::Header::new_gnu();
            h.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_size(data.len() as u64);
            h.set_entry_type(kind);
            if kind != tar::EntryType::Regular {
                h.set_link_name("tables/users.ndjson").unwrap();
            }
            h.set_cksum();
            b.append(&h, data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn refused(bytes: &[u8], needle: &str) {
        let e = read_all(bytes).unwrap_err();
        assert!(format!("{e:#}").contains(needle), "{needle}: {e:#}");
    }

    #[test]
    fn hostile_entries_are_refused() {
        let users: &[u8] = b"{}\n";
        let m = manifest(vec![table("users", users, 1)], None);
        let reg = tar::EntryType::Regular;
        for kind in [tar::EntryType::Symlink, tar::EntryType::Link] {
            refused(
                &raw(&m, &[("tables/users.ndjson", kind, b"")]),
                "not a regular file",
            );
        }
        refused(
            &raw(
                &m,
                &[
                    ("tables/users.ndjson", reg, users),
                    ("tables/users.ndjson", reg, users),
                ],
            ),
            "out of manifest order",
        );
        refused(
            &raw(&m, &[("tables/unknown.ndjson", reg, b"")]),
            "not in the manifest",
        );
        refused(&raw(&m, &[("secret.key", reg, b"")]), "not in the manifest");
        // A blob larger than the manifest declares is refused before it is read.
        let blob: &[u8] = b"hello";
        let hash = ArtifactHash::parse(&sha256_hex(blob)).unwrap();
        let path = artifact_path(&hash);
        let m2 = manifest(
            vec![table("users", users, 1)],
            Some(ArtifactsInfo { count: 1, bytes: 4 }),
        );
        refused(
            &raw(
                &m2,
                &[("tables/users.ndjson", reg, users), (&path, reg, blob)],
            ),
            "artifact bytes than the manifest declares",
        );
    }

    #[test]
    fn oversized_manifest_is_refused() {
        let mut b = tar::Builder::new(zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap());
        let mut h = tar::Header::new_gnu();
        h.set_size(MAX_MANIFEST + 1);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, MANIFEST, io::repeat(b' ').take(MAX_MANIFEST + 1))
            .unwrap();
        let bytes = b.into_inner().unwrap().finish().unwrap();
        refused(&bytes, "larger than");
    }

    #[test]
    fn kind_and_lists_must_match() {
        let mut m = manifest(vec![], None);
        m.files = Some(vec![]);
        assert!(m.check().is_err());
        m.tables = None;
        assert!(m.check().is_err(), "an export without tables");
        m.kind = Kind::Backup;
        m.check().unwrap();
        m.missing = vec!["x".into()];
        assert!(m.check().is_err());
        m.missing.clear();
        m.tables = Some(vec![]);
        assert!(m.check().is_err());
    }
}
