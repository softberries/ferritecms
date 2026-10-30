//! The clock and cluster checks of `ferrite doctor`: `clock.db_skew`, `clock.peers`,
//! `cluster.versions` and `cluster.site_url` (P52, P53). They need Postgres: SQLite uses the
//! local clock, and its data-folder lock keeps every other process out, so a SQLite site
//! has no peers. Only the node registry is read.

use std::time::Duration;

use ferrite_cluster::{MAX_CLOCK_SKEW, STALE_AFTER, clock_skew};
use ferrite_config::Config;
use ferrite_domain::{Node, Timestamp};
use serde_json::Value;

use super::Finding;
use super::db::{Database, unavailable};

/// Above this the skew is an error: TOTP accepts one 30 s step either side (P52).
const MAX_TOTP_SKEW: Duration = Duration::from_secs(30);
/// The detail of a cluster check on SQLite.
const SOLO_SQLITE: &str = "solo SQLite site: no other node can share the database";

/// The node registry as doctor read it: the live nodes (heartbeat within
/// [`STALE_AFTER`] by the database clock), newest heartbeat first.
pub(super) enum Registry {
    /// No Postgres to read (SQLite, or the database did not open).
    None(&'static str),
    /// Reading the registry failed.
    Failed(String),
    Live(Vec<Node>),
}

impl Registry {
    /// Whether a live node is named `name`.
    pub(super) fn has_node(&self, name: &str) -> bool {
        matches!(self, Registry::Live(nodes) if nodes.iter().any(|n| n.name == name))
    }

