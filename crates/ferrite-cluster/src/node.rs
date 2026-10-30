//! The node registry (04 §3, rulings P14, P15): joining checks the other live nodes, a
//! heartbeat task keeps this node's row fresh, and `leave` removes it on clean shutdown.
//!
//! Liveness is judged by the database clock only: a node is live while its
//! `heartbeat_at >= db_now - STALE_AFTER`. A restarted process registers under a new id,
//! so its predecessor's row simply goes stale (and is pruned by `nodes.prune` after
//! [`NODE_EXPIRY`]).
//!
//! **Solo mode allows one live node per site**, but it is enforced at join only. A node
//! that loses its database link for longer than [`STALE_AFTER`] goes stale while still
//! running; a replacement may then join, and when the first node's link comes back both
//! are live. The heartbeat detects that and logs an error (once per transition) on both
//! nodes; it does not exit, since stopping one of them is the operator's decision.
//!
//! **A process that owns its database** ([`NodeIdentity::owns_database`]: SQLite, where
//! the data-folder lock keeps every other process out) skips all of that: any other row
//! was left by a dead process (a crash, `kill -9`), so the join neither waits for it nor
//! compares its `site_id`/`site.url`, and deletes it; the heartbeat does not look for
//! other live nodes either (ruling R17). The checks above apply where several processes
//! share the database (Postgres, in either mode).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ferrite_domain::{NewNode, Node, NodeId, Timestamp};
use ferrite_store::{LockGuard, Store, StoreError, lock};
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
/// After a skew warning, "back within" is logged only once the skew is below this
/// (hysteresis, so a skew hovering around 2 s does not log every beat).
pub const CLOCK_SKEW_RECOVERED: Duration = Duration::from_millis(1500);
/// How long a solo node waits for another live node to go stale before refusing to start
/// (a crashed predecessor goes stale after [`STALE_AFTER`]).
pub const SOLO_WAIT: Duration = Duration::from_secs(35);

/// Held for the final check-and-register pass of a join (R7), so two nodes joining at
/// once are checked one after the other.
pub const LOCK_JOIN: &str = "cluster.join";
/// How long a join waits for [`LOCK_JOIN`]; the pass under it is a few statements.
pub const JOIN_LOCK_WAIT: Duration = Duration::from_secs(10);
/// Longest node name, in bytes (the same cap as `cluster.node_name`).
pub const MAX_NODE_NAME: usize = 64;

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
    /// No other process can use this database while this one runs (SQLite: the caller
    /// holds the data-folder lock). Other rows are then from dead processes; see the
    /// module docs.
    pub owns_database: bool,
}

/// The node name (P14): `configured` (`cluster.node_name`) when set and not blank, else
/// `$HOSTNAME-<pid>`, else `node-<pid>`. Trimmed and cut to [`MAX_NODE_NAME`] bytes (the
/// hostname is cut, so the pid survives).
pub fn node_name(configured: Option<&str>) -> String {
    let hostname = std::env::var("HOSTNAME").ok();
    name_from(configured, hostname.as_deref(), std::process::id())
}

fn name_from(configured: Option<&str>, hostname: Option<&str>, pid: u32) -> String {
    let configured = configured.map(str::trim).filter(|s| !s.is_empty());
    let hostname = hostname.map(str::trim).filter(|s| !s.is_empty());
    match (configured, hostname) {
        (Some(name), _) => truncate(name, MAX_NODE_NAME).to_owned(),
        (None, Some(host)) => {
            let suffix = format!("-{pid}");
            let host = truncate(host, MAX_NODE_NAME.saturating_sub(suffix.len()));
            format!("{host}{suffix}")
        }
        (None, None) => format!("node-{pid}"),
    }
}

