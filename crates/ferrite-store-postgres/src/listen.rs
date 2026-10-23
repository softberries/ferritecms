//! `LISTEN/NOTIFY` (04 §4, P2): per-schema channel names and the listening side.
//! The sending side is `pg_notify` inside the writing statement (`repos/event.rs`,
//! `repos/job.rs`), so a notification goes out exactly when the write commits.

use async_trait::async_trait;
use ferrite_store::{Channel, Notice, StoreError, Subscription, validate_text};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgListener, PgPoolOptions},
};

use crate::exec::map_db_err;

/// Longest `database.schema`: `ferrite_events_` (15 bytes) + 48 = 63, Postgres'
/// identifier limit (`NAMEDATALEN - 1`); a longer channel name would be truncated.
pub(crate) const MAX_SCHEMA_LEN: usize = 48;

/// The channel names of one site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Channels {
    events: String,
    jobs: String,
}

impl Channels {
    /// `ferrite_events` / `ferrite_jobs` without a schema, `ferrite_events_<schema>` /
    /// `ferrite_jobs_<schema>` with one (channels are database-wide, sites are schemas).
    /// The schema is ASCII-lowercased, which keeps its byte length, so the name matches
    /// whether or not a client quotes the `LISTEN` identifier (sqlx 0.9 does). A schema
    /// longer than [`MAX_SCHEMA_LEN`] bytes or containing NUL is `Invalid`.
    pub(crate) fn new(schema: Option<&str>) -> Result<Self, StoreError> {
        let Some(schema) = schema else {
            return Ok(Self {
                events: "ferrite_events".into(),
                jobs: "ferrite_jobs".into(),
            });
        };
        validate_text("database schema", schema)?;
        if schema.len() > MAX_SCHEMA_LEN {
            return Err(StoreError::Invalid(format!(
                "database schema is {} bytes; at most {MAX_SCHEMA_LEN}, so the notification \
                 channel name fits Postgres' 63-byte limit",
                schema.len()
            )));
        }
        let suffix = schema.to_ascii_lowercase();
        Ok(Self {
            events: format!("ferrite_events_{suffix}"),
            jobs: format!("ferrite_jobs_{suffix}"),
        })
    }

    pub(crate) fn name(&self, channel: Channel) -> &str {
        match channel {
            Channel::Events => &self.events,
            Channel::Jobs => &self.jobs,
        }
    }
}

/// A `LISTEN` on its own one-connection pool (the pool only serves reconnects).
struct PgSubscription {
    pool: PgPool,
    channel: String,
    /// `None` after `recv` failed: the next `recv` rebuilds the listener and reports
    /// `Reconnected`, because notifications may have been lost meanwhile.
    listener: Option<PgListener>,
}

/// Opens a subscription: `connect` must carry no `search_path` or `statement_timeout`
/// (the connection sits idle in `LISTEN` indefinitely).
pub(crate) async fn subscribe(
    connect: PgConnectOptions,
    channel: &str,
) -> Result<Box<dyn Subscription>, StoreError> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_with(connect)
        .await
        .map_err(map_db_err)?;
    Ok(Box::new(PgSubscription::open(pool, channel).await?))
}

impl PgSubscription {
    async fn open(pool: PgPool, channel: &str) -> Result<Self, StoreError> {
        let listener = listen(&pool, channel).await?;
        Ok(Self {
            pool,
            channel: channel.to_owned(),
            listener: Some(listener),
        })
    }
}

/// A listener on `pool` that has run `LISTEN channel`.
async fn listen(pool: &PgPool, channel: &str) -> Result<PgListener, StoreError> {
    let mut listener = PgListener::connect_with(pool).await.map_err(map_db_err)?;
    // Reconnect (and re-LISTEN) before reporting the loss, so nothing committed after
    // `Reconnected` is returned can be missed.
    listener.eager_reconnect(true);
    listener.listen(channel).await.map_err(map_db_err)?;
    Ok(listener)
}