    /// Whether the registry was read.
    pub(super) fn was_read(&self) -> bool {
        matches!(self, Registry::Live(_))
    }
}

/// Reads the live nodes from Postgres.
pub(super) async fn registry(db: Option<&Database>, config: Option<&Config>) -> Registry {
    let Some(db) = db else {
        return Registry::None(unavailable(config));
    };
    let Some(store) = db.postgres() else {
        return Registry::None(SOLO_SQLITE);
    };
    let nodes = store.read().nodes();
    let read =
        async { Ok::<_, ferrite_store::StoreError>((nodes.list().await?, nodes.db_now().await?)) };
    match read.await {
        Ok((all, now)) => Registry::Live(
            all.into_iter()
                .filter(|n| {
                    // A heartbeat in the future (by the database clock) is age zero.
                    let age = now.as_micros().saturating_sub(n.heartbeat_at.as_micros());
                    Duration::from_micros(u64::try_from(age).unwrap_or(0)) <= STALE_AFTER
                })
                .collect(),
        ),
        Err(e) => Registry::Failed(format!("cannot read the node registry: {e}")),
    }
}

/// `clock.db_skew` (P52): this machine's clock against Postgres' `now()`, measured at the
/// midpoint of the round trip; over 2 s a warning, over 30 s an error.
pub(super) async fn db_skew(db: Option<&Database>, config: Option<&Config>) -> Finding {
    const ID: &str = "clock.db_skew";
    let Some(db) = db else {
        return Finding::skip(ID, unavailable(config));
    };
    let Some(store) = db.postgres() else {
        return Finding::skip(ID, "SQLite uses this machine's clock: nothing to compare");
    };
    let before = Timestamp::now();
    let now = match store.read().nodes().db_now().await {
        Ok(now) => now,
        Err(e) => return Finding::warn(ID, format!("cannot read the database clock: {e}")),
    };
    let skew = clock_skew(before, now, Timestamp::now());
    let abs = u128::from(skew.unsigned_abs());
    let side = if skew >= 0 { "ahead of" } else { "behind" };
    let detail = format!("this machine's clock is {abs} ms {side} the database's");
    if abs > MAX_TOTP_SKEW.as_millis() {
        Finding::error(
            ID,
            format!(
                "{detail}: over {} s, authenticator codes (TOTP) fail; fix the time sync (NTP)",
                MAX_TOTP_SKEW.as_secs()
            ),
        )
    } else if abs > MAX_CLOCK_SKEW.as_millis() {
        Finding::warn(
            ID,
            format!(
                "{detail}: over {} s; check the time sync (NTP)",
                MAX_CLOCK_SKEW.as_secs()
            ),
        )
    } else {
        Finding::ok(ID, detail)
    }
}

/// The live nodes, or the finding that replaces the check.
fn live<'a>(id: &'static str, registry: &'a Registry) -> Result<&'a [Node], Finding> {
    match registry {
        Registry::None(why) => Err(Finding::skip(id, *why)),
        Registry::Failed(e) => Err(Finding::warn(id, e.clone())),
        Registry::Live(nodes) if nodes.is_empty() => {
            Err(Finding::info(id, "no live node in the registry"))
        }
        Registry::Live(nodes) => Ok(nodes),
    }
}

/// `clock.peers` (P52): live nodes whose recorded `clock_skew_ms` is over 2 s.
pub(super) fn peers(registry: &Registry) -> Finding {
    const ID: &str = "clock.peers";
    let nodes = match live(ID, registry) {
        Ok(nodes) => nodes,
        Err(finding) => return finding,
    };
    let skewed: Vec<String> = nodes
        .iter()
        .filter_map(|n| {
            let ms = n.meta.get("clock_skew_ms").and_then(Value::as_i64)?;
            (u128::from(ms.unsigned_abs()) > MAX_CLOCK_SKEW.as_millis())
                .then(|| format!("{} ({ms:+} ms)", n.name))
        })
        .collect();
    if skewed.is_empty() {
        Finding::ok(
            ID,
            format!(
                "{} live node(s), each within {} s of the database clock",
                nodes.len(),
                MAX_CLOCK_SKEW.as_secs()
            ),
        )
    } else {
        Finding::warn(
            ID,
            format!(
                "clock more than {} s off the database's on {}: check their time sync (NTP)",
                MAX_CLOCK_SKEW.as_secs(),
                skewed.join(", ")
            ),
        )
    }
}

/// `cluster.versions`: the live nodes' binary and schema versions; a mixed set is a
/// warning (normal while a rolling upgrade runs, not after it).
pub(super) fn versions(registry: &Registry) -> Finding {
    const ID: &str = "cluster.versions";
    let nodes = match live(ID, registry) {
        Ok(nodes) => nodes,
        Err(finding) => return finding,
    };
    let mut kinds: Vec<(&str, i64)> = nodes
        .iter()
        .map(|n| (n.version.as_str(), n.schema_version))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    if let [(version, schema)] = kinds.as_slice() {
        return Finding::ok(
            ID,
            format!(
                "{} live node(s) on ferrite {version} (schema {schema})",
                nodes.len()
            ),
        );
    }
    let each: Vec<String> = nodes
        .iter()
        .map(|n| {
            format!(
                "{} ferrite {} (schema {})",
                n.name, n.version, n.schema_version
            )
        })
        .collect();
    Finding::warn(
        ID,
        format!(
            "mixed versions: {} (expected during a rolling upgrade; finish it)",
            each.join(", ")
        ),
    )
}

/// `cluster.site_url`: live nodes record the `site.url` and mode they run with; one that
/// differs from this configuration is a warning (a node refuses to join such a cluster,
/// P15).
pub(super) fn site_url(registry: &Registry, config: Option<&Config>) -> Finding {
    const ID: &str = "cluster.site_url";
    let nodes = match live(ID, registry) {
        Ok(nodes) => nodes,
        Err(finding) => return finding,
    };
    let Some(config) = config else {
        return Finding::skip(ID, super::fs::NO_CONFIG);
    };
    // As `serve` registers them: the URL without a trailing slash, the resolved mode.
    let url = config.site.url.as_str().trim_end_matches('/');
    let mode = crate::node::node_mode(config.effective_cluster_mode()).as_str();
    let mut differ = Vec::new();
    for n in nodes {
        let theirs = |key: &str| n.meta.get(key).and_then(Value::as_str);
        if let Some(theirs) = theirs("site_url")
            && theirs != url
        {
            differ.push(format!("{} has site_url {theirs}", n.name));
        }
        if let Some(theirs) = theirs("mode")
            && theirs != mode
        {
            differ.push(format!("{} runs in {theirs} mode", n.name));
        }
    }
    if differ.is_empty() {
        Finding::ok(
            ID,
            format!(
                "{} live node(s) agree with this configuration ({url}, {mode} mode)",
                nodes.len()
            ),
        )
    } else {
        Finding::warn(
            ID,
            format!(
                "{} (this configuration: {url}, {mode} mode)",
                differ.join("; ")
            ),
        )
    }
}
