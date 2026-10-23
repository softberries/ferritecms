//! The node registry (04 §3, rulings P14, P15): joining checks the other live nodes, a
//! heartbeat task keeps this node's row fresh, and `leave` removes it on clean shutdown.
//!
//! Liveness is judged by the database clock only: a node is live while its
//! `heartbeat_at >= db_now - STALE_AFTER`. A restarted process registers under a new id,
//! so its predecessor's row simply goes stale (and is pruned by `nodes.prune` after
//! [`NODE_EXPIRY`]).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ferrite_domain::{NewNode, Node, NodeId, Timestamp};
use ferrite_store::{Store, StoreError};
use serde_json::{Map, Value, json};
use tokio::{sync::Notify, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

/// How often a node refreshes its row.
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(10);
/// A node whose last heartbeat is older than this is stale (3 × heartbeat, 02 §4).
pub const STALE_AFTER: Duration = Duration::from_secs(30);
/// Node rows are pruned this long after their last heartbeat (`nodes.prune`).
pub const NODE_EXPIRY: Duration = Duration::from_secs(24 * 60 * 60);
/// Clock skew against the database above this is warned about (04 §3).
pub const MAX_CLOCK_SKEW: Duration = Duration::from_secs(2);
/// How long a solo node waits for another live node to go stale before refusing to start
/// (a crashed predecessor goes stale after [`STALE_AFTER`]).
pub const SOLO_WAIT: Duration = Duration::from_secs(35);

/// Slack added when sleeping until a node goes stale, so the re-check sees it stale.
const STALE_MARGIN: Duration = Duration::from_millis(10);

/// The resolved cluster mode (`cluster.mode = "auto"` already decided).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterMode {
    Solo,
    Cluster,
}

impl ClusterMode {
    /// The value stored in `nodes.meta.mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Solo => "solo",
            Self::Cluster => "cluster",
        }
    }
}

/// Who this node is, as registered in `nodes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeIdentity {
    /// A fresh id per process start.
    pub id: NodeId,
    /// See [`node_name`].
    pub name: String,
    /// The binary's version.
    pub version: semver::Version,
    /// The highest migration this binary knows (`MigrationStatus::binary_version`).
    pub schema_version: i64,
    pub mode: ClusterMode,
    /// `site.url` as configured; compared verbatim with other live nodes (P15).
    pub site_url: String,
}

/// The node name (P14): `configured` (`cluster.node_name`) when set and not empty, else
/// `$HOSTNAME-<pid>`, else `node-<pid>`.
pub fn node_name(configured: Option<&str>) -> String {
    let hostname = std::env::var("HOSTNAME").ok();
    name_from(configured, hostname.as_deref(), std::process::id())
}

fn name_from(configured: Option<&str>, hostname: Option<&str>, pid: u32) -> String {
    match (configured, hostname) {
        (Some(name), _) if !name.is_empty() => name.to_owned(),
        (_, Some(host)) if !host.is_empty() => format!("{host}-{pid}"),
        _ => format!("node-{pid}"),
    }
}

/// Timings for [`Registry::join_with`]; the default is the production constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinTimings {
    /// See [`STALE_AFTER`].
    pub stale_after: Duration,
    /// See [`SOLO_WAIT`].
    pub solo_wait: Duration,
}

impl Default for JoinTimings {
    fn default() -> Self {
        Self {
            stale_after: STALE_AFTER,
            solo_wait: SOLO_WAIT,
        }
    }
}

/// Clock skew in milliseconds, local minus database: the database time is compared with
/// the midpoint of the local round trip that read it (P14).
pub fn clock_skew(local_before: Timestamp, db: Timestamp, local_after: Timestamp) -> i64 {
    let before = local_before.as_micros();
    let after = local_after.as_micros();
    let mid = before.saturating_add(after.saturating_sub(before) / 2);
    mid.saturating_sub(db.as_micros()) / 1000
}

