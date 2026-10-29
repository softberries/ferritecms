//! Moving a site's rows between databases (M4-1 P1–P8): [`export_to`] dumps the catalogue
//! tables of one read snapshot into an archive, [`import_from`] loads an archive into a
//! fresh database in one transaction, and [`copy`] streams one database into another.
//!
//! The target rules (P5) are in [`prepare_target`]: the database must be fresh (every
//! exported table empty but for the built-in content types and taxonomies the migrations
//! seed), and it is brought to the archive's schema first (an unmigrated database is
//! migrated up to it, a newer fresh one down). After the load, [`finalize`] migrates up
//! to this binary's schema under the usual startup rules and rebuilds the search index,
//! which is never exported (P2).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use bytes::Bytes;
use ferrite_config::DatabaseConfig;
use ferrite_domain::{ArtifactHash, ContentId, ContentStatus};
use ferrite_media::{MediaError, MediaStore};
use ferrite_store::catalogue::{self, Row, TableSpec};
use ferrite_store::{Backend, ExportReader, ImportWriter, MAX_EXPORT_PAGE, MigrationStatus, Store};
use futures::TryStreamExt;
use semver::Version;
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;
use tokio_util::io::{StreamReader, SyncIoBridge};
use uuid::Uuid;

use crate::archive::{
    ArchiveEntry, ArchiveReader, ArchiveWriter, ArtifactsInfo, EntryKind, FORMAT, Kind, Manifest,
    Newer, TableEntry, artifact_path, table_path,
};
use crate::migrate::{down_locked, up_locked};
use crate::open::{Decision, MIGRATE_LOCK_WAIT, decide};

/// Rows per export page.
const PAGE: u32 = 1000;
/// Rows per loader call.
const BATCH: usize = 500;
/// Items re-indexed per transaction.
const INDEX_BATCH: usize = 200;
/// Bytes per chunk when a blob goes into the media store.
const CHUNK: usize = 256 * 1024;

/// Every exported table, in catalogue (foreign-key) order.
pub(crate) fn exported_tables() -> Vec<&'static TableSpec> {
    catalogue::TABLES
        .iter()
        .filter(|t| t.is_exported())
        .collect()
}

/// The name of a backend in a manifest.
fn backend_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Sqlite => "sqlite",
        Backend::Postgres => "postgres",
    }
}

/// The source of an export or copy must be exactly at this binary's schema: the catalogue
/// describes it. Older: exit 1 (migrate first); newer: [`Newer`] (exit 3).
pub(crate) fn check_source(status: &MigrationStatus) -> anyhow::Result<()> {
    if status.db_version > status.binary_version || !status.unknown_applied.is_empty() {
        return Err(Newer(format!(
            "the source database is at schema {}, newer than this binary ({}); use the binary that \
             migrated it",
            status.db_version, status.binary_version
        ))
        .into());
    }
    if !status.pending.is_empty() {
        bail!(
            "the source database is at schema {}, this binary at {}: run `ferrite migrate up` first",
            status.db_version,
            status.binary_version
        );
    }
    Ok(())
}

/// The next page of `table` after the last row of the previous one.
async fn next_page(
    reader: &mut dyn ExportReader,
    table: &TableSpec,
    after: &mut Option<Row>,
) -> anyhow::Result<Vec<Row>> {
    let rows = reader
        .page(table, after.as_ref(), PAGE)
        .await
        .with_context(|| format!("cannot read table {}", table.name))?;
    if let Some(last) = rows.last() {
        *after = Some(
            table
                .key
                .iter()
                .filter_map(|k| last.get(*k).map(|v| ((*k).to_owned(), v.clone())))
                .collect(),
        );
    }
    Ok(rows)
}

/// What a load saw.
#[derive(Debug, Default)]
pub(crate) struct LoadStats {
    /// Rows loaded.
    pub rows: u64,
    /// Every content item, for the search rebuild.
    pub items: Vec<Uuid>,
    /// Users with TOTP on (their secrets are sealed with the source's key, P6).
    pub totp_users: u64,
    /// Blobs written to the media store.
    pub blobs_stored: u64,
    /// Blobs the media store already had.
    pub blobs_present: u64,
    /// Blobs read but not stored (no `--with-media`).
    pub blobs_ignored: u64,
}

