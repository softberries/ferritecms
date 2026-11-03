//! `webhooks.endpoints` (M6-6, design 18 §10.3): the registered endpoints, read-only.
//! `warn` when one was disabled automatically (`failing`, `gone`), has been failing for
//! more than 24 hours, or is paused on this install (a copied or restored database), and
//! when `[webhooks] local_targets` is on. Names and hosts only: a URL's path or query can
//! hold a token.

use ferrite_config::Config;
use ferrite_core::webhooks::host_of;
use ferrite_domain::Timestamp;
use ferrite_store::SCHEMA_META_INSTALL_ID;

use super::Finding;
use super::db::{Database, unavailable};

/// Failing longer than this is a warning.
const FAILING_WARN_MICROS: i64 = 24 * 3600 * 1_000_000;

/// One endpoint as the check needs it.
struct Endpoint {
    name: String,
    host: String,
    enabled: bool,
    reason: Option<String>,
    install_id: String,
    failing_since: Option<i64>,
}

async fn endpoints(db: &Database) -> Result<Vec<Endpoint>, String> {
    match db {
        Database::Sqlite(ro) => {
            let rows = db.read(async { ro.webhook_endpoints().await }).await?;
            Ok(rows
                .into_iter()
                .map(
                    |(name, url, enabled, reason, install_id, failing)| Endpoint {
                        name,
                        host: host_of(&url),
                        enabled,
                        reason,
                        install_id,
                        failing_since: failing,
                    },
                )
                .collect())
        }
        Database::Postgres { store, .. } => {
            // A schema before `0016_webhooks` has no table: nothing to report.
            let rows = match db
                .read(async { Ok(store.read().webhooks().list_endpoints().await?) })
                .await
            {
                Ok(rows) => rows,
                Err(e) if e.contains("webhook_endpoints") && e.contains("does not exist") => {
                    Vec::new()
                }
                Err(e) => return Err(e),
            };
            Ok(rows
                .into_iter()
                .map(|e| Endpoint {
                    host: host_of(&e.url),
                    name: e.name,
                    enabled: e.enabled,
                    reason: e.disabled_reason.map(|r| r.as_str().to_owned()),
                    install_id: e.install_id,
                    failing_since: e.failing_since.map(|t| t.as_micros()),
                })
                .collect())
        }
    }
}

fn label(e: &Endpoint) -> String {
    let name: String = e
        .name
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    format!("\"{name}\" ({})", e.host)
}

/// See the module docs.
pub(super) async fn endpoints_check(db: Option<&Database>, config: Option<&Config>) -> Finding {
    const ID: &str = "webhooks.endpoints";
    let Some(db) = db else {
        return Finding::skip(ID, unavailable(config));
    };
    let list = match endpoints(db).await {
        Ok(list) => list,
        Err(e) => return Finding::error(ID, e),
    };
    let install = match db.meta(SCHEMA_META_INSTALL_ID).await {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => return Finding::error(ID, e),
    };
    let now = Timestamp::now().as_micros();
    let mut warnings = Vec::new();
    if config.is_some_and(|c| c.webhooks.local_targets) {
        warnings.push(
            "[webhooks] local_targets is on: webhooks may call http and private addresses \
             (development only)"
                .to_owned(),
        );
    }
    for e in &list {
        match (e.enabled, e.reason.as_deref()) {
            (false, Some(r @ ("failing" | "gone"))) => {
                warnings.push(format!("{} was disabled automatically ({r})", label(e)));
            }
            (true, _) if e.install_id != install => warnings.push(format!(
                "{} is paused: this database was copied or restored; enable it to send \
                 from this site",
                label(e)
            )),
            (true, _)
                if e.failing_since
                    .is_some_and(|since| now.saturating_sub(since) > FAILING_WARN_MICROS) =>
            {
                warnings.push(format!(
                    "{} has been failing for more than 24 hours",
                    label(e)
                ));
            }
            _ => {}
        }
    }
    if !warnings.is_empty() {
        return Finding::warn(ID, warnings.join("; "));
    }
    if list.is_empty() {
        return Finding::ok(ID, "none");
    }
    let active = list
        .iter()
        .filter(|e| e.enabled && e.install_id == install)
        .count();
    Finding::ok(ID, format!("{} endpoints, {active} active", list.len()))
}
