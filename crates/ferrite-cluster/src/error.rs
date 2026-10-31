use std::time::Duration;

use ferrite_domain::NodeId;
use ferrite_store::StoreError;

/// Errors from setting up cluster components.
#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Two job handlers registered the same kind.
    #[error("two job handlers for kind {0:?}")]
    DuplicateHandler(String),
    /// `schema_meta` has no `site_id` (the database was never migrated).
    #[error("the database has no site_id in schema_meta; run `ferrite migrate up`")]
    MissingSiteId,
    /// A live node registered with another `site_id` (P15): `schema_meta` was replaced
    /// under running nodes, e.g. by a restore.
    #[error(
        "live node {node:?} ({id}) belongs to site {theirs}, but this database is site {ours}; \
         the database was replaced under running nodes (e.g. a restore): stop every node, \
         then start them again"
    )]
    SiteMismatch {
        node: String,
        id: NodeId,
        theirs: String,
        ours: String,
    },
    /// A live node serves another `site.url` (P15), usually a wrong `database.schema` or
    /// `search_path`.
    #[error(
        "live node {node:?} ({id}) serves site.url {theirs}, but this node is configured with \
         {ours}; check database.schema (or the search_path), or stop the old nodes before \
         changing site.url (a stopped node goes stale after 30 s)"
    )]
    SiteUrlMismatch {
        node: String,
        id: NodeId,
        theirs: String,
        ours: String,
    },
    /// Solo mode and another node was still live when the wait ran out (P15). `age` is the
    /// time since its last heartbeat, by the database clock.
    #[error(
        "node {name:?} is still running (last heartbeat {age:?} ago); solo mode allows one \
         node per site, so stop that node first"
    )]
    SoloNodeLive { name: String, age: Duration },
    /// A [`Periodic`](crate::Periodic) whose offset is not below its period: its `run_at`
    /// would fall in a later period under this period's key (checked when the scheduler
    /// is built).
    #[error("periodic job {kind:?}: offset {offset:?} is not below its period {every:?}")]
    PeriodicOffset {
        kind: &'static str,
        offset: Duration,
        every: Duration,
    },
}
