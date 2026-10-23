//! `LISTEN/NOTIFY` (04 §4, P2): per-schema channel names and the listening side.
//! The sending side is `pg_notify` inside the writing statement (`repos/event.rs`,
//! `repos/job.rs`), so a notification goes out exactly when the write commits.

use async_trait::async_trait;
use ferrite_store::{Channel, Notice, StoreError, Subscription, validate_text};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};

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
    listener: PgListener,
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
    let mut listener = PgListener::connect_with(&pool).await.map_err(map_db_err)?;
    // Reconnect (and re-LISTEN) before reporting the loss, so nothing committed after
    // `Reconnected` is returned can be missed.
    listener.eager_reconnect(true);
    listener.listen(channel).await.map_err(map_db_err)?;
    Ok(Box::new(PgSubscription { listener }))
}

#[async_trait]
impl Subscription for PgSubscription {
    async fn recv(&mut self) -> Result<Notice, StoreError> {
        match self.listener.try_recv().await.map_err(map_db_err)? {
            Some(_) => Ok(Notice::Notified),
            None => Ok(Notice::Reconnected),
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
}