/// `s` cut to at most `max` bytes on a character boundary.
fn truncate(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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

/// Whether a node whose last heartbeat was `heartbeat_at` is live at `db_now` (both by the
/// database clock): its heartbeat is at most `stale_after` old ([`STALE_AFTER`] unless the
/// timings say otherwise); a heartbeat in the future counts as age zero.
pub fn is_live(db_now: Timestamp, heartbeat_at: Timestamp, stale_after: Duration) -> bool {
    age(db_now, heartbeat_at) <= stale_after
}

/// Time since `heartbeat_at` by the database clock (zero if it is in the future).
fn age(db_now: Timestamp, heartbeat_at: Timestamp) -> Duration {
    let micros = db_now.as_micros().saturating_sub(heartbeat_at.as_micros());
    Duration::from_micros(u64::try_from(micros).unwrap_or(0))
}

/// A change of a logged condition (clock skew, two live solo nodes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Crossing {
    Unchanged,
    Entered,
    Left,
}

/// Skew state with hysteresis: enter above [`MAX_CLOCK_SKEW`], leave below
/// [`CLOCK_SKEW_RECOVERED`] (either sign). Returns the new state and the crossing.
fn skew_state(was_over: bool, skew_ms: i64) -> (bool, Crossing) {
    let abs = u128::from(skew_ms.unsigned_abs());
    if !was_over && abs > MAX_CLOCK_SKEW.as_millis() {
        (true, Crossing::Entered)
    } else if was_over && abs < CLOCK_SKEW_RECOVERED.as_millis() {
        (false, Crossing::Left)
    } else {
        (was_over, Crossing::Unchanged)
    }
}

fn flag_crossing(was: bool, now: bool) -> Crossing {
    match (was, now) {
        (false, true) => Crossing::Entered,
        (true, false) => Crossing::Left,
        _ => Crossing::Unchanged,
    }
}

/// Reads the database clock and measures the skew against it.
async fn measure(store: &dyn Store) -> Result<(Timestamp, i64), StoreError> {
    let before = Timestamp::now();
    let db = store.read().nodes().db_now().await?;
    let after = Timestamp::now();
    Ok((db, clock_skew(before, db, after)))
}

