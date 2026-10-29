//! A backup repository (M4-1 P13): a folder or an S3 prefix holding
//!
//! - `ferrite-backup-repo.json`: the marker, written on first use (R11);
//! - `artifacts/<ab>/<hash>`: blobs shared by every snapshot, copied only when missing;
//! - `snapshots/<UTC yyyymmddThhmmssZ>/{manifest.json, ferrite.db, ferrite.toml, secret.key,
//!   artifacts.txt}`, where `artifacts.txt` lists the blobs the snapshot needs, one hash per
//!   line (R12; its size, sha256 and line count are in the manifest).
//!
//! A snapshot is complete once its `manifest.json` exists: it is written last, after the
//! other files and the read-back check of `ferrite.db`, and deleted first when a snapshot
//! is pruned. A non-empty folder or prefix without the marker is never used, and pruning
//! only deletes keys of this layout. One site per repository: pruning deletes every blob no
//! kept snapshot lists.

use std::collections::HashSet;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use bytes::Bytes;
use ferrite_domain::{ArtifactHash, Timestamp, to_hex};
use ferrite_media::{MediaError, MediaStore};
use futures::{StreamExt, TryStreamExt};
use object_store::path::Path as Key;
use object_store::{ObjectStore, ObjectStoreExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::target::RepoLocation;
use super::{ARTIFACT_LIST, CONFIG_FILE, DB_FILE};
use crate::archive::{FileEntry, MANIFEST, MAX_MANIFEST, Manifest, Newer};
use crate::keys::SECRET_KEY_FILE;
use crate::media_url::parse_media_url;

/// The snapshot name format (UTC).
pub const SNAPSHOT_FORMAT: &str = "%Y%m%dT%H%M%SZ";
/// The file that marks a folder or prefix as a repository.
pub const MARKER: &str = "ferrite-backup-repo.json";
/// The marker format this binary writes and the newest it reads.
const MARKER_FORMAT: u32 = 1;
/// Every file a snapshot folder may hold; nothing else is ever deleted from one.
const SNAPSHOT_FILES: [&str; 5] = [
    MANIFEST,
    DB_FILE,
    CONFIG_FILE,
    SECRET_KEY_FILE,
    ARTIFACT_LIST,
];
/// Staging objects under `tmp/` younger than this are left alone by pruning.
const TMP_GRACE: chrono::TimeDelta = chrono::TimeDelta::hours(24);
/// One line of an artifact list: 64 hex characters and `\n`.
const LINE: u64 = 65;

/// `ferrite-backup-repo.json`.
#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    format: u32,
    created_at: String,
}

/// An open repository.
pub struct Repo {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    /// The folder of a local repository (its files are written directly, mode 0600).
    local: Option<PathBuf>,
    blobs: MediaStore,
    display: String,
}

/// What [`Repo::copy_blobs`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BlobCopy {
    /// Copied from the site's media store.
    pub written: u64,
    /// Already in the repository.
    pub present: u64,
    /// Bytes copied.
    pub bytes: u64,
    /// The blobs now in the repository (written or present), in the order given.
    pub listed: Vec<ArtifactHash>,
    /// Their total size.
    pub listed_bytes: u64,
    /// Blobs missing from the site's media store (R10).
    pub missing: Vec<ArtifactHash>,
}

/// What [`Repo::prune`] deleted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PruneReport {
    /// Snapshot names deleted, oldest first (incomplete ones older than the kept included).
    pub snapshots: Vec<String>,
    /// Blobs deleted because no kept snapshot lists them.
    pub artifacts: u64,
}

