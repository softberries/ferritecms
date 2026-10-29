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
/// The longest table row (one NDJSON line) read from an archive; far above any row the
/// store accepts, it only bounds memory against a hostile archive.
const MAX_LINE: usize = 64 * 1024 * 1024;

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

/// A target readied by [`prepare_target`]: the open loader, and the schema step taken for
/// it, which [`undo_schema`] reverts when the load fails.
pub(crate) struct Prepared {
    pub writer: Box<dyn ImportWriter>,
    /// The target's schema before the step.
    pub original: i64,
    /// The schema the load runs at (the archive's).
    pub schema: i64,
}

/// Readies a target for a load of `tables` written at schema `schema` (P5):
/// - refuses a newer archive or target ([`Newer`]), a target with pending migrations and
///   a database that is not fresh;
/// - migrates an unmigrated database up to `schema` and a newer fresh one down to it;
/// - checks, at `schema`, that the migration to this binary's schema after the load is
///   allowed by the startup rules for `db` (`auto_migrate`, a Postgres contract migration
///   needs `allow_contract`), so the load is refused before any row is written rather
///   than left at an older schema;
/// - opens the loader and clears `tables` (the built-in rows the migrations seed).
///
/// A failure after the schema step undoes it ([`undo_schema`]).
pub(crate) async fn prepare_target(
    store: &dyn Store,
    schema: i64,
    tables: &[&'static TableSpec],
    db: &DatabaseConfig,
    binary: &Version,
) -> anyhow::Result<Prepared> {
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
                 `ferrite migrate up` first (it applies contract migrations too)"
            );
        }
        if let Some(table) = first_non_empty(store).await? {
            bail!(
                "the target database is not empty: table {table} has rows (import into a fresh \
                 database made by `ferrite init`)"
            );
        }
    }
    let stepped = if current < schema {
        up_locked(store, Some(schema), MIGRATE_LOCK_WAIT).await
    } else if current > schema {
        // Safe: the database holds no data (checked above).
        down_locked(store, schema, true, MIGRATE_LOCK_WAIT).await
    } else {
        Ok(Vec::new())
    };
    if let Err(e) = stepped {
        // Each migration commits on its own, so a failure can stop part way.
        let now = schema_now(store, binary).await;
        tracing::error!(error = %format!("{e:#}"), current, schema, %now, "schema step failed");
        eprintln!(
            "warning: migrating the target from schema {current} to {schema} failed; it stopped \
             at schema {now}. Run `ferrite migrate up` before retrying."
        );
        return Err(e);
    }
    let opened = async {
        let status = store.migration_status(binary).await?;
        decide(&status, store.backend(), db, binary).context(
            "after the load the database is migrated to this binary's schema, which these \
             database settings refuse",
        )?;
        let mut writer = store.import_writer().await?;
        writer.clear(tables).await?;
        anyhow::Ok(writer)
    }
    .await;
    match opened {
        Ok(writer) => Ok(Prepared {
            writer,
            original: current,
            schema,
        }),
        Err(e) => {
            undo_schema(store, schema, current, binary, false).await;
            Err(e)
        }
    }
}

/// The target's applied schema version for a message, or `unknown`.
async fn schema_now(store: &dyn Store, binary: &Version) -> String {
    store
        .migration_status(binary)
        .await
        .map_or_else(|_| "unknown".to_owned(), |s| s.db_version.to_string())
}

