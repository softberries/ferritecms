//! The database checks of `ferrite doctor`: `db.connect`, `db.schema` and `db.identity`
//! (P49, P50, P53). Nothing is created or written:
//!
//! - **SQLite** is read over [`SqliteReadOnly`]: no data-folder lock, no writer, no
//!   migration, so it works while `serve` holds the store and never blocks a start.
//! - **Postgres** is first reached with a bare connection that reads the server version
//!   and checks that the configured schema exists; only then is the store opened (its open
//!   would create a missing schema).

use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrite_config::{Config, DbKind};
use ferrite_store::{
    Compat, MigrationStatus, SCHEMA_META_INSTALL_ID, SCHEMA_META_LAST_BINARY, Store,
};
use ferrite_store_postgres::{PgOptions, PgStore};
use sqlx::Connection;

use super::Finding;
use super::fs::NO_CONFIG;
use crate::open::{MissingWalIndex, SqliteReadOnly};

/// How long reaching Postgres (connect, version and schema queries, the store's pool) may
/// take, and then each later database read: not a network check (P51), but it must not
/// hang doctor.
const PG_TIMEOUT: Duration = Duration::from_secs(10);
/// The oldest Postgres the store opens (`server_version_num`).
const MIN_PG_SERVER: i32 = 150_000;
/// `application_name` of doctor's Postgres connections.
const APP_NAME: &str = "ferrite:doctor";
/// The detail of a database check when `db.connect` failed.
pub(super) const NO_DATABASE: &str = "the database did not open (see db.connect)";

/// The site's database, opened read-only.
pub(super) enum Database {
    Sqlite(SqliteReadOnly),
    /// The store, and its URL (only to scrub it, and its password, out of error text).
    Postgres {
        store: Arc<dyn Store>,
        url: String,
    },
}

impl Database {
    /// Runs one database read, bounded by [`PG_TIMEOUT`]; the error is a finding's detail
    /// (Postgres: with the URL and its password scrubbed out).
    pub(super) async fn read<T>(
        &self,
        read: impl std::future::Future<Output = anyhow::Result<T>>,
    ) -> Result<T, String> {
        match tokio::time::timeout(PG_TIMEOUT, read).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(self.clean(&format!("{e:#}"))),
            Err(_) => Err(format!(
                "the database did not answer within {} s",
                PG_TIMEOUT.as_secs()
            )),
        }
    }

    /// `message` made safe to print.
    fn clean(&self, message: &str) -> String {
        match self {
            Database::Sqlite(_) => message.to_owned(),
            Database::Postgres { url, .. } => scrub(message, url),
        }
    }

    async fn migration_status(&self, binary: &semver::Version) -> Result<MigrationStatus, String> {
        self.read(async {
            match self {
                Database::Sqlite(db) => db.migration_status(binary).await,
                Database::Postgres { store, .. } => Ok(store.migration_status(binary).await?),
            }
        })
        .await
    }

    /// A `schema_meta` value.
    pub(super) async fn meta(&self, key: &str) -> Result<Option<String>, String> {
        self.read(async {
            match self {
                Database::Sqlite(db) => db.schema_meta(key).await,
                Database::Postgres { store, .. } => Ok(store.read().schema_meta().get(key).await?),
            }
        })
        .await
    }

    /// The Postgres store (the node registry and the database clock live there).
    pub(super) fn postgres(&self) -> Option<&dyn Store> {
        match self {
            Database::Sqlite(_) => None,
            Database::Postgres { store, .. } => Some(store.as_ref()),
        }
    }
}

/// The detail of a check that needs the database.
pub(super) fn unavailable(config: Option<&Config>) -> &'static str {
    if config.is_some() {
        NO_DATABASE
    } else {
        NO_CONFIG
    }
}