impl Repo {
    /// Opens the repository at `loc`. With `create`, a missing folder is created and an
    /// empty folder or prefix becomes a repository (the marker is written); otherwise the
    /// marker must exist. A non-empty location without the marker is always refused.
    pub async fn open(loc: &RepoLocation, create: bool) -> anyhow::Result<Repo> {
        let (url, local, display) = match loc {
            RepoLocation::Dir(dir) => {
                if !create && !dir.is_dir() {
                    bail!("{} is not a backup repository folder", dir.display());
                }
                if create {
                    std::fs::create_dir_all(dir)
                        .with_context(|| format!("cannot create {}", dir.display()))?;
                }
                (
                    format!("local:{}", dir.display()),
                    Some(dir.clone()),
                    dir.display().to_string(),
                )
            }
            RepoLocation::S3(url) => (url.clone(), None, url.clone()),
        };
        let blobs = parse_media_url(&url, true)?;
        let repo = Repo {
            store: blobs.object_store(),
            prefix: blobs.prefix().to_owned(),
            local,
            blobs,
            display,
        };
        repo.check_marker(create).await?;
        Ok(repo)
    }

    async fn check_marker(&self, create: bool) -> anyhow::Result<()> {
        match self.store.get(&self.key(MARKER)).await {
            Ok(got) => {
                if got.meta.size > MAX_MANIFEST {
                    bail!("{}: {MARKER} is too large", self.display);
                }
                let bytes = got.bytes().await?;
                let marker: Marker = serde_json::from_slice(&bytes)
                    .with_context(|| format!("{}: {MARKER} is not valid", self.display))?;
                if marker.format > MARKER_FORMAT {
                    return Err(Newer(format!(
                        "{} is a repository of format {}; this binary reads format \
                         {MARKER_FORMAT} (use a newer ferrite)",
                        self.display, marker.format
                    ))
                    .into());
                }
                return Ok(());
            }
            Err(object_store::Error::NotFound { .. }) => {}
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", self.display)),
        }
        if !self.is_empty().await? {
            bail!(
                "{} is not empty and is not a ferrite backup repository (it has no {MARKER}); \
                 use an empty or new folder or prefix",
                self.display
            );
        }
        if !create {
            bail!(
                "{} is not a ferrite backup repository (it is empty)",
                self.display
            );
        }
        let marker = Marker {
            format: MARKER_FORMAT,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        self.put_bytes(MARKER, serde_json::to_vec_pretty(&marker)?.into())
            .await
            .with_context(|| format!("cannot create {MARKER} in {}", self.display))
    }

    async fn is_empty(&self) -> anyhow::Result<bool> {
        if let Some(dir) = &self.local {
            return Ok(std::fs::read_dir(dir)
                .with_context(|| format!("cannot read {}", dir.display()))?
                .next()
                .is_none());
        }
        let root = self.prefix.trim_end_matches('/');
        let root = (!root.is_empty()).then(|| Key::from(root));
        match self.store.list(root.as_ref()).next().await {
            None => Ok(true),
            Some(Ok(_)) => Ok(false),
            Some(Err(e)) => Err(e).with_context(|| format!("cannot list {}", self.display)),
        }
    }

    /// The repository's blobs, as a media store.
    pub fn blobs(&self) -> &MediaStore {
        &self.blobs
    }

    /// The repository, for messages.
    pub fn display(&self) -> &str {
        &self.display
    }

    fn key(&self, rel: &str) -> Key {
        Key::from(format!("{}{rel}", self.prefix))
    }

    fn file_key(snapshot: &str, name: &str) -> String {
        format!("snapshots/{snapshot}/{name}")
    }

    /// Every snapshot folder (complete or not), oldest first.
    async fn snapshot_dirs(&self) -> anyhow::Result<Vec<String>> {
        let listed = match self
            .store
            .list_with_delimiter(Some(&self.key("snapshots")))
            .await
        {
            Ok(listed) => listed,
            Err(object_store::Error::NotFound { .. }) => return Ok(Vec::new()),
            Err(e) => {
                return Err(e).with_context(|| format!("cannot list {}", self.display));
            }
        };
        let mut names: Vec<String> = listed
            .common_prefixes
            .iter()
            .filter_map(|p| p.filename().map(str::to_owned))
            .filter(|n| is_snapshot_name(n))
            .collect();
        names.sort();
        Ok(names)
    }

    async fn exists(&self, rel: &str) -> anyhow::Result<bool> {
        match self.store.head(&self.key(rel)).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", self.display)),
        }
    }

