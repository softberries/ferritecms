//! The IR: a temporary SQLite file holding a parsed WXR file (P21), so the passes read
//! records by type or id instead of the XML, and a resumed run skips the parse and every
//! pass already done.
//!
//! Tables: `channel` (one row), `authors`, `terms` and `items` (`seq` keeps file order,
//! `record_json` is the [`model`](crate::model) record as JSON), `passes(name, done_at)`,
//! `meta(key, value)` (the WXR file's digest, so a re-run finds the IR of the same
//! file), and `media_urls(path, media_id, url)`, the [`MediaIndex`](crate::urls::MediaIndex)
//! pass 2 builds, so a resumed run has it; `content_paths(wp_id, item_id, path)` (each
//! imported item's id and path) and `fixups(wp_id)` (items whose links or relations wait
//! for later items), which pass 3 fills and the fix-up reads. It is a cache: what the site holds is decided by `import_map` (P3).

use std::path::Path;
use std::time::Duration;

use futures::{Stream, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{Sqlite, Transaction};
use uuid::Uuid;

use crate::ImportError;
use crate::model::{Author, Channel, Item, Record, Term};

/// Records written per transaction by [`Ir::push`].
const PUSH_BATCH: usize = 1_000;
/// Connections: one writer at a time, and readers beside a running [`Ir::items`] stream.
const CONNECTIONS: u32 = 4;

const SCHEMA: &str = "
CREATE TABLE channel (id INTEGER PRIMARY KEY CHECK (id = 1), record_json TEXT NOT NULL);
CREATE TABLE authors (seq INTEGER PRIMARY KEY, wp_id INTEGER NOT NULL, record_json TEXT NOT NULL);
CREATE TABLE terms (
    seq INTEGER PRIMARY KEY,
    taxonomy TEXT NOT NULL,
    wp_id INTEGER NOT NULL,
    record_json TEXT NOT NULL
);
CREATE TABLE items (
    seq INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    wp_id INTEGER NOT NULL,
    record_json TEXT NOT NULL
);
CREATE INDEX items_kind ON items (kind, seq);
CREATE INDEX items_wp_id ON items (wp_id, seq);
CREATE TABLE passes (name TEXT PRIMARY KEY, done_at TEXT NOT NULL);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE media_urls (path TEXT PRIMARY KEY, media_id TEXT NOT NULL, url TEXT NOT NULL);
CREATE TABLE content_paths (wp_id INTEGER PRIMARY KEY, item_id TEXT NOT NULL, path TEXT NOT NULL);
CREATE TABLE fixups (wp_id INTEGER PRIMARY KEY);
";

/// An open IR file. Writes go through [`push`](Self::push) (buffered in a transaction;
/// [`flush`](Self::flush) commits it) and [`mark_pass`](Self::mark_pass).
pub struct Ir {
    pool: SqlitePool,
    /// The open transaction of [`push`](Self::push) and how many records it holds.
    pending: Option<(Transaction<'static, Sqlite>, usize)>,
}

impl Ir {
    /// Creates a new IR file at `path` (its folder must exist); an existing file is
    /// refused.
    pub async fn create(path: &Path) -> Result<Ir, ImportError> {
        if path.exists() {
            return Err(ImportError::File {
                path: path.to_owned(),
                source: std::io::ErrorKind::AlreadyExists.into(),
            });
        }
        let ir = Self::connect(path, true).await?;
        sqlx::raw_sql(SCHEMA).execute(&ir.pool).await?;
        Ok(ir)
    }

    /// Opens an existing IR file.
    pub async fn open(path: &Path) -> Result<Ir, ImportError> {
        Self::connect(path, false).await
    }

    async fn connect(path: &Path, create: bool) -> Result<Ir, ImportError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(create)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(CONNECTIONS)
            .connect_with(options)
            .await?;
        Ok(Ir {
            pool,
            pending: None,
        })
    }

    /// Closes the file. A [`push`](Self::push) not yet [flushed](Self::flush) is lost.
    pub async fn close(mut self) {
        // The pool waits for every connection, so the transaction's goes back first.
        drop(self.pending.take());
        self.pool.close().await;
    }

    /// Appends `record`, in a transaction committed every 1 000 records; call
    /// [`flush`](Self::flush) after the last one. A channel replaces the stored one.
    pub async fn push(&mut self, record: &Record) -> Result<(), ImportError> {
        if self.pending.is_none() {
            self.pending = Some((self.pool.begin().await?, 0));
        }
        let Some((tx, count)) = self.pending.as_mut() else {
            return Ok(());
        };
        match record {
            Record::Channel(c) => {
                sqlx::query(
                    "INSERT INTO channel (id, record_json) VALUES (1, ?1)
                     ON CONFLICT (id) DO UPDATE SET record_json = excluded.record_json",
                )
                .bind(json(c)?)
                .execute(&mut **tx)
                .await?;
            }
            Record::Author(a) => {
                sqlx::query("INSERT INTO authors (wp_id, record_json) VALUES (?1, ?2)")
                    .bind(a.wp_id)
                    .bind(json(a)?)
                    .execute(&mut **tx)
                    .await?;
            }
            Record::Term(t) => {
                sqlx::query("INSERT INTO terms (taxonomy, wp_id, record_json) VALUES (?1, ?2, ?3)")
                    .bind(&t.taxonomy)
                    .bind(t.wp_id)
                    .bind(json(t)?)
                    .execute(&mut **tx)
                    .await?;
            }
            Record::Item(i) => {
                sqlx::query("INSERT INTO items (kind, wp_id, record_json) VALUES (?1, ?2, ?3)")
                    .bind(&i.post_type)
                    .bind(i.wp_id)
                    .bind(json(i)?)
                    .execute(&mut **tx)
                    .await?;
            }
        }
        *count += 1;
        if *count >= PUSH_BATCH {
            self.flush().await?;
        }
        Ok(())
    }

    /// Commits the records [`push`](Self::push) has buffered.
    pub async fn flush(&mut self) -> Result<(), ImportError> {
        if let Some((tx, _)) = self.pending.take() {
            tx.commit().await?;
        }
        Ok(())
    }

    /// The channel; [`ImportError::Missing`] when the file had none.
    pub async fn channel(&self) -> Result<Channel, ImportError> {
        let row: Option<String> =
            sqlx::query_scalar("SELECT record_json FROM channel WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some(s) => Ok(serde_json::from_str(&s)?),
            None => Err(ImportError::Missing {
                what: "channel",
                offset: 0,
            }),
        }
    }

    /// Every author, in file order.
    pub async fn authors(&self) -> Result<Vec<Author>, ImportError> {
        self.all("SELECT record_json FROM authors ORDER BY seq")
            .await
    }

    /// Every term, in file order.
    pub async fn terms(&self) -> Result<Vec<Term>, ImportError> {
        self.all("SELECT record_json FROM terms ORDER BY seq").await
    }

    async fn all<T: DeserializeOwned>(&self, sql: &'static str) -> Result<Vec<T>, ImportError> {
        let rows: Vec<String> = sqlx::query_scalar(sql).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|s| serde_json::from_str(s).map_err(ImportError::from))
            .collect()
    }

    /// The items whose post type is one of `post_types` (every item when it is empty),
    /// in file order, streamed.
    pub fn items(
        &self,
        post_types: &[&str],
    ) -> impl Stream<Item = Result<Item, ImportError>> + Send + '_ {
        let kinds = Value::from(post_types.to_vec()).to_string();
        sqlx::query_scalar::<_, String>(
            "SELECT record_json FROM items
             WHERE ?1 = '[]' OR kind IN (SELECT value FROM json_each(?1))
             ORDER BY seq",
        )
        .bind(kinds)
        .fetch(&self.pool)
        .map(|row| Ok(serde_json::from_str(&row?)?))
    }

    /// The item with WP id `wp_id` (the first one, should a broken export repeat it).
    pub async fn item(&self, wp_id: i64) -> Result<Option<Item>, ImportError> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT record_json FROM items WHERE wp_id = ?1 ORDER BY seq LIMIT 1",
        )
        .bind(wp_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    /// Whether the step `name` (`parse`, a pass) finished on this file.
    pub async fn pass_done(&self, name: &str) -> Result<bool, ImportError> {
        let row: Option<i64> = sqlx::query_scalar("SELECT 1 FROM passes WHERE name = ?1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    /// How many steps finished on this file.
    pub async fn passes_done(&self) -> Result<u64, ImportError> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM passes")
            .fetch_one(&self.pool)
            .await?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Records that the step `name` finished (again is a no-op). [`flush`](Self::flush)
    /// first: the writer waits for an open [`push`](Self::push) transaction.
    pub async fn mark_pass(&self, name: &str) -> Result<(), ImportError> {
        sqlx::query(
            "INSERT INTO passes (name, done_at) VALUES (?1, datetime('now'))
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Replaces the stored media URL index with `rows` (`path`, media id, public URL).
    pub(crate) async fn set_media_urls(
        &self,
        rows: &[(String, Uuid, String)],
    ) -> Result<(), ImportError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM media_urls")
            .execute(&mut *tx)
            .await?;
        for (path, id, url) in rows {
            sqlx::query("INSERT INTO media_urls (path, media_id, url) VALUES (?1, ?2, ?3)")
                .bind(path)
                .bind(id.to_string())
                .bind(url)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The stored media URL index (`path`, media id, public URL).
    pub(crate) async fn media_urls(&self) -> Result<Vec<(String, Uuid, String)>, ImportError> {
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT path, media_id, url FROM media_urls ORDER BY path")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|(path, id, url)| Ok((path, parse_uuid(&id)?, url)))
            .collect()
    }

    /// Records (or replaces) the id and path of imported items (`wp_id`, id, path).
    pub(crate) async fn put_content_paths(
        &self,
        rows: &[(i64, Uuid, String)],
    ) -> Result<(), ImportError> {
        let mut tx = self.pool.begin().await?;
        for (wp_id, id, path) in rows {
            sqlx::query(
                "INSERT INTO content_paths (wp_id, item_id, path) VALUES (?1, ?2, ?3)
                 ON CONFLICT (wp_id) DO UPDATE SET item_id = excluded.item_id, path = excluded.path",
            )
            .bind(wp_id)
            .bind(id.to_string())
            .bind(path)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Every recorded item (`wp_id`, id, path).
    pub(crate) async fn content_paths(&self) -> Result<Vec<(i64, Uuid, String)>, ImportError> {
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT wp_id, item_id, path FROM content_paths ORDER BY wp_id")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|(wp_id, id, path)| Ok((wp_id, parse_uuid(&id)?, path)))
            .collect()
    }

    /// Queues items for the fix-up (again is a no-op).
    pub(crate) async fn add_fixups(&self, wp_ids: &[i64]) -> Result<(), ImportError> {
        let mut tx = self.pool.begin().await?;
        for wp_id in wp_ids {
            sqlx::query("INSERT INTO fixups (wp_id) VALUES (?1) ON CONFLICT (wp_id) DO NOTHING")
                .bind(wp_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The items queued for the fix-up, by WP id.
    pub(crate) async fn fixups(&self) -> Result<Vec<i64>, ImportError> {
        Ok(
            sqlx::query_scalar("SELECT wp_id FROM fixups ORDER BY wp_id")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// The `meta` value of `key`.
    pub(crate) async fn meta(&self, key: &str) -> Result<Option<String>, ImportError> {
        Ok(sqlx::query_scalar("SELECT value FROM meta WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Sets the `meta` value of `key`.
    pub(crate) async fn set_meta(&self, key: &str, value: &str) -> Result<(), ImportError> {
        sqlx::query(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// An id stored in the work file.
fn parse_uuid(text: &str) -> Result<Uuid, ImportError> {
    Uuid::parse_str(text).map_err(|_| ImportError::Missing {
        what: "a valid id in the work file",
        offset: 0,
    })
}

fn json<T: Serialize>(value: &T) -> Result<String, ImportError> {
    Ok(serde_json::to_string(value)?)
}