/// Time since `heartbeat_at` by the database clock (zero if it is in the future).
fn age(db_now: Timestamp, heartbeat_at: Timestamp) -> Duration {
    let micros = db_now.as_micros().saturating_sub(heartbeat_at.as_micros());
    Duration::from_micros(u64::try_from(micros).unwrap_or(0))
}

fn skew_too_large(skew_ms: i64) -> bool {
    skew_ms.unsigned_abs() > MAX_CLOCK_SKEW.as_millis() as u64
}

/// Reads the database clock and measures the skew against it.
async fn measure(store: &dyn Store) -> Result<(Timestamp, i64), StoreError> {
    let before = Timestamp::now();
    let db = store.read().nodes().db_now().await?;
    let after = Timestamp::now();
    Ok((db, clock_skew(before, db, after)))
}

/// P15: refuses a live node of another site or another `site.url`. A node whose meta
/// lacks the key gives no evidence either way and is not refused.
fn check_site(live: &[&Node], site_id: &str, site_url: &str) -> Result<(), crate::ClusterError> {
    for n in live {
        if let Some(theirs) = n.meta.get("site_id").and_then(Value::as_str)
            && theirs != site_id
        {
            return Err(crate::ClusterError::SiteMismatch {
                node: n.name.clone(),
                id: n.id,
                theirs: theirs.to_owned(),
                ours: site_id.to_owned(),
            });
        }
        if let Some(theirs) = n.meta.get("site_url").and_then(Value::as_str)
            && theirs != site_url
        {
            return Err(crate::ClusterError::SiteUrlMismatch {
                node: n.name.clone(),
                id: n.id,
                theirs: theirs.to_owned(),
                ours: site_url.to_owned(),
            });
        }
    }
    Ok(())
}

/// This node's membership in the registry. Cheap to clone.
#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

struct Inner {
    store: Arc<dyn Store>,
    me: NodeIdentity,
    site_id: String,
    /// Whether the last measured skew was above [`MAX_CLOCK_SKEW`] (warn on crossings).
    skew_over: AtomicBool,
    wake: Notify,
}

impl Registry {
    /// [`Registry::join_with`] with the production timings.
    pub async fn join(
        store: Arc<dyn Store>,
        me: NodeIdentity,
    ) -> Result<Registry, crate::ClusterError> {
        Self::join_with(store, me, JoinTimings::default()).await
    }

