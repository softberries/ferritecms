//! Connection setup: one pool, `statement_timeout`, optional `search_path` schema (02 §6).

use std::{str::FromStr, sync::Arc};

use ferrite_store::StoreError;
use sqlx::{
    AssertSqlSafe, Connection, PgConnection, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use crate::{PgStore, exec::map_db_err, repos::PgRepos};

/// `statement_timeout` on application connections.
const STATEMENT_TIMEOUT: &str = "30s";

/// How to open a [`PgStore`].
#[derive(Debug, Clone)]
pub struct PgOptions {
    /// `postgres://` URL; `sslmode` in it is honoured.
    pub url: String,
    /// Pool size (default 16).
    pub max_connections: u32,
    /// Schema for every table (`search_path`); created if missing. `None`: server default.
    pub schema: Option<String>,
}

impl PgOptions {
    /// Defaults for the given URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: 16,
            schema: None,
        }
    }
}

/// Quotes an SQL identifier (`"` doubled), so any schema name is safe to interpolate.
pub(crate) fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

impl PgStore {
    /// Creates the schema if set and missing, then opens the pool. Does not migrate.
    pub async fn open(opts: PgOptions) -> Result<Self, StoreError> {
        let connect = PgConnectOptions::from_str(&opts.url).map_err(map_db_err)?;
        let search_path_sql = opts
            .schema
            .as_deref()
            .map(|s| format!("SET search_path TO {}", quote_ident(s)));
        if let Some(schema) = &opts.schema {
            let mut conn = PgConnection::connect_with(&connect)
                .await
                .map_err(map_db_err)?;
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
            conn.close().await.map_err(map_db_err)?;
        }

        let session: Arc<str> = match &search_path_sql {
            Some(sp) => format!("{sp}; SET statement_timeout = '{STATEMENT_TIMEOUT}'"),
            None => format!("SET statement_timeout = '{STATEMENT_TIMEOUT}'"),
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

        Ok(Self {
            read: PgRepos::new(pool.clone()),
            pool,
            connect,
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
    use super::quote_ident;

    #[test]
    fn quote_ident_escapes_quotes() {
        assert_eq!(quote_ident("t_1"), "\"t_1\"");
        assert_eq!(quote_ident("a\"; DROP"), "\"a\"\"; DROP\"");
    }
}