    /// The complete snapshots (those with a manifest), oldest first.
    pub async fn snapshots(&self) -> anyhow::Result<Vec<String>> {
        let mut out = Vec::new();
        for name in self.snapshot_dirs().await? {
            if self.exists(&Self::file_key(&name, MANIFEST)).await? {
                out.push(name);
            }
        }
        Ok(out)
    }

    /// Reads and checks a snapshot's manifest (a newer format fails with
    /// `archive::Newer`).
    pub async fn read_manifest(&self, snapshot: &str) -> anyhow::Result<Manifest> {
        let key = self.key(&Self::file_key(snapshot, MANIFEST));
        let got = self
            .store
            .get(&key)
            .await
            .with_context(|| format!("cannot read snapshot {snapshot} of {}", self.display))?;
        if got.meta.size > MAX_MANIFEST {
            bail!("snapshot {snapshot}: manifest.json is larger than {MAX_MANIFEST} bytes");
        }
        let bytes = got.bytes().await?;
        Manifest::parse(&bytes).with_context(|| format!("snapshot {snapshot}"))
    }

    /// Downloads a snapshot's artifact list into `dir` (checked against the manifest's size
    /// and sha256) and returns its path; `None` when the snapshot has no media.
    pub async fn fetch_artifact_list(
        &self,
        snapshot: &str,
        manifest: &Manifest,
        dir: &Path,
    ) -> anyhow::Result<Option<PathBuf>> {
        let Some(entry) = manifest
            .files
            .iter()
            .flatten()
            .find(|f| f.path == ARTIFACT_LIST)
        else {
            if manifest.artifacts.is_some() {
                bail!("snapshot {snapshot} has artifacts but no {ARTIFACT_LIST}");
            }
            return Ok(None);
        };
        let path = dir.join(ARTIFACT_LIST);
        self.fetch_file(snapshot, entry, &path).await?;
        Ok(Some(path))
    }