/// Other nodes live at `db` (heartbeat within `stale_after`).
fn live_others(nodes: &[Node], me: NodeId, db: Timestamp, stale_after: Duration) -> Vec<&Node> {
    nodes
        .iter()
        .filter(|n| n.id != me && is_live(db, n.heartbeat_at, stale_after))
        .collect()
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

/// One look at the registry during a join.
struct Survey {
    skew: i64,
    /// [`NodeIdentity::owns_database`] only: every other row (left by dead processes).
    dead: Vec<NodeId>,
    /// Solo mode only: the most recently beating other live node, with its age.
    blocker: Option<(String, Duration)>,
    /// Names of other live nodes registered in solo mode.
    solo_peers: Vec<String>,
}

async fn survey(
    store: &dyn Store,
    me: &NodeIdentity,
    site_id: &str,
    stale_after: Duration,
) -> Result<Survey, crate::ClusterError> {
    let (db, skew) = measure(store).await?;
    let nodes = store.read().nodes().list().await?;
    if me.owns_database {
        return Ok(Survey {
            skew,
            dead: nodes
                .iter()
                .map(|n| n.id)
                .filter(|&id| id != me.id)
                .collect(),
            blocker: None,
            solo_peers: Vec::new(),
        });
    }
    let live = live_others(&nodes, me.id, db, stale_after);
    check_site(&live, site_id, &me.site_url)?;
    let blocker = match me.mode {
        ClusterMode::Solo => live
            .iter()
            .max_by_key(|n| n.heartbeat_at)
            .map(|n| (n.name.clone(), age(db, n.heartbeat_at))),
        ClusterMode::Cluster => None,
    };
    let solo_peers = live
        .iter()
        .filter(|n| n.meta.get("mode").and_then(Value::as_str) == Some("solo"))
        .map(|n| n.name.clone())
        .collect();
    Ok(Survey {
        skew,
        dead: Vec::new(),
        blocker,
        solo_peers,
    })
}

async fn release(guard: Box<dyn LockGuard>) {
    if let Err(e) = guard.release().await {
        tracing::warn!(error = %e, "releasing the join lock failed");
    }
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
    stale_after: Duration,
    /// Skew state for the hysteresis in [`skew_state`].
    skew_over: AtomicBool,
    /// Solo mode: whether another live node was seen at the last beat.
    dual_live: AtomicBool,
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
    ///   `timings.solo_wait`, then [`SoloNodeLive`](crate::ClusterError::SoloNodeLive);
    /// - in cluster mode, a live node registered in solo mode is warned about;
    /// - with [`NodeIdentity::owns_database`], none of the above: the other rows are
    ///   deleted instead (see the module docs).
    ///
    /// The waiting happens without any lock. Then one final pass (list, checks, register)
    /// runs under [`LOCK_JOIN`] (R7, both modes), so two nodes joining at once are checked
    /// one after the other; the lock is never held while sleeping: a blocker seen in that
    /// pass releases it and goes back to waiting. The row is registered with
    /// `meta = {site_id, site_url, mode, clock_skew_ms}`; skew above [`MAX_CLOCK_SKEW`]
    /// is warned about. An empty `site_id` counts as missing.
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
            .filter(|s| !s.is_empty())
            .ok_or(crate::ClusterError::MissingSiteId)?;
        let deadline = Instant::now() + timings.solo_wait;
        let skew = loop {
            Self::wait_until_clear(store.as_ref(), &me, &site_id, timings, deadline).await?;
            let guard = lock(store.as_ref(), LOCK_JOIN, JOIN_LOCK_WAIT).await?;
            let pass = Self::final_pass(store.as_ref(), &me, &site_id, timings).await;
            release(guard).await;
            if let Some(skew) = pass? {
                break skew;
            }
        };
        let (over, crossing) = skew_state(false, skew);
        if crossing == Crossing::Entered {
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
                stale_after: timings.stale_after,
                skew_over: AtomicBool::new(over),
                dual_live: AtomicBool::new(false),
                wake: Notify::new(),
            }),
        };
        tracing::info!(
            node = %registry.inner.me.name,
            id = %registry.inner.me.id,
            mode = registry.inner.me.mode.as_str(),
            "node registered"
        );
        Ok(registry)
    }

    /// Unlocked: returns once no other live node blocks this one (solo mode), or fails.
    async fn wait_until_clear(
        store: &dyn Store,
        me: &NodeIdentity,
        site_id: &str,
        timings: JoinTimings,
        deadline: Instant,
    ) -> Result<(), crate::ClusterError> {
        loop {
            let s = survey(store, me, site_id, timings.stale_after).await?;
            let Some((name, age)) = s.blocker else {
                return Ok(());
            };
            let now = Instant::now();
            if now >= deadline {
                return Err(crate::ClusterError::SoloNodeLive { name, age });
            }
            tracing::info!(
                node = %name,
                ?age,
                "solo mode: another node is live; waiting for it to stop heartbeating"
            );
            let to_stale = timings.stale_after.saturating_sub(age) + STALE_MARGIN;
            tokio::time::sleep(to_stale.min(deadline - now)).await;
        }
    }

    /// Under [`LOCK_JOIN`]: checks again and registers. `None` when a blocker appeared.
    async fn final_pass(
        store: &dyn Store,
        me: &NodeIdentity,
        site_id: &str,
        timings: JoinTimings,
    ) -> Result<Option<i64>, crate::ClusterError> {
        let s = survey(store, me, site_id, timings.stale_after).await?;
        if s.blocker.is_some() {
            return Ok(None);
        }
        if me.mode == ClusterMode::Cluster && !s.solo_peers.is_empty() {
            tracing::warn!(
                nodes = ?s.solo_peers,
                "joining in cluster mode while live nodes run in solo mode; \
                 give every node the same cluster.mode"
            );
        }
        for &id in &s.dead {
            store.read().nodes().deregister(id).await?;
        }
        if !s.dead.is_empty() {
            tracing::info!(
                rows = s.dead.len(),
                "removed node rows left by earlier processes (this process owns the database)"
            );
        }
        let meta = base_meta(site_id, me, s.skew, Value::Null);
        store.read().nodes().register(&new_node(me, meta)).await?;
        Ok(Some(s.skew))
    }

    /// This node's id.
    pub fn id(&self) -> NodeId {
        self.inner.me.id
    }

    /// Spawns the heartbeat: every [`HEARTBEAT_EVERY`] (or at once after
    /// [`Registry::beat_now`]) it re-measures the clock skew and writes `meta()` merged
    /// with the registry's own keys (`site_id`, `site_url`, `mode`, `clock_skew_ms`, which
    /// `meta()` cannot override; a non-object `meta()` is ignored). A heartbeat that finds
    /// no row (pruned or deleted) registers the node again. In solo mode on a shared
    /// database each beat also lists the nodes and logs an error when another node is
    /// live (R8; see the module docs).
    /// Failures are logged and retried at the next beat; each beat is bounded by
    /// [`HEARTBEAT_EVERY`]. Ends on `stop`; stop it and await the handle before
    /// [`Registry::leave`], or a late beat re-registers.
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