impl LoadStats {
    fn observe(&mut self, table: &TableSpec, row: &Row) {
        self.rows += 1;
        match table.name {
            "content_items" => {
                if let Some(id) = row
                    .get("id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    self.items.push(id);
                }
            }
            "users" if row.get("totp_secret").is_some_and(|v| !v.is_null()) => {
                self.totp_users += 1;
            }
            _ => {}
        }
    }
}

/// The first exported table holding rows a fresh database does not have, if any.
async fn first_non_empty(store: &dyn Store) -> anyhow::Result<Option<&'static str>> {
    let mut reader = store.export_reader().await?;
    let result = async {
        for table in exported_tables() {
            let rows = match table.name {
                // Only `site_id`, which every migrated database has.
                "schema_meta" => continue,
                // The migrations seed the built-in rows.
                "content_types" | "taxonomies" => {
                    let rows = reader.page(table, None, MAX_EXPORT_PAGE).await?;
                    rows.into_iter()
                        .filter(|r| r.get("built_in") != Some(&serde_json::Value::Bool(true)))
                        .count()
                }
                _ => reader.page(table, None, 1).await?.len(),
            };
            if rows > 0 {
                return Ok(Some(table.name));
            }
        }
        Ok(None)
    }
    .await;
    reader.finish().await?;
    result
}