    /// Validates the other live nodes and registers this one (04 §3, P15):
    ///
    /// - a live node with another `site_id` → [`SiteMismatch`](crate::ClusterError::SiteMismatch);
    /// - a live node with another `site_url` → [`SiteUrlMismatch`](crate::ClusterError::SiteUrlMismatch);
    /// - in solo mode, any other live node is waited for until it goes stale, up to
    ///   `timings.solo_wait`, then [`SoloNodeLive`](crate::ClusterError::SoloNodeLive).
    ///
    /// Then the clock skew is measured (warned above [`MAX_CLOCK_SKEW`]) and the row is
    /// registered with `meta = {site_id, site_url, mode, clock_skew_ms}`. Two solo nodes
    /// starting in the same instant can both pass the check; on SQLite the data-folder lock
    /// already prevents that.
    pub async fn join_with(
        store: Arc<dyn Store>,
        me: NodeIdentity,
        timings: JoinTimings,
    ) -> Result<Registry, crate::ClusterError> {
        let site_id = store
            .read()
            .schema_meta()
            .get("site_id")
            .await?
            .ok_or(crate::ClusterError::MissingSiteId)?;
        let deadline = Instant::now() + timings.solo_wait;
        let skew = loop {
            let (db, skew) = measure(store.as_ref()).await?;
            let nodes = store.read().nodes().list().await?;
            let live: Vec<&Node> = nodes
                .iter()
                .filter(|n| n.id != me.id && age(db, n.heartbeat_at) <= timings.stale_after)
                .collect();
            check_site(&live, &site_id, &me.site_url)?;
            let blocker = live.iter().max_by_key(|n| n.heartbeat_at);
            let (ClusterMode::Solo, Some(blocker)) = (me.mode, blocker) else {
                break skew;
            };
            let age = age(db, blocker.heartbeat_at);
            let now = Instant::now();
            if now >= deadline {
                return Err(crate::ClusterError::SoloNodeLive {
                    name: blocker.name.clone(),
                    age,
                });
            }
            tracing::info!(
                node = %blocker.name,
                ?age,
                "solo mode: another node is live; waiting for it to stop heartbeating"
            );
            let to_stale = timings.stale_after.saturating_sub(age) + STALE_MARGIN;
            tokio::time::sleep(to_stale.min(deadline - now)).await;
        };
        let over = skew_too_large(skew);
        if over {
            tracing::warn!(
                clock_skew_ms = skew,
                "this node's clock differs from the database clock by more than 2 s; \
                 leases and schedules use the database clock, but check NTP"
            );
        }
        let registry = Registry {
            inner: Arc::new(Inner {
                store,
                me,
                site_id,
                skew_over: AtomicBool::new(over),
                wake: Notify::new(),
            }),
        };
        let meta = registry.inner.meta(skew, Value::Null);
        registry
            .inner
            .store
            .read()
            .nodes()
            .register(&registry.inner.new_node(meta))
            .await?;
        tracing::info!(
            node = %registry.inner.me.name,
            id = %registry.inner.me.id,
            mode = registry.inner.me.mode.as_str(),
            "node registered"
        );
        Ok(registry)
    }

    /// This node's id.
    pub fn id(&self) -> NodeId {
        self.inner.me.id
    }

    /// Spawns the heartbeat: every [`HEARTBEAT_EVERY`] (or at once after
    /// [`Registry::beat_now`]) it re-measures the clock skew and writes `meta()` merged
    /// with the registry's own keys (`site_id`, `site_url`, `mode`, `clock_skew_ms`, which
    /// `meta()` cannot override; a non-object `meta()` is ignored). A heartbeat that finds
    /// no row (pruned or deleted) registers the node again. Failures are logged and retried
    /// at the next beat; each beat is bounded by [`HEARTBEAT_EVERY`]. Ends on `stop`; stop
    /// it and await the handle before [`Registry::leave`], or a late beat re-registers.
    pub fn spawn_heartbeat(
        &self,
        meta: Arc<dyn Fn() -> Value + Send + Sync>,
        stop: CancellationToken,
    ) -> JoinHandle<()> {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval_at(Instant::now() + HEARTBEAT_EVERY, HEARTBEAT_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => break,
                    () = inner.wake.notified() => tick.reset(),
                    _ = tick.tick() => {}
                }
                tokio::select! {
                    biased;
                    () = stop.cancelled() => break,
                    r = tokio::time::timeout(HEARTBEAT_EVERY, inner.beat(meta.as_ref())) => {
                        match r {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => tracing::warn!(error = %e, "node heartbeat failed"),
                            Err(_) => tracing::warn!("node heartbeat timed out"),
                        }
                    }
                }
            }
        })
    }

    /// Wakes the heartbeat task to beat now (after a reload changed the node's meta).
    pub fn beat_now(&self) {
        self.inner.wake.notify_one();
    }

    /// Removes this node's row (clean shutdown); a missing row is fine.
    pub async fn leave(&self) -> Result<(), StoreError> {
        self.inner
            .store
            .read()
            .nodes()
            .deregister(self.inner.me.id)
            .await
    }
}

impl Inner {
    fn new_node(&self, meta: Value) -> NewNode {
        NewNode {
            id: self.me.id,
            name: self.me.name.clone(),
            version: self.me.version.to_string(),
            schema_version: self.me.schema_version,
            meta,
        }
    }