fn new_node(me: &NodeIdentity, meta: Value) -> NewNode {
    NewNode {
        id: me.id,
        name: me.name.clone(),
        version: me.version.to_string(),
        schema_version: me.schema_version,
        meta,
    }
}

/// `extra` (if an object) plus the registry's own keys, which win.
fn base_meta(site_id: &str, me: &NodeIdentity, skew_ms: i64, extra: Value) -> Value {
    let mut map = match extra {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => {
            tracing::warn!(meta = %other, "node meta is not a JSON object; ignored");
            Map::new()
        }
    };
    for (key, value) in [
        ("site_id", json!(site_id)),
        ("site_url", json!(me.site_url)),
        ("mode", json!(me.mode.as_str())),
        ("clock_skew_ms", json!(skew_ms)),
    ] {
        map.insert(key.to_owned(), value);
    }
    Value::Object(map)
}

impl Inner {
    /// Logs when the skew crosses the thresholds (hysteresis, either sign); returns the
    /// crossing.
    fn note_skew(&self, skew_ms: i64) -> Crossing {
        let was = self.skew_over.load(Ordering::Relaxed);
        let (over, crossing) = skew_state(was, skew_ms);
        self.skew_over.store(over, Ordering::Relaxed);
        match crossing {
            Crossing::Entered => tracing::warn!(
                clock_skew_ms = skew_ms,
                "this node's clock now differs from the database clock by more than 2 s"
            ),
            Crossing::Left => {
                tracing::info!(clock_skew_ms = skew_ms, "clock skew is back within 1.5 s")
            }
            Crossing::Unchanged => {}
        }
        crossing
    }

    /// Solo mode: logs (once per transition) whether other live nodes exist.
    fn note_solo_peers(&self, others: &[&Node]) -> Crossing {
        let now = !others.is_empty();
        let crossing = flag_crossing(self.dual_live.swap(now, Ordering::Relaxed), now);
        match crossing {
            Crossing::Entered => tracing::error!(
                nodes = ?others.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
                "two live nodes in solo mode: another node of this site is heartbeating \
                 (e.g. this node lost its database link and was replaced); stop one of them"
            ),
            Crossing::Left => tracing::info!("no other live node in solo mode any more"),
            Crossing::Unchanged => {}
        }
        crossing
    }