    /// Copies the blobs in `hashes` that the repository lacks from `media`. A blob missing
    /// from `media` is listed in [`BlobCopy::missing`]; one whose content does not match its
    /// hash fails the call.
    pub async fn copy_blobs(
        &self,
        media: &MediaStore,
        hashes: &[ArtifactHash],
    ) -> anyhow::Result<BlobCopy> {
        let mut out = BlobCopy::default();
        for hash in hashes {
            if let Some(size) = self.blobs.head(hash).await? {
                out.present += 1;
                out.listed_bytes += size;
                out.listed.push(hash.clone());
                continue;
            }
            match media.copy_to(&self.blobs, hash).await {
                Ok(bytes) => {
                    out.written += 1;
                    out.bytes += bytes;
                    out.listed_bytes += bytes;
                    out.listed.push(hash.clone());
                }
                Err(MediaError::NotFound) => out.missing.push(hash.clone()),
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot copy artifact {hash}"));
                }
            }
        }
        Ok(out)
    }

    /// The name for a new snapshot: now, or one second after the newest existing folder
    /// when that is not older (names stay unique and ordered).
    async fn next_name(&self) -> anyhow::Result<String> {
        let now = chrono::Utc::now().naive_utc();
        let mut name = now.format(SNAPSHOT_FORMAT).to_string();
        if let Some(newest) = self.snapshot_dirs().await?.pop()
            && newest >= name
        {
            let t = chrono::NaiveDateTime::parse_from_str(&newest, SNAPSHOT_FORMAT)?;
            name = (t + chrono::TimeDelta::seconds(1))
                .format(SNAPSHOT_FORMAT)
                .to_string();
        }
        Ok(name)
    }

    /// Writes a snapshot: each of `files` (`(name, local path)`, which must match the
    /// manifest's `files`), then checks the stored `ferrite.db` by reading it back, then
    /// writes `manifest`. Returns the snapshot's name. On failure what was written of the
    /// snapshot is deleted (best effort).
    pub async fn write_snapshot(
        &self,
        files: &[(String, PathBuf)],
        manifest: &Manifest,
    ) -> anyhow::Result<String> {
        let entries = manifest.files.as_deref().unwrap_or_default();
        if entries.len() != files.len()
            || entries
                .iter()
                .zip(files)
                .any(|(e, (name, _))| e.path != *name || !SNAPSHOT_FILES.contains(&name.as_str()))
        {
            bail!("the snapshot files do not match the manifest");
        }
        let name = self.next_name().await?;
        let result = async {
            for (file, path) in files {
                self.put_file(&Self::file_key(&name, file), path).await?;
            }
            if let Some(db) = entries.iter().find(|e| e.path == DB_FILE) {
                let got = self.sha256_of(&Self::file_key(&name, &db.path)).await?;
                if got != db.sha256 {
                    bail!(
                        "the stored ferrite.db reads back with sha256 {got}, written {}",
                        db.sha256
                    );
                }
            }
            let json = serde_json::to_vec_pretty(manifest)?;
            self.put_bytes(&Self::file_key(&name, MANIFEST), json.into())
                .await
        }
        .await;
        if let Err(e) = result {
            if let Err(cleanup) = self.delete_snapshot(&name).await {
                tracing::warn!(snapshot = %name, error = %format!("{cleanup:#}"), "cannot remove a failed snapshot");
            }
            return Err(e.context(format!("cannot write snapshot {name} to {}", self.display)));
        }
        Ok(name)
    }

    async fn put_file(&self, rel: &str, src: &Path) -> anyhow::Result<()> {
        if let Some(root) = &self.local {
            let dest = root.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
            let (src, dest) = (src.to_owned(), dest.clone());
            return tokio::task::spawn_blocking(move || {
                let mut from = std::fs::File::open(&src)
                    .with_context(|| format!("cannot open {}", src.display()))?;
                let mut to = crate::init::create_private(&dest)?;
                std::io::copy(&mut from, &mut to)
                    .with_context(|| format!("cannot write {}", dest.display()))?;
                to.sync_all()?;
                anyhow::Ok(())
            })
            .await?;
        }
        let mut file = tokio::fs::File::open(src)
            .await
            .with_context(|| format!("cannot open {}", src.display()))?;
        let mut writer = object_store::buffered::BufWriter::new(self.store.clone(), self.key(rel));
        let copied = async {
            tokio::io::copy(&mut file, &mut writer).await?;
            writer.shutdown().await
        }
        .await;
        if let Err(e) = copied {
            let _ = writer.abort().await;
            return Err(e).with_context(|| format!("cannot upload {rel}"));
        }
        Ok(())
    }

    async fn put_bytes(&self, rel: &str, data: Bytes) -> anyhow::Result<()> {
        if let Some(root) = &self.local {
            let dest = root.join(rel);
            let mut to = crate::init::create_private(&dest)?;
            std::io::Write::write_all(&mut to, &data)
                .with_context(|| format!("cannot write {}", dest.display()))?;
            to.sync_all()?;
            return Ok(());
        }
        self.store
            .put(&self.key(rel), data.into())
            .await
            .with_context(|| format!("cannot upload {rel}"))?;
        Ok(())
    }

    /// The sha256 of a stored file, read back from the repository.
    async fn sha256_of(&self, rel: &str) -> anyhow::Result<String> {
        let got = self
            .store
            .get(&self.key(rel))
            .await
            .with_context(|| format!("cannot read back {rel}"))?;
        let mut stream = got.into_stream();
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            hasher.update(&chunk.with_context(|| format!("cannot read back {rel}"))?);
        }
        Ok(to_hex(&hasher.finalize()))
    }

    /// Downloads a snapshot file to `dest` (created with mode 0600), checking its size and
    /// sha256 against `entry`.
    pub async fn fetch_file(
        &self,
        snapshot: &str,
        entry: &FileEntry,
        dest: &Path,
    ) -> anyhow::Result<()> {
        let rel = Self::file_key(snapshot, &entry.path);
        let got = self
            .store
            .get(&self.key(&rel))
            .await
            .with_context(|| format!("cannot read {rel} from {}", self.display))?;
        if got.meta.size != entry.bytes {
            bail!(
                "{rel}: {} bytes, the manifest says {}",
                got.meta.size,
                entry.bytes
            );
        }
        let mut stream = got.into_stream();
        let mut file = tokio::fs::File::from_std(crate::init::create_private(dest)?);
        let mut hasher = Sha256::new();
        let mut bytes = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("cannot read {rel}"))?;
            bytes += chunk.len() as u64;
            if bytes > entry.bytes {
                bail!("{rel} is longer than the manifest says");
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        let sha = to_hex(&hasher.finalize());
        if bytes != entry.bytes || sha != entry.sha256 {
            bail!(
                "{rel}: sha256 mismatch (the manifest says {}, the content is {sha}); the \
                 repository is damaged or was changed",
                entry.sha256
            );
        }
        Ok(())
    }

    /// Deletes one snapshot folder's files (only the names a snapshot holds): the manifest
    /// first (so it is never seen as complete again), then the rest.
    async fn delete_snapshot(&self, name: &str) -> anyhow::Result<()> {
        for file in SNAPSHOT_FILES {
            let key = self.key(&Self::file_key(name, file));
            match self.store.delete(&key).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
                Err(e) => return Err(e).with_context(|| format!("cannot delete {key}")),
            }
        }
        if let Some(root) = &self.local {
            let _ = std::fs::remove_dir(root.join("snapshots").join(name));
        }
        Ok(())
    }

    /// Keeps the newest `keep` complete snapshots and deletes the older snapshot folders
    /// (incomplete ones included), then every blob no kept snapshot lists, and staging
    /// leftovers under `tmp/` older than 24 hours. `keep` must be at least 1.
    pub async fn prune(&self, keep: usize) -> anyhow::Result<PruneReport> {
        if keep == 0 {
            bail!("keep must be at least 1");
        }
        let complete = self.snapshots().await?;
        let Some(oldest_kept) = complete.get(complete.len().saturating_sub(keep)).cloned() else {
            // No complete snapshot: nothing says which blobs are needed.
            return Ok(PruneReport::default());
        };
        let scratch = tempfile::Builder::new()
            .prefix("ferrite-prune-")
            .tempdir()
            .context("cannot create a temporary folder")?;
        let mut needed: HashSet<ArtifactHash> = HashSet::new();
        for name in complete.iter().filter(|n| **n >= oldest_kept) {
            let manifest = self.read_manifest(name).await?;
            let dir = scratch.path().join(name);
            std::fs::create_dir(&dir)?;
            if let Some(list) = self.fetch_artifact_list(name, &manifest, &dir).await? {
                let count = manifest.artifacts.map_or(0, |a| a.count);
                for hash in read_hash_list(std::fs::File::open(&list)?, count) {
                    needed.insert(hash.with_context(|| format!("snapshot {name}"))?);
                }
            }
        }
        let mut report = PruneReport::default();
        for name in self.snapshot_dirs().await? {
            if name < oldest_kept {
                self.delete_snapshot(&name).await?;
                report.snapshots.push(name);
            }
        }
        let listed: Vec<_> = self.blobs.list_artifacts().try_collect().await?;
        for object in listed {
            if let Some(hash) = object.hash
                && !needed.contains(&hash)
            {
                self.blobs.delete(&hash).await?;
                report.artifacts += 1;
            }
        }
        let cutoff = Timestamp::from(chrono::Utc::now() - TMP_GRACE);
        let tmp: Vec<_> = self.blobs.list_tmp().try_collect().await?;
        let tmp_dir = format!("{}tmp/", self.prefix);
        for object in tmp {
            let ours = object
                .key
                .strip_prefix(tmp_dir.as_str())
                .is_some_and(is_staging_name);
            if ours && object.last_modified.as_micros() < cutoff.as_micros() {
                self.blobs.delete_key(&object.key).await?;
            }
        }
        Ok(report)
    }
}

