//! Connection setup: one pool, `statement_timeout`, `idle_in_transaction_session_timeout`,
//! optional `search_path` schema (02 §6), `application_name` per connection role.

use std::{str::FromStr, sync::Arc};

use ferrite_store::{StoreError, validate_text};
use sqlx::{
    AssertSqlSafe, Connection, PgConnection, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use crate::{PgStore, exec::map_db_err, listen::Channels, lock::Locks, repos::PgRepos};

/// `statement_timeout` on application connections.
const STATEMENT_TIMEOUT: &str = "30s";

/// `idle_in_transaction_session_timeout` on application connections (P4): well under the
/// invalidation loop's 10-minute late horizon, so no writer holds an allocated event seq
/// uncommitted past it.
const IDLE_IN_TRANSACTION_TIMEOUT: &str = "60s";

/// Oldest supported server (`server_version_num`): 0001 uses `UNIQUE NULLS NOT DISTINCT`.
const MIN_SERVER_VERSION: i32 = 150_000;

/// Rejects servers older than PostgreSQL 15.
fn check_server_version(num: i32) -> Result<(), StoreError> {
    if num < MIN_SERVER_VERSION {
        return Err(StoreError::Unsupported("postgres >= 15 required"));
    }
    Ok(())
}

/// How to open a [`PgStore`].
#[derive(Debug, Clone)]
pub struct PgOptions {
    /// `postgres://` URL; `sslmode` in it is honoured.
    pub url: String,
    /// Pool size (default 16).
    pub max_connections: u32,
    /// Schema for every table (`search_path`); created if missing. `None`: server default.
    /// At most 48 bytes, so the notification channel names fit (P2).
    pub schema: Option<String>,
    /// `application_name` of the store's connections (default `"ferrite"`);
    /// subscriptions use `<application_name>:listen`, named locks
    /// `<application_name>:lock:<name>`.
    pub application_name: String,
}

impl PgOptions {
    /// Defaults for the given URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: 16,
            schema: None,
            application_name: "ferrite".into(),
        }
    }
}

/// Session settings of every application-pool connection.
fn app_session_settings() -> String {
    format!(
        "SET statement_timeout = '{STATEMENT_TIMEOUT}'; \
         SET idle_in_transaction_session_timeout = '{IDLE_IN_TRANSACTION_TIMEOUT}'"
    )
}

/// Quotes an SQL identifier (`"` doubled), so any schema name is safe to interpolate.
pub(crate) fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

impl PgStore {
    /// Creates the schema if set and missing, then opens the pool. Does not migrate.
    /// A server older than PostgreSQL 15 gives `Unsupported("postgres >= 15 required")`;
    /// a schema longer than 48 bytes, or NUL in it or in `application_name`, `Invalid`.
    pub async fn open(opts: PgOptions) -> Result<Self, StoreError> {
        let channels = Channels::new(opts.schema.as_deref())?;
        validate_text("application_name", &opts.application_name)?;
        let base = PgConnectOptions::from_str(&opts.url).map_err(map_db_err)?;
        let listen = base
            .clone()
            .application_name(&format!("{}:listen", opts.application_name));
        let locks = Locks::new(base.clone(), &opts.application_name, opts.schema.as_deref());
        let connect = base.application_name(&opts.application_name);
        let search_path_sql = opts
            .schema
            .as_deref()
            .map(|s| format!("SET search_path TO {}", quote_ident(s)));
        // Version check first, on its own connection, so an unsupported server is
        // rejected before anything (the schema) is created on it.
        let mut conn = PgConnection::connect_with(&connect)
            .await
            .map_err(map_db_err)?;
        let version: i32 = sqlx::query_scalar("SELECT current_setting('server_version_num')::int")
            .fetch_one(&mut conn)
            .await
            .map_err(map_db_err)?;
        check_server_version(version)?;
        if let Some(schema) = &opts.schema {
            // Only CREATE when missing, so an existing schema needs no CREATE privilege.
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)")
                    .bind(schema)
                    .fetch_one(&mut conn)
                    .await
                    .map_err(map_db_err)?;
            if !exists {
                sqlx::raw_sql(AssertSqlSafe(format!(
                    "CREATE SCHEMA IF NOT EXISTS {}",
                    quote_ident(schema)
                )))
                .execute(&mut conn)
                .await
                .map_err(map_db_err)?;
            }
        }
        conn.close().await.map_err(map_db_err)?;