    async fn beat(&self, extra: &(dyn Fn() -> Value + Send + Sync)) -> Result<(), StoreError> {
        let (db, skew) = measure(self.store.as_ref()).await?;
        self.note_skew(skew);
        let meta = base_meta(&self.site_id, &self.me, skew, extra());
        let nodes = self.store.read().nodes();
        if !nodes.heartbeat(self.me.id, &meta).await? {
            tracing::info!(id = %self.me.id, "node row is gone; registering again");
            nodes.register(&new_node(&self.me, meta)).await?;
        }
        if self.me.mode == ClusterMode::Solo && !self.me.owns_database {
            let all = nodes.list().await?;
            self.note_solo_peers(&live_others(&all, self.me.id, db, self.stale_after));
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
    }

    #[test]
    fn skew_hysteresis() {
        use Crossing::*;
        assert_eq!(skew_state(false, 2000), (false, Unchanged));
        assert_eq!(skew_state(false, 2001), (true, Entered));
        assert_eq!(skew_state(false, -3100), (true, Entered));
        // Between 1.5 s and 2 s: stays over, no log.
        assert_eq!(skew_state(true, 1800), (true, Unchanged));
        assert_eq!(skew_state(true, -1500), (true, Unchanged));
        assert_eq!(skew_state(true, 1499), (false, Left));
        assert_eq!(skew_state(true, -10), (false, Left));
        assert_eq!(skew_state(true, 5000), (true, Unchanged));
        assert_eq!(flag_crossing(false, true), Entered);
        assert_eq!(flag_crossing(true, true), Unchanged);
        assert_eq!(flag_crossing(true, false), Left);
        assert_eq!(flag_crossing(false, false), Unchanged);
    }

    #[tokio::test]
    async fn note_skew_reports_crossings_once() {
        let (store, _dir) = ferrite_store_tests::sqlite_store().await;
        let inner = Inner {
            store: Arc::new(store),
            me: NodeIdentity {
                id: NodeId::new(),
                name: "n".into(),
                version: semver::Version::new(0, 1, 0),
                schema_version: 4,
                mode: ClusterMode::Solo,
                site_url: "https://example.com/".into(),
                owns_database: false,
            },
            site_id: "s".into(),
            stale_after: STALE_AFTER,
            skew_over: AtomicBool::new(false),
            dual_live: AtomicBool::new(false),
            wake: Notify::new(),
        };
        let seq: Vec<Crossing> = [100, 2500, 2600, 1700, 1400, 1600, -2100]
            .into_iter()
            .map(|ms| inner.note_skew(ms))
            .collect();
        use Crossing::*;
        assert_eq!(
            seq,
            [
                Unchanged, Entered, Unchanged, Unchanged, Left, Unchanged, Entered
            ]
        );

        let other = Node {
            id: NodeId::new(),
            name: "other".into(),
            version: "0.1.0".into(),
            schema_version: 4,
            started_at: Timestamp::now(),
            heartbeat_at: Timestamp::now(),
            meta: json!({}),
        };
        assert_eq!(inner.note_solo_peers(&[&other]), Entered);
        assert_eq!(inner.note_solo_peers(&[&other]), Unchanged);
        assert_eq!(inner.note_solo_peers(&[]), Left);
    }

    #[test]
    fn live_filter() {
        let db = at("2026-10-23T10:00:31.000Z");
        let mk = |hb: &str| Node {
            id: NodeId::new(),
            name: hb.into(),
            version: "0.1.0".into(),
            schema_version: 4,
            started_at: at(hb),
            heartbeat_at: at(hb),
            meta: json!({}),
        };
        let nodes = [
            mk("2026-10-23T10:00:00.000Z"),
            mk("2026-10-23T10:00:01.000Z"),
            mk("2026-10-23T10:00:30.000Z"),
        ];
        let me = nodes[2].id;
        let live = live_others(&nodes, me, db, STALE_AFTER);
        assert_eq!(
            live.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["2026-10-23T10:00:01.000Z"],
            "31 s old is stale, exactly 30 s is live, own row excluded"
        );
    }

    #[test]
    fn node_names() {
        assert_eq!(name_from(Some("web-1"), Some("host"), 7), "web-1");
        assert_eq!(name_from(Some(""), Some("host"), 7), "host-7");
        assert_eq!(name_from(None, Some("host"), 7), "host-7");
        assert_eq!(name_from(None, Some(""), 7), "node-7");
        assert_eq!(name_from(None, None, 7), "node-7");
        assert_eq!(name_from(Some("  web-1 \n"), None, 7), "web-1");
        assert_eq!(name_from(Some("   "), Some(" host "), 7), "host-7");
        assert_eq!(name_from(Some(&"x".repeat(80)), None, 7).len(), 64);
        let long = name_from(None, Some(&"é".repeat(40)), 12345);
        assert!(long.len() <= 64 && long.ends_with("-12345"), "{long}");
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

    #[test]
    fn liveness_is_inclusive_at_stale_after() {
        let db = at("2026-10-23T10:00:30.000Z");
        assert!(is_live(db, at("2026-10-23T10:00:00.000Z"), STALE_AFTER));
        assert!(!is_live(db, at("2026-10-23T09:59:59.999Z"), STALE_AFTER));
        assert!(is_live(db, at("2026-10-23T10:00:45.000Z"), STALE_AFTER));
    }
}
