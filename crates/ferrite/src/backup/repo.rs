//! A backup repository (M4-1 P13): a folder or an S3 prefix holding
//!
//! - `artifacts/<ab>/<hash>`: blobs shared by every snapshot, copied only when missing;
//! - `snapshots/<UTC yyyymmddThhmmssZ>/{manifest.json, ferrite.db, ferrite.toml, secret.key}`.
//!
//! A snapshot is complete once its `manifest.json` exists: it is written last, after the
//! other files and the read-back check of `ferrite.db`, and deleted first when a snapshot
//! is pruned. One site per repository: pruning deletes every blob no kept manifest lists.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use bytes::Bytes;
use ferrite_domain::{ArtifactHash, to_hex};
use ferrite_media::{MediaError, MediaStore};
use futures::{StreamExt, TryStreamExt};
use object_store::path::Path as Key;
use object_store::{ObjectStore, ObjectStoreExt};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::target::RepoLocation;
use crate::archive::{FileEntry, MANIFEST, Manifest};
use crate::media_url::parse_media_url;

/// The snapshot name format (UTC).
pub const SNAPSHOT_FORMAT: &str = "%Y%m%dT%H%M%SZ";
/// The largest `manifest.json` read from a repository.
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;

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
    /// Blobs deleted because no kept manifest lists them.
    pub artifacts: u64,
}

impl Repo {
    /// Opens the repository at `loc`. A folder is created when `create` is set, otherwise
    /// it must exist. Nothing is probed.
    pub fn open(loc: &RepoLocation, create: bool) -> anyhow::Result<Repo> {
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
        Ok(Repo {
            store: blobs.object_store(),
            prefix: blobs.prefix().to_owned(),
            local,
            blobs,
            display,
        })
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
                .any(|(e, (name, _))| e.path != *name)
        {
            bail!("the snapshot files do not match the manifest");
        }
        let name = self.next_name().await?;
        let result = async {
            for (file, path) in files {
                self.put_file(&Self::file_key(&name, file), path).await?;
            }
            if let Some(db) = entries.iter().find(|e| e.path == "ferrite.db") {
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

    /// Deletes one snapshot folder: the manifest first (so it is never seen as complete
    /// again), then the rest.
    async fn delete_snapshot(&self, name: &str) -> anyhow::Result<()> {
        let manifest = self.key(&Self::file_key(name, MANIFEST));
        match self.store.delete(&manifest).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
            Err(e) => return Err(e).context("cannot delete a snapshot manifest"),
        }
        let dir = self.key(&format!("snapshots/{name}"));
        let keys: Vec<Key> = self
            .store
            .list(Some(&dir))
            .map_ok(|m| m.location)
            .try_collect()
            .await?;
        for key in keys {
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
    /// (incomplete ones included), then every blob no kept manifest lists, and staging
    /// leftovers under `tmp/`. `keep` must be at least 1.
    pub async fn prune(&self, keep: usize) -> anyhow::Result<PruneReport> {
        if keep == 0 {
            bail!("keep must be at least 1");
        }
        let complete = self.snapshots().await?;
        let Some(oldest_kept) = complete.get(complete.len().saturating_sub(keep)).cloned() else {
            // No complete snapshot: nothing says which blobs are needed.
            return Ok(PruneReport::default());
        };
        let kept: Vec<&String> = complete.iter().filter(|n| **n >= oldest_kept).collect();
        let mut needed: HashSet<String> = HashSet::new();
        for name in &kept {
            let m = self.read_manifest(name).await?;
            needed.extend(m.artifact_hashes);
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
                && !needed.contains(hash.as_str())
            {
                self.blobs.delete(&hash).await?;
                report.artifacts += 1;
            }
        }
        let tmp: Vec<_> = self.blobs.list_tmp().try_collect().await?;
        for object in tmp {
            self.blobs.delete_key(&object.key).await?;
        }
        Ok(report)
    }
}

/// `yyyymmddThhmmssZ`.
fn is_snapshot_name(name: &str) -> bool {
    name.len() == 16 && chrono::NaiveDateTime::parse_from_str(name, SNAPSHOT_FORMAT).is_ok()
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
}