/// Best effort after a failed load: takes the target from `schema` (the archive's) back to
/// `original` (its schema before [`prepare_target`]), so it is fresh at its own schema
/// again and a retry is accepted. A migration down is lossy, so the target is first
/// checked to be still fresh; when it holds rows (a commit that reported an error but
/// landed) nothing is undone and a warning says so. With `commit_attempted`, a check that
/// cannot run (the reader needs the binary's columns, which an older schema may lack) also
/// skips the undo; without it the load never reached its commit, its transaction was
/// rolled back, and the undo goes ahead. Failures are logged and printed, not returned.
pub(crate) async fn undo_schema(
    store: &dyn Store,
    schema: i64,
    original: i64,
    binary: &Version,
    commit_attempted: bool,
) {
    if schema == original {
        return;
    }
    let skip = match first_non_empty(store).await {
        Ok(None) => None,
        Ok(Some(table)) => Some(format!("table {table} has rows")),
        Err(e) if commit_attempted => Some(format!("its tables cannot be checked ({e:#})")),
        Err(_) => None,
    };
    if let Some(why) = skip {
        tracing::error!(%why, schema, original, "the target's schema step is not undone");
        eprintln!(
            "warning: the target database is left at schema {schema} (it was at {original}): {why}, \
             so the load may have been committed. Check it before retrying; a fresh target is \
             needed for another import."
        );
        return;
    }
    let undone = if original > schema {
        up_locked(store, Some(original), MIGRATE_LOCK_WAIT).await
    } else {
        down_locked(store, original, true, MIGRATE_LOCK_WAIT).await
    };
    let now = schema_now(store, binary).await;
    match undone {
        Ok(_) if now == original.to_string() => {}
        Ok(_) => {
            tracing::error!(original, %now, "the target's schema was not restored");
            eprintln!(
                "warning: the target database is at schema {now}, not {original} as before; run \
                 `ferrite migrate up` before retrying"
            );
        }
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), original, %now, "restoring the target's schema failed");
            eprintln!(
                "warning: the target database is at schema {now} (it was at {original}): {e:#}; \
                 run `ferrite migrate up` before retrying"
            );
        }
    }
}

/// Undoes the schema step when dropped while armed, so a panic in the blocking load
/// still reverts it (after the writer, declared later, has rolled back).
struct UndoGuard<'a> {
    handle: &'a Handle,
    store: &'a dyn Store,
    binary: &'a Version,
    schema: i64,
    original: i64,
    armed: bool,
    commit_attempted: bool,
}

impl Drop for UndoGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.handle.block_on(undo_schema(
                self.store,
                self.schema,
                self.original,
                self.binary,
                self.commit_attempted,
            ));
        }
    }
}