/// `db.connect`: opens the database read-only and reports backend, latency and (Postgres)
/// the server version. `lock`: the SQLite data-folder lock's state.
pub(super) async fn connect(
    config: Option<&Config>,
    lock: Option<super::fs::Lock>,
) -> (Finding, Option<Database>) {
    const ID: &str = "db.connect";
    let Some(config) = config else {
        return (Finding::skip(ID, NO_CONFIG), None);
    };
    match config.database_kind() {
        DbKind::Sqlite { path } => {
            let started = Instant::now();
            let db = match SqliteReadOnly::open(&path).await {
                Ok(db) => db,
                Err(e) if e.downcast_ref::<MissingWalIndex>().is_some() => {
                    return (Finding::warn(ID, format!("{e:#}")), None);
                }
                Err(e) => return (Finding::error(ID, format!("{e:#}")), None),
            };
            // One read, as the latency probe.
            if let Err(e) = db.schema_meta("site_id").await {
                return (
                    Finding::error(ID, format!("{}: {e:#}", path.display())),
                    None,
                );
            }
            let how = if lock == Some(super::fs::Lock::Held) {
                "read-only, beside the process that holds ferrite.lock"
            } else if db.immutable() {
                "read-only (no -wal file: opened immutable)"
            } else {
                "read-only"
            };
            let finding = Finding::ok(
                ID,
                format!(
                    "SQLite {}, {how}, {} ms",
                    path.display(),
                    started.elapsed().as_millis()
                ),
            );
            (finding, Some(Database::Sqlite(db)))
        }
        DbKind::Postgres { url } => {
            let schema = config.database.schema.as_deref();
            match tokio::time::timeout(PG_TIMEOUT, postgres(url.expose(), schema)).await {
                Ok(Ok((detail, store))) => (Finding::ok(ID, detail), Some(store)),
                Ok(Err(detail)) => (Finding::error(ID, detail), None),
                Err(_) => (
                    Finding::error(
                        ID,
                        format!(
                            "{}: no answer within {} s",
                            pg_target(url.expose(), schema),
                            PG_TIMEOUT.as_secs()
                        ),
                    ),
                    None,
                ),
            }
        }
    }
}

/// Reaches Postgres (P49: the schema must exist before the store is opened). The error is
/// a finding's detail, with the URL and its password scrubbed out.
async fn postgres(url: &str, schema: Option<&str>) -> Result<(String, Database), String> {
    let target = pg_target(url, schema);
    let fail = |e: &dyn std::fmt::Display| format!("{target}: {}", scrub(&e.to_string(), url));
    let options = sqlx::postgres::PgConnectOptions::from_str(url)
        .map_err(|_| format!("{target}: database.url is not a valid postgres:// URL"))?
        .application_name(APP_NAME);
    let started = Instant::now();
    let mut conn = sqlx::PgConnection::connect_with(&options)
        .await
        .map_err(|e| fail(&e))?;
    let (num, version): (i32, String) = sqlx::query_as(
        "SELECT current_setting('server_version_num')::int, current_setting('server_version')",
    )
    .fetch_one(&mut conn)
    .await
    .map_err(|e| fail(&e))?;
    let exists = match schema {
        Some(schema) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)",
        )
        .bind(schema)
        .fetch_one(&mut conn)
        .await
        .map_err(|e| fail(&e))?,
        None => true,
    };
    let latency = started.elapsed();
    let _ = conn.close().await;
    if num < MIN_PG_SERVER {
        return Err(format!(
            "{target}: Postgres {version} is too old (ferrite needs 15 or newer)"
        ));
    }
    if !exists {
        return Err(format!(
            "{target}: the schema {} does not exist; doctor does not create it (`ferrite \
             init --db … --schema …` or `ferrite migrate up` does)",
            schema.unwrap_or_default()
        ));
    }
    let store = PgStore::open(PgOptions {
        url: url.to_owned(),
        max_connections: 2,
        schema: schema.map(str::to_owned),
        application_name: APP_NAME.to_owned(),
    })
    .await
    .map_err(|e| fail(&e))?;
    Ok((
        format!(
            "Postgres {target}, server {version}, {} ms",
            latency.as_millis()
        ),
        Database::Postgres {
            store: Arc::new(store),
            url: url.to_owned(),
        },
    ))
}

/// `host:port/database (schema S)` of a Postgres URL: never the user or the password.
fn pg_target(url: &str, schema: Option<&str>) -> String {
    let place = match url::Url::parse(url) {
        Ok(u) => {
            let host = u
                .host_str()
                .filter(|h| !h.is_empty())
                .unwrap_or("localhost");
            let port = u.port().unwrap_or(5432);
            let db = u.path().trim_start_matches('/');
            if db.is_empty() {
                format!("{host}:{port}")
            } else {
                format!("{host}:{port}/{db}")
            }
        }
        Err(_) => "the configured server".to_owned(),
    };
    match schema {
        Some(s) => format!("{place} (schema {s})"),
        None => place,
    }
}

/// `message` with the URL and its password (as written and percent-decoded) replaced.
fn scrub(message: &str, url: &str) -> String {
    let mut out = message.replace(url, "<database.url>");
    if let Ok(u) = url::Url::parse(url)
        && let Some(password) = u.password().filter(|p| !p.is_empty())
    {
        out = out.replace(password, "***");
        if let Ok(decoded) = percent_decode(password) {
            out = out.replace(&decoded, "***");
        }
    }
    out
}

/// Percent-decodes `s` (a URL's password).
fn percent_decode(s: &str) -> Result<String, std::string::FromUtf8Error> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out)
}