/// Readies a target for a load of `tables` written at schema `schema` (P5): refuses a
/// newer schema ([`Newer`]) and a database that is not fresh, migrates an unmigrated
/// database up to `schema` and a newer fresh one down to it, then opens the loader and
/// clears `tables` (the built-in rows the migrations seed).
pub(crate) async fn prepare_target(
    store: &dyn Store,
    schema: i64,
    tables: &[&'static TableSpec],
    binary: &Version,
) -> anyhow::Result<Box<dyn ImportWriter>> {
    let status = store.migration_status(binary).await?;
    let latest = status.binary_version;
    if schema > latest {
        return Err(Newer(format!(
            "the archive is at schema {schema}, newer than this binary ({latest}); use a newer ferrite"
        ))
        .into());
    }
    if schema < 1 {
        bail!("the archive has an invalid schema version {schema}");
    }
    let current = status.db_version;
    if current > latest || !status.unknown_applied.is_empty() {
        return Err(Newer(format!(
            "the target database is at schema {current}, newer than this binary ({latest})"
        ))
        .into());
    }
    if current > 0 {
        if !status.pending.is_empty() {
            bail!(
                "the target database is at schema {current} with pending migrations; run \
                 `ferrite migrate up` first"
            );
        }
        if let Some(table) = first_non_empty(store).await? {
            bail!(
                "the target database is not empty: table {table} has rows (import into a fresh \
                 database made by `ferrite init`)"
            );
        }
    }
    if current < schema {
        up_locked(store, Some(schema), MIGRATE_LOCK_WAIT).await?;
    } else if current > schema {
        // Safe: the database holds no data (checked above).
        down_locked(store, schema, true, MIGRATE_LOCK_WAIT).await?;
    }
    let mut writer = store.import_writer().await?;
    writer.clear(tables).await?;
    Ok(writer)
}

/// After a load: migrates up to this binary's schema under the startup rules (a migration
/// they refuse, e.g. a Postgres contract migration without `allow_contract`, is left
/// pending with a note), then rebuilds the search index of every item through the same
/// core function every content write uses.
pub(crate) async fn finalize(
    store: &dyn Store,
    db: &DatabaseConfig,
    binary: &Version,
    stats: &LoadStats,
) -> anyhow::Result<()> {
    let status = store.migration_status(binary).await?;
    match decide(&status, store.backend(), db, binary) {
        Ok(Decision::MigrateUp) => {
            let applied = up_locked(store, None, MIGRATE_LOCK_WAIT).await?;
            tracing::info!(?applied, "applied pending migrations");
        }
        Ok(_) => {}
        Err(e) => println!(
            "note: the database stays at schema {}: {e:#}",
            status.db_version
        ),
    }
    rebuild_search(store, &stats.items)
        .await
        .context("the rows were imported, but rebuilding the search index failed")
}

async fn rebuild_search(store: &dyn Store, items: &[Uuid]) -> anyhow::Result<()> {
    for chunk in items.chunks(INDEX_BATCH) {
        let tx = store.begin().await?;
        for id in chunk {
            let Some(item) = tx.content().get(ContentId::from_uuid(*id)).await? else {
                continue;
            };
            if item.status != ContentStatus::Trash {
                ferrite_core::content::index_search(tx.as_ref(), &item).await?;
            }
        }
        tx.commit().await?;
    }
    Ok(())
}

/// Prints the P6 note on TOTP secrets, when any user has TOTP on.
pub(crate) fn print_totp_hint(users: u64, data: Option<&Path>) {
    if users == 0 {
        return;
    }
    let data = data.map_or_else(|| "<data>".to_owned(), |d| d.display().to_string());
    println!(
        "note: {users} user(s) have two-factor sign-in (TOTP) on. Their secrets are sealed with \
         the source site's key, which an export never holds: copy the source's secret.key into \
         {data} (or set auth.secret_key to the source's key), or run `ferrite user disable-totp \
         --login <login> --data {data}` for each of them."
    );
}

/// Writes a hex sha256 of everything written.
struct Hashing<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The table files of one snapshot, spooled to `dir` (tar needs their sizes).
struct Spooled {
    tables: Vec<TableEntry>,
    hashes: Vec<ArtifactHash>,
    site_id: Option<String>,
}

async fn spool(reader: &mut dyn ExportReader, dir: &Path) -> anyhow::Result<Spooled> {
    let mut out = Spooled {
        tables: Vec::new(),
        hashes: Vec::new(),
        site_id: None,
    };
    for table in exported_tables() {
        let path = dir.join(format!("{}.ndjson", table.name));
        let file = std::fs::File::create(&path)
            .with_context(|| format!("cannot create {}", path.display()))?;
        let mut w = Hashing {
            inner: io::BufWriter::new(file),
            hasher: Sha256::new(),
        };
        let mut rows = 0u64;
        let mut after = None;
        loop {
            let page = next_page(reader, table, &mut after).await?;
            if page.is_empty() {
                break;
            }
            for row in &page {
                match table.name {
                    "artifacts" => {
                        let hash = row.get("hash").and_then(|v| v.as_str()).unwrap_or("");
                        out.hashes.push(
                            ArtifactHash::parse(hash)
                                .with_context(|| format!("invalid artifact hash {hash:?}"))?,
                        );
                    }
                    "schema_meta" => {
                        out.site_id = row.get("value").and_then(|v| v.as_str()).map(str::to_owned);
                    }
                    _ => {}
                }
                serde_json::to_writer(&mut w, row)?;
                w.write_all(b"\n")?;
                rows += 1;
            }
        }
        w.flush()?;
        out.tables.push(TableEntry {
            name: table.name.to_owned(),
            rows,
            sha256: ferrite_domain::to_hex(&w.hasher.finalize()),
        });
    }
    Ok(out)
}

/// Writes an export archive of `store` to `out` (M4-1 P7): every exported table from one
/// read snapshot (on SQLite one reader connection is held for the dump) and, with
/// `media`, the blob of every `artifacts` row. A blob missing from the store is left out
/// with a warning. The source must be at this binary's schema ([`check_source`]).
/// Returns the manifest and `out`.
pub async fn export_to<W: Write + Send + 'static>(
    store: &dyn Store,
    media: Option<Arc<MediaStore>>,
    out: W,
    binary: &Version,
) -> anyhow::Result<(Manifest, W)> {
    let status = store.migration_status(binary).await?;
    check_source(&status)?;
    let dir = tempfile::Builder::new()
        .prefix("ferrite-export-")
        .tempdir()
        .context("cannot create a temporary folder")?;
    let mut reader = store.export_reader().await?;
    let spooled = spool(reader.as_mut(), dir.path()).await;
    reader.finish().await?;
    let spooled = spooled?;

    let mut blobs = Vec::new();
    if let Some(media) = &media {
        for hash in &spooled.hashes {
            match media.head(hash).await? {
                Some(size) => blobs.push((hash.clone(), size)),
                None => tracing::warn!(%hash, "artifact missing from the media store; left out"),
            }
        }
    }
    let manifest = Manifest {
        format: FORMAT,
        kind: Kind::Export,
        ferrite_version: binary.to_string(),
        schema_version: status.db_version,
        source_backend: backend_name(store.backend()).to_owned(),
        site_id: spooled.site_id,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        tables: Some(spooled.tables),
        files: None,
        artifacts: media.as_ref().map(|_| ArtifactsInfo {
            count: blobs.len() as u64,
            bytes: blobs.iter().map(|(_, s)| s).sum(),
        }),
    };
    let handle = Handle::current();
    let m = manifest.clone();
    let out = tokio::task::spawn_blocking(move || -> anyhow::Result<W> {
        let mut writer = ArchiveWriter::new(out, &m)?;
        for t in m.tables.iter().flatten() {
            writer.append_file(
                &table_path(&t.name),
                &dir.path().join(format!("{}.ndjson", t.name)),
            )?;
        }
        if let Some(media) = media {
            for (hash, size) in blobs {
                let read = handle
                    .block_on(media.open_read(&hash, None))
                    .with_context(|| format!("cannot read artifact {hash}"))?;
                if read.size != size {
                    bail!("artifact {hash} changed size during the export");
                }
                let stream = read.stream.map_err(|e: MediaError| io::Error::other(e));
                let bridge =
                    SyncIoBridge::new_with_handle(StreamReader::new(stream), handle.clone());
                writer.append_stream(&artifact_path(&hash), size, bridge)?;
            }
        }
        let out = writer.finish()?;
        drop(dir);
        Ok(out)
    })
    .await??;
    Ok((manifest, out))
}