/// After a load: migrates up to this binary's schema under the startup rules (which
/// [`prepare_target`] already checked), then rebuilds the search index of every item
/// through the same core function every content write uses. The rebuild runs only at
/// the binary's schema, which the repository queries are written for.
pub(crate) async fn finalize(
    store: &dyn Store,
    db: &DatabaseConfig,
    binary: &Version,
    stats: &LoadStats,
) -> anyhow::Result<()> {
    let status = store.migration_status(binary).await?;
    let migrated = match decide(&status, store.backend(), db, binary) {
        Ok(Decision::MigrateUp) => up_locked(store, None, MIGRATE_LOCK_WAIT)
            .await
            .map(|applied| tracing::info!(?applied, "applied pending migrations")),
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    };
    migrated.context(
        "the rows were imported, but migrating to this binary's schema failed and the search \
         index is not rebuilt; this import cannot be finished, so import again into a fresh \
         database",
    )?;
    rebuild_search(store, &stats.items).await.context(
        "the rows were imported, but rebuilding the search index failed; import again into a \
         fresh database",
    )
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

/// Hashes everything read through it.
struct HashingRead<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> Read for HashingRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
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
        let file = crate::init::create_private(&path)?;
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
    let mut missing = Vec::new();
    if let Some(media) = &media {
        for hash in &spooled.hashes {
            match media.head(hash).await? {
                Some(size) => blobs.push((hash.clone(), size)),
                None => {
                    eprintln!("missing artifact {hash}");
                    missing.push(hash.to_string());
                }
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
        missing,
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
                let mut hashing = HashingRead {
                    inner: bridge,
                    hasher: Sha256::new(),
                };
                writer.append_stream(&artifact_path(&hash), size, &mut hashing)?;
                let got = ferrite_domain::to_hex(&hashing.hasher.finalize());
                if got != hash.as_str() {
                    bail!(
                        "artifact {hash}: the media store holds content with hash {got} under \
                         that name (a damaged file); the export is not usable"
                    );
                }
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
    db: DatabaseConfig,
    binary: Version,
    seams: LoadSeams,
) -> anyhow::Result<(Manifest, LoadStats)> {
    let handle = Handle::current();
    tokio::task::spawn_blocking(move || {
        load_archive(&handle, input, store.as_ref(), media, &db, &binary, seams)
    })
    .await?
}

/// Test seams of [`import_from`] (empty without the `test-seams` feature).
#[derive(Debug, Default, Clone, Copy)]
pub struct LoadSeams {
    /// Report an error after a successful commit, as a lost connection can.
    #[cfg(feature = "test-seams")]
    pub fail_after_commit: bool,
}

fn load_archive(
    handle: &Handle,
    input: Box<dyn Read + Send>,
    store: &dyn Store,
    media: Option<Arc<MediaStore>>,
    db: &DatabaseConfig,
    binary: &Version,
    seams: LoadSeams,
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
    let Prepared {
        writer,
        original,
        schema,
    } = handle.block_on(prepare_target(
        store,
        manifest.schema_version,
        &tables,
        db,
        binary,
    ))?;
    // Declared before the writer, so on an error or a panic the writer is dropped (rolled
    // back) first and the guard then undoes the schema step.
    let mut guard = UndoGuard {
        handle,
        store,
        binary,
        schema,
        original,
        armed: true,
        commit_attempted: false,
    };
    let mut writer = writer;
    let mut stats = LoadStats::default();
    load_entries(handle, &mut entries, writer.as_mut(), media, &mut stats)?;
    guard.commit_attempted = true;
    handle.block_on(writer.commit())?;
    #[cfg(feature = "test-seams")]
    if seams.fail_after_commit {
        bail!("test seam: an error reported after the commit");
    }
    let _ = seams;
    guard.armed = false;
    Ok((manifest, stats))
}

fn load_entries<R: Read>(
    handle: &Handle,
    entries: &mut crate::archive::Entries<'_, R>,
    writer: &mut dyn ImportWriter,
    media: Option<Arc<MediaStore>>,
    stats: &mut LoadStats,
) -> anyhow::Result<()> {
    while let Some(mut entry) = entries.next_entry()? {
        match entry.kind.clone() {
            EntryKind::Table { spec, rows } => {
                let read = load_table(handle, writer, spec, &mut entry, stats)?;
                let path = entry.path.clone();
                entry.finish()?;
                if read != rows {
                    bail!("{path}: {read} rows, the manifest says {rows}");
                }
                handle.block_on(writer.finish_table(spec))?;
            }
            EntryKind::Artifact(hash) => {
                store_blob(handle, media.as_ref(), &hash, entry, stats)?;
            }
            EntryKind::File(path) => bail!("unexpected file {path} in an export"),
        }
    }
    Ok(())
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
    let mut line = Vec::new();
    loop {
        line.clear();
        // At most MAX_LINE bytes plus the newline, so a hostile entry cannot grow it.
        let n = (&mut lines)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)
            .with_context(|| format!("cannot read {path}"))?;
        if n == 0 {
            break;
        }
        count += 1;
        if line.last() == Some(&b'\n') {
            line.pop();
        } else if line.len() > MAX_LINE {
            bail!(
                "{path}: line {count} is longer than {} MiB; the archive is damaged or hostile",
                MAX_LINE / (1024 * 1024)
            );
        }
        let row: Row = serde_json::from_slice(&line)
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
    // The entry never yields more than its declared size; the cap guards the store too.
    let limit = entry.size();
    let put = handle.spawn(async move {
        let stream = futures::stream::poll_fn(move |cx| rx.poll_recv(cx));
        m.put_stream(stream, limit).await
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
    db: &DatabaseConfig,
    binary: &Version,
) -> anyhow::Result<LoadStats> {
    let status = source.migration_status(binary).await?;
    check_source(&status)?;
    let tables = exported_tables();
    let Prepared {
        mut writer,
        original,
        schema,
    } = prepare_target(target, status.db_version, &tables, db, binary).await?;
    let mut stats = LoadStats::default();
    let (loaded, commit_attempted) =
        match copy_rows(source, writer.as_mut(), &tables, &mut stats).await {
            Ok(()) => (writer.commit().await.map_err(Into::into), true),
            Err(e) => {
                drop(writer);
                (Err(e), false)
            }
        };
    if let Err(e) = loaded {
        undo_schema(target, schema, original, binary, commit_attempted).await;
        return Err(e);
    }
    Ok(stats)
}

/// Streams every table of one read snapshot of `source` into `writer`.
async fn copy_rows(
    source: &dyn Store,
    writer: &mut dyn ImportWriter,
    tables: &[&'static TableSpec],
    stats: &mut LoadStats,
) -> anyhow::Result<()> {
    let mut reader = source.export_reader().await?;
    let copied = async {
        for table in tables {
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
    let finished = reader.finish().await;
    copied?;
    Ok(finished?)
}