/// `yyyymmddThhmmssZ`.
pub(crate) fn is_snapshot_name(name: &str) -> bool {
    name.len() == 16 && chrono::NaiveDateTime::parse_from_str(name, SNAPSHOT_FORMAT).is_ok()
}

/// A media store staging name: `<uuid>` or `probe-<uuid>`.
fn is_staging_name(name: &str) -> bool {
    let id = name.strip_prefix("probe-").unwrap_or(name);
    uuid::Uuid::try_parse(id).is_ok()
}

/// Reads an artifact list: `count` lines of one hash each. Each line is read with a cap
/// (a hostile file cannot make a line large), and a line that is not a hash, a missing
/// final newline or a different line count is an error. The caller checked the file's
/// sha256 first.
pub(crate) fn read_hash_list<R: Read>(
    input: R,
    count: u64,
) -> impl Iterator<Item = anyhow::Result<ArtifactHash>> {
    let mut reader = std::io::BufReader::new(input);
    let mut seen = 0u64;
    let mut done = false;
    let mut line = Vec::with_capacity(LINE as usize);
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        line.clear();
        let read = (&mut reader).take(LINE).read_until(b'\n', &mut line);
        let result = match read {
            Err(e) => Err(anyhow::Error::new(e).context("cannot read the artifact list")),
            Ok(0) if seen == count => {
                done = true;
                return None;
            }
            Ok(0) => Err(anyhow::anyhow!(
                "the artifact list has {seen} lines, the manifest says {count}"
            )),
            Ok(_) if line.last() != Some(&b'\n') => Err(anyhow::anyhow!(
                "the artifact list has an over-long or unterminated line {}",
                seen + 1
            )),
            Ok(_) if seen == count => Err(anyhow::anyhow!(
                "the artifact list has more lines than the manifest says ({count})"
            )),
            Ok(n) => {
                seen += 1;
                std::str::from_utf8(&line[..n - 1])
                    .ok()
                    .and_then(|h| ArtifactHash::parse(h).ok())
                    .with_context(|| format!("line {seen} of the artifact list is not a hash"))
            }
        };
        if result.is_err() {
            done = true;
        }
        Some(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_names() {
        assert!(is_snapshot_name("20261029T101500Z"));
        assert!(!is_snapshot_name("20261029T101500"));
        assert!(!is_snapshot_name("latest"));
        assert!(!is_snapshot_name("20261329T101500Z"));
    }

    #[test]
    fn staging_names() {
        let id = uuid::Uuid::now_v7().to_string();
        assert!(is_staging_name(&id));
        assert!(is_staging_name(&format!("probe-{id}")));
        assert!(!is_staging_name("keep.txt"));
        assert!(!is_staging_name(&format!("{id}/x")));
    }

    fn list(text: &str, count: u64) -> anyhow::Result<Vec<ArtifactHash>> {
        read_hash_list(text.as_bytes(), count).collect()
    }

    #[test]
    fn hash_lists() {
        let a = "ab".repeat(32);
        let b = "cd".repeat(32);
        let good = format!("{a}\n{b}\n");
        assert_eq!(list(&good, 2).unwrap().len(), 2);
        assert!(list("", 0).unwrap().is_empty());
        let e = list(&good, 3).unwrap_err();
        assert!(format!("{e:#}").contains("2 lines"), "{e:#}");
        let e = list(&good, 1).unwrap_err();
        assert!(format!("{e:#}").contains("more lines"), "{e:#}");
        let e = list(&a, 1).unwrap_err();
        assert!(format!("{e:#}").contains("unterminated"), "{e:#}");
        let e = list("not a hash\n", 1).unwrap_err();
        assert!(format!("{e:#}").contains("not a hash"), "{e:#}");
        // A huge line is refused after LINE bytes, not read whole.
        let huge = "a".repeat(10 * 1024 * 1024);
        let e = list(&huge, 1).unwrap_err();
        assert!(format!("{e:#}").contains("over-long"), "{e:#}");
        // Larger than the old 16 MiB manifest ceiling: 300 000 hashes stream fine.
        let mut big = String::new();
        let h = ArtifactHash::parse(&a).unwrap();
        for _ in 0..300_000 {
            big.push_str(h.as_str());
            big.push('\n');
        }
        assert!(big.len() as u64 > MAX_MANIFEST);
        assert_eq!(read_hash_list(big.as_bytes(), 300_000).count(), 300_000);
    }
}