    /// `extra` (if an object) plus the registry's own keys, which win.
    fn meta(&self, skew_ms: i64, extra: Value) -> Value {
        let mut map = match extra {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            other => {
                tracing::warn!(meta = %other, "node meta is not a JSON object; ignored");
                Map::new()
            }
        };
        for (key, value) in [
            ("site_id", json!(self.site_id)),
            ("site_url", json!(self.me.site_url)),
            ("mode", json!(self.me.mode.as_str())),
            ("clock_skew_ms", json!(skew_ms)),
        ] {
            map.insert(key.to_owned(), value);
        }
        Value::Object(map)
    }

    /// Logs only when the skew crosses [`MAX_CLOCK_SKEW`] (either sign), not every beat.
    fn note_skew(&self, skew_ms: i64) {
        let over = skew_too_large(skew_ms);
        let was = self.skew_over.swap(over, Ordering::Relaxed);
        if over && !was {
            tracing::warn!(
                clock_skew_ms = skew_ms,
                "this node's clock now differs from the database clock by more than 2 s"
            );
        } else if was && !over {
            tracing::info!(clock_skew_ms = skew_ms, "clock skew is back within 2 s");
        }
    }

    async fn beat(&self, extra: &(dyn Fn() -> Value + Send + Sync)) -> Result<(), StoreError> {
        let (_, skew) = measure(self.store.as_ref()).await?;
        self.note_skew(skew);
        let meta = self.meta(skew, extra());
        let nodes = self.store.read().nodes();
        if !nodes.heartbeat(self.me.id, &meta).await? {
            tracing::info!(id = %self.me.id, "node row is gone; registering again");
            nodes.register(&self.new_node(meta)).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        Timestamp::from(
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc),
        )
    }

    #[test]
    fn clock_skew_midpoint() {
        let before = at("2026-10-23T10:00:00.000Z");
        let after = at("2026-10-23T10:00:00.200Z");
        assert_eq!(
            clock_skew(before, at("2026-10-23T09:59:57.000Z"), after),
            3100
        );
        // Symmetric: the database is ahead by the same amount.
        assert_eq!(
            clock_skew(before, at("2026-10-23T10:00:03.200Z"), after),
            -3100
        );
        assert_eq!(clock_skew(before, at("2026-10-23T10:00:00.100Z"), after), 0);
        assert!(skew_too_large(3100) && skew_too_large(-3100));
        assert!(!skew_too_large(2000) && !skew_too_large(-2000));
    }

    #[test]
    fn node_names() {
        assert_eq!(name_from(Some("web-1"), Some("host"), 7), "web-1");
        assert_eq!(name_from(Some(""), Some("host"), 7), "host-7");
        assert_eq!(name_from(None, Some("host"), 7), "host-7");
        assert_eq!(name_from(None, Some(""), 7), "node-7");
        assert_eq!(name_from(None, None, 7), "node-7");
    }

    #[test]
    fn constants() {
        assert_eq!(HEARTBEAT_EVERY, Duration::from_secs(10));
        assert_eq!(STALE_AFTER, 3 * HEARTBEAT_EVERY);
        assert_eq!(NODE_EXPIRY, Duration::from_secs(86_400));
        assert_eq!(MAX_CLOCK_SKEW, Duration::from_secs(2));
        assert_eq!(SOLO_WAIT, Duration::from_secs(35));
        assert_eq!(
            JoinTimings::default(),
            JoinTimings {
                stale_after: STALE_AFTER,
                solo_wait: SOLO_WAIT
            }
        );
    }

    #[test]
    fn ages() {
        let db = at("2026-10-23T10:00:31.000Z");
        assert_eq!(
            age(db, at("2026-10-23T10:00:00.000Z")),
            Duration::from_secs(31)
        );
        assert_eq!(age(db, at("2026-10-23T10:00:40.000Z")), Duration::ZERO);
    }
}