/// `db.schema`: the migration status, as `migrate status` judges it; `true` when the
/// schema is incompatible (exit code 3).
pub(super) async fn schema(db: Option<&Database>, config: Option<&Config>) -> (Finding, bool) {
    const ID: &str = "db.schema";
    let Some(db) = db else {
        return (Finding::skip(ID, unavailable(config)), false);
    };
    let binary = match crate::binary_version() {
        Ok(v) => v,
        Err(e) => return (Finding::error(ID, format!("{e:#}")), false),
    };
    let status = match db.migration_status(&binary).await {
        Ok(status) => status,
        Err(e) => return (Finding::error(ID, e), false),
    };
    let (db_v, bin_v) = (status.db_version, status.binary_version);
    let unknown = if status.unknown_applied.is_empty() || status.verdict == Compat::NewerSchemaOk {
        String::new()
    } else {
        format!(
            "; applied migrations this binary does not know: {:?}",
            status.unknown_applied
        )
    };
    match &status.verdict {
        Compat::UpToDate => (
            Finding::ok(ID, format!("schema {db_v}, up to date{unknown}")),
            false,
        ),
        Compat::NewerSchemaOk => (
            Finding::ok(
                ID,
                format!("schema {db_v} is newer than this binary's {bin_v} and compatible with it"),
            ),
            false,
        ),
        Compat::NeedsUp => (
            Finding::warn(
                ID,
                format!(
                    "{} pending migration(s) (schema {db_v}, this binary {bin_v}): run \
                     `ferrite migrate up` (`serve` applies them at start when \
                     database.auto_migrate is on){unknown}",
                    status.pending.len()
                ),
            ),
            false,
        ),
        Compat::Incompatible { requires } => (
            Finding::error(
                ID,
                format!(
                    "incompatible: schema {db_v} requires ferrite >= {requires} (this is \
                     {binary}); upgrade this binary, or run `ferrite migrate down --to \
                     {bin_v}` with the newer one"
                ),
            ),
            true,
        ),
    }
}

/// `db.identity`: `site_id` present (error), `install_id` present (warning), and the
/// binary that last started here not newer than this one (warning).
pub(super) async fn identity(db: Option<&Database>, config: Option<&Config>) -> Finding {
    const ID: &str = "db.identity";
    let Some(db) = db else {
        return Finding::skip(ID, unavailable(config));
    };
    let read = async {
        Ok::<_, String>((
            db.meta("site_id").await?,
            db.meta(SCHEMA_META_INSTALL_ID).await?,
            db.meta(SCHEMA_META_LAST_BINARY).await?,
        ))
    };
    let (site, install, last) = match read.await {
        Ok(values) => values,
        Err(e) => return Finding::error(ID, e),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut facts = Vec::new();
    match site.as_deref().filter(|s| !s.is_empty()) {
        Some(site) => facts.push(format!("site {site}")),
        None => errors.push(
            "schema_meta has no site_id: nodes refuse to start without one (restore the \
             database from a backup)"
                .to_owned(),
        ),
    }
    match install.as_deref().filter(|s| !s.is_empty()) {
        Some(install) => facts.push(format!("install {install}")),
        None => warnings.push(
            "schema_meta has no install_id: the media GC deletes nothing until it writes one \
             on its next pass"
                .to_owned(),
        ),
    }
    match last.as_deref() {
        None => facts.push("no start recorded yet".to_owned()),
        Some(raw) => match (semver::Version::parse(raw), crate::binary_version()) {
            (Ok(last), Ok(binary)) if last > binary => warnings.push(format!(
                "the site was last started by ferrite {last}, newer than this binary \
                 ({binary}): run the newer binary, or see `ferrite rollback`"
            )),
            (Ok(last), _) => facts.push(format!("last started by ferrite {last}")),
            (Err(_), _) => warnings.push(format!(
                "schema_meta.{SCHEMA_META_LAST_BINARY} is not a version: {raw:?}"
            )),
        },
    }
    if !errors.is_empty() {
        errors.extend(warnings);
        Finding::error(ID, errors.join("; "))
    } else if !warnings.is_empty() {
        Finding::warn(ID, warnings.join("; "))
    } else {
        Finding::ok(ID, facts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_target_shows_no_credentials() {
        let url = "postgres://user:s3cret@db.example.com:6543/site?sslmode=require";
        let shown = pg_target(url, Some("blog"));
        assert_eq!(shown, "db.example.com:6543/site (schema blog)");
        assert_eq!(pg_target("postgres://h/d", None), "h:5432/d");
    }

    #[test]
    fn scrub_removes_the_url_and_the_password() {
        let url = "postgres://user:p%40ss@h/d";
        let msg = format!("failed for {url}: p@ss and p%40ss");
        let out = scrub(&msg, url);
        assert!(!out.contains("p@ss") && !out.contains("p%40ss"), "{out}");
        assert!(out.contains("<database.url>"), "{out}");
    }
}