/// Loads an export archive read from `input` into the fresh database `store` (P5) in one
/// transaction; with `media`, stores its blobs there too (skipping those present). The
/// archive is checked as it is read (see [`crate::archive`]); any refusal, damage or
/// truncation rolls the whole load back. The caller then runs [`finalize`].
pub async fn import_from(
    input: Box<dyn Read + Send>,
    store: Arc<dyn Store>,
    media: Option<Arc<MediaStore>>,
    binary: Version,
) -> anyhow::Result<(Manifest, LoadStats)> {
    let handle = Handle::current();
    tokio::task::spawn_blocking(move || {
        load_archive(&handle, input, store.as_ref(), media, &binary)
    })
    .await?
}

fn load_archive(
    handle: &Handle,
    input: Box<dyn Read + Send>,
    store: &dyn Store,
    media: Option<Arc<MediaStore>>,
    binary: &Version,
) -> anyhow::Result<(Manifest, LoadStats)> {
    let mut reader = ArchiveReader::new(input)?;
    let mut entries = reader.entries()?;
    let manifest = entries.manifest().clone();
    if manifest.kind != Kind::Export {
        bail!("the archive is a backup, not an export (use `ferrite restore`)");
    }
    let tables = manifest
        .tables
        .iter()
        .flatten()
        .map(|t| catalogue::table(&t.name).context("unknown table"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut writer = handle.block_on(prepare_target(
        store,
        manifest.schema_version,
        &tables,
        binary,
    ))?;
    let mut stats = LoadStats::default();
    while let Some(mut entry) = entries.next_entry()? {
        match entry.kind.clone() {
            EntryKind::Table { spec, rows } => {
                let read = load_table(handle, writer.as_mut(), spec, &mut entry, &mut stats)?;
                let path = entry.path.clone();
                entry.finish()?;
                if read != rows {
                    bail!("{path}: {read} rows, the manifest says {rows}");
                }
                handle.block_on(writer.finish_table(spec))?;
            }
            EntryKind::Artifact(hash) => {
                store_blob(handle, media.as_ref(), &hash, entry, &mut stats)?;
            }
            EntryKind::File(path) => bail!("unexpected file {path} in an export"),
        }
    }
    handle.block_on(writer.commit())?;
    Ok((manifest, stats))
}

/// Streams the rows of one table file into the loader; returns how many there were.
fn load_table<R: Read>(
    handle: &Handle,
    writer: &mut dyn ImportWriter,
    spec: &'static TableSpec,
    entry: &mut ArchiveEntry<'_, R>,
    stats: &mut LoadStats,
) -> anyhow::Result<u64> {
    let path = entry.path.clone();
    let mut lines = BufReader::new(entry);
    let mut batch = Vec::with_capacity(BATCH);
    let mut count = 0u64;
    let mut line = String::new();
    loop {
        line.clear();
        if lines
            .read_line(&mut line)
            .with_context(|| format!("cannot read {path}"))?
            == 0
        {
            break;
        }
        count += 1;
        let row: Row = serde_json::from_str(line.trim_end_matches('\n'))
            .with_context(|| format!("{path}: line {count} is not a JSON object"))?;
        stats.observe(spec, &row);
        batch.push(row);
        if batch.len() == BATCH {
            handle
                .block_on(writer.rows(spec, std::mem::take(&mut batch)))
                .with_context(|| format!("cannot load {path}"))?;
        }
    }
    if !batch.is_empty() {
        handle
            .block_on(writer.rows(spec, batch))
            .with_context(|| format!("cannot load {path}"))?;
    }
    Ok(count)
}

/// Puts one blob into `media` (unless it is there with the same size), checking its hash.
fn store_blob<R: Read>(
    handle: &Handle,
    media: Option<&Arc<MediaStore>>,
    hash: &ArtifactHash,
    mut entry: ArchiveEntry<'_, R>,
    stats: &mut LoadStats,
) -> anyhow::Result<()> {
    let Some(media) = media else {
        entry.finish()?;
        stats.blobs_ignored += 1;
        return Ok(());
    };
    if handle.block_on(media.head(hash))? == Some(entry.size()) {
        entry.finish()?;
        stats.blobs_present += 1;
        return Ok(());
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<io::Result<Bytes>>(4);
    let m = media.clone();
    let put = handle.spawn(async move {
        let stream = futures::stream::poll_fn(move |cx| rx.poll_recv(cx));
        m.put_stream(stream, u64::MAX).await
    });
    let mut buf = vec![0; CHUNK];
    let mut read_error = None;
    loop {
        match entry.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx
                    .blocking_send(Ok(Bytes::copy_from_slice(&buf[..n])))
                    .is_err()
                {
                    break;
                }
            }
            Err(e) => {
                let _ = tx.blocking_send(Err(io::Error::other(e.to_string())));
                read_error = Some(e);
                break;
            }
        }
    }
    drop(tx);
    let placed = handle.block_on(put)?;
    if let Some(e) = read_error {
        return Err(anyhow::Error::new(e).context(format!("cannot read {}", entry.path)));
    }
    entry.finish()?;
    let placed = placed.with_context(|| format!("cannot store artifact {hash}"))?;
    if placed.hash != *hash {
        bail!("artifact {hash} holds content with hash {}", placed.hash);
    }
    stats.blobs_stored += 1;
    Ok(())
}

/// Copies every exported row of `source` into the fresh database `target` (P8), streamed
/// from one read snapshot into one write transaction. The caller then runs [`finalize`].
pub async fn copy(
    source: &dyn Store,
    target: &dyn Store,
    binary: &Version,
) -> anyhow::Result<LoadStats> {
    let status = source.migration_status(binary).await?;
    check_source(&status)?;
    let tables = exported_tables();
    let mut writer = prepare_target(target, status.db_version, &tables, binary).await?;
    let mut reader = source.export_reader().await?;
    let mut stats = LoadStats::default();
    let copied = async {
        for table in &tables {
            let mut after = None;
            loop {
                let rows = next_page(reader.as_mut(), table, &mut after).await?;
                if rows.is_empty() {
                    break;
                }
                for row in &rows {
                    stats.observe(table, row);
                }
                writer
                    .rows(table, rows)
                    .await
                    .with_context(|| format!("cannot load table {}", table.name))?;
            }
            writer.finish_table(table).await?;
        }
        anyhow::Ok(())
    }
    .await;
    reader.finish().await?;
    copied?;
    writer.commit().await?;
    Ok(stats)
}