#[async_trait]
impl Subscription for PgSubscription {
    async fn recv(&mut self) -> Result<Notice, StoreError> {
        let Some(listener) = self.listener.as_mut() else {
            // The previous call failed: rebuild; until that succeeds, keep failing.
            self.listener = Some(listen(&self.pool, &self.channel).await?);
            return Ok(Notice::Reconnected);
        };
        match listener.try_recv().await {
            Ok(Some(_)) => Ok(Notice::Notified),
            Ok(None) => Ok(Notice::Reconnected),
            Err(e) => {
                // Whatever state sqlx left it in, start over on the next call.
                self.listener = None;
                Err(map_db_err(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names() {
        let none = Channels::new(None).unwrap();
        assert_eq!(none.name(Channel::Events), "ferrite_events");
        assert_eq!(none.name(Channel::Jobs), "ferrite_jobs");

        let mixed = Channels::new(Some("T_Ab")).unwrap();
        assert_eq!(mixed.name(Channel::Events), "ferrite_events_t_ab");
        assert_eq!(mixed.name(Channel::Jobs), "ferrite_jobs_t_ab");

        let max = "s".repeat(MAX_SCHEMA_LEN);
        let fits = Channels::new(Some(&max)).unwrap();
        assert_eq!(fits.name(Channel::Events).len(), 63);
        assert!(fits.name(Channel::Jobs).len() <= 63);

        for bad in ["s".repeat(MAX_SCHEMA_LEN + 1), "a\0b".to_owned()] {
            assert!(
                matches!(Channels::new(Some(&bad)), Err(StoreError::Invalid(_))),
                "{bad:?}"
            );
        }
    }

    /// A failed `recv` (the reconnect after a lost connection fails) is followed by a
    /// rebuild that reports `Reconnected`, and the rebuilt listener hears notifications.
    #[tokio::test]
    async fn recv_after_error_rebuilds_and_reports_reconnected() {
        use std::{
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
            time::Duration,
        };

        let Ok(url) = std::env::var("FERRITE_TEST_PG_URL") else {
            if std::env::var("CI").as_deref() == Ok("true") {
                panic!("FERRITE_TEST_PG_URL must be set in CI");
            }
            eprintln!("SKIP: Postgres test skipped (FERRITE_TEST_PG_URL is not set)");
            return;
        };
        let app = format!("t_{}:listen", uuid::Uuid::now_v7().simple());
        let channel = format!("ferrite_events_t_{}", uuid::Uuid::now_v7().simple());
        let refuse = Arc::new(AtomicBool::new(false));
        let gate = refuse.clone();
        let connect = std::str::FromStr::from_str(&url)
            .map(|o: PgConnectOptions| o.application_name(&app))
            .unwrap();
        // New connections fail while `refuse` is set (an unreachable server).
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(2))
            .after_connect(move |_conn, _meta| {
                let refuse = gate.load(Ordering::SeqCst);
                Box::pin(async move {
                    if refuse {
                        return Err(sqlx::Error::Protocol("refused by test".into()));
                    }
                    Ok(())
                })
            })
            .connect_with(connect)
            .await
            .unwrap();
        let mut sub = PgSubscription::open(pool, &channel).await.unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();

        refuse.store(true, Ordering::SeqCst);
        let killed: Vec<bool> = sqlx::query_scalar(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = $1",
        )
        .bind(&app)
        .fetch_all(&admin)
        .await
        .unwrap();
        assert_eq!(killed, [true]);
        let first = tokio::time::timeout(Duration::from_secs(10), sub.recv())
            .await
            .unwrap();
        assert!(first.is_err(), "{first:?}");
        assert!(sub.listener.is_none());
        // Still unreachable: the rebuild fails and is retried on the next call.
        assert!(sub.recv().await.is_err());

        refuse.store(false, Ordering::SeqCst);
        assert_eq!(sub.recv().await.unwrap(), Notice::Reconnected);
        sqlx::query("SELECT pg_notify($1, '1')")
            .bind(&channel)
            .execute(&admin)
            .await
            .unwrap();
        let next = tokio::time::timeout(Duration::from_secs(2), sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next, Notice::Notified);
        admin.close().await;
    }
}