        let session: Arc<str> = match &search_path_sql {
            Some(sp) => format!("{sp}; {}", app_session_settings()),
            None => app_session_settings(),
        }
        .into();
        let pool = PgPoolOptions::new()
            .max_connections(opts.max_connections.max(1))
            .after_connect(move |conn, _meta| {
                let session = session.clone();
                Box::pin(async move {
                    sqlx::raw_sql(AssertSqlSafe(session)).execute(conn).await?;
                    Ok(())
                })
            })
            .connect_with(connect.clone())
            .await
            .map_err(map_db_err)?;
        let channels = Arc::new(channels);
        Ok(Self {
            read: PgRepos::new(pool.clone(), channels.clone()),
            pool,
            connect,
            listen,
            channels,
            locks,
            search_path_sql,
        })
    }

    /// A dedicated one-connection pool for migrations: same schema, no
    /// `statement_timeout`. (A pool rather than a bare connection because sqlx's
    /// `Acquire` for `&mut PgConnection` is not general enough for a `Send` future.)
    pub(crate) async fn migration_pool(&self) -> Result<PgPool, StoreError> {
        let search_path: Option<Arc<str>> = self.search_path_sql.as_deref().map(Arc::from);
        PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    if let Some(sql) = search_path {
                        sqlx::raw_sql(AssertSqlSafe(sql)).execute(conn).await?;
                    }
                    Ok(())
                })
            })
            .connect_with(self.connect.clone())
            .await
            .map_err(map_db_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_version_must_be_15_or_newer() {
        assert!(matches!(
            check_server_version(140_011),
            Err(StoreError::Unsupported("postgres >= 15 required"))
        ));
        check_server_version(150_000).unwrap();
        check_server_version(170_011).unwrap();
    }

    #[tokio::test]
    async fn idle_in_transaction_timeout_is_set() {
        let Ok(url) = std::env::var("FERRITE_TEST_PG_URL") else {
            // Same rule as the contract-test harness: skipping is not allowed in CI.
            if std::env::var("CI").as_deref() == Ok("true") {
                panic!("FERRITE_TEST_PG_URL must be set in CI");
            }
            eprintln!("SKIP: Postgres test skipped (FERRITE_TEST_PG_URL is not set)");
            return;
        };
        let mut opts = PgOptions::new(url);
        opts.max_connections = 1;
        let store = PgStore::open(opts).await.unwrap();
        let (idle, statement, app): (String, String, String) = sqlx::query_as(
            "SELECT current_setting('idle_in_transaction_session_timeout'), \
             current_setting('statement_timeout'), current_setting('application_name')",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let shown: String = sqlx::query_scalar("SHOW idle_in_transaction_session_timeout")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        store.pool.close().await;
        assert_eq!(shown, "1min");
        assert_eq!(idle, "1min");
        assert_eq!(statement, "30s");
        assert_eq!(app, "ferrite");
    }

    #[tokio::test]
    async fn open_rejects_long_schema_before_connecting() {
        let mut opts = PgOptions::new("postgres://nobody@127.0.0.1:1/none");
        opts.schema = Some("s".repeat(49));
        assert!(matches!(
            PgStore::open(opts).await,
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn quote_ident_escapes_quotes() {
        assert_eq!(quote_ident("t_1"), "\"t_1\"");
        assert_eq!(quote_ident("a\"; DROP"), "\"a\"\"; DROP\"");
    }
}
