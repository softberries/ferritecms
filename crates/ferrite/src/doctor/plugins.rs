//! `plugins.trust` (M6-2, design 14 §9): the installed plugins against `[plugins]
//! signatures`, keys revoked under installed plugins, and the registry's last check.
//! Database only (no network): it runs with `--offline` too.

use ferrite_config::{Config, RegistryCheck};
use ferrite_domain::Timestamp;
use ferrite_domain::plugin_trust::SignaturePolicy;

use super::Finding;
use super::db::{Database, unavailable};

/// A registry checked daily whose last successful check is older than this warns.
const STALE_AFTER_MICROS: i64 = 3 * 24 * 3600 * 1_000_000;

/// One installed plugin's trust facts.
pub(crate) struct PluginFacts {
    pub(crate) name: String,
    /// `reg:…` or `key:…`; `None` unsigned (or installed before M6-2).
    pub(crate) owner: Option<String>,
    pub(crate) signer: Option<String>,
}

/// One key row.
pub(crate) struct KeyFacts {
    pub(crate) key_id: String,
    pub(crate) source: String,
    pub(crate) revoked: bool,
}

/// The cached index of the configured registry.
pub(crate) struct RegistryFacts {
    pub(crate) serial: i64,
    pub(crate) checked_at: Timestamp,
    pub(crate) last_error: Option<String>,
}

/// What the check reads.
pub(crate) struct TrustFacts {
    pub(crate) plugins: Vec<PluginFacts>,
    pub(crate) keys: Vec<KeyFacts>,
    pub(crate) registry: Option<RegistryFacts>,
}

fn ago(now: Timestamp, at: Timestamp) -> String {
    let secs = (now.as_micros() - at.as_micros()).max(0) / 1_000_000;
    match secs {
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 2 * 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

/// See the module docs.
pub(super) async fn trust(db: Option<&Database>, config: Option<&Config>) -> Finding {
    const ID: &str = "plugins.trust";
    let (Some(db), Some(config)) = (db, config) else {
        return Finding::skip(ID, unavailable(config));
    };
    let trust = &config.plugins.trust;
    let target = match crate::registry::configured(trust) {
        Ok(t) => t,
        Err(e) => return Finding::error(ID, format!("{e:#}")),
    };
    let url = target.as_ref().map(|t| t.url.to_string());
    let facts = match db.trust_facts(url.as_deref()).await {
        Ok(Some(f)) => f,
        // A schema before 0015 (db.schema says so).
        Ok(None) => return Finding::skip(ID, "the database has no plugin trust tables yet"),
        Err(e) => return Finding::error(ID, e),
    };
    let mut warnings = Vec::new();
    let revoked = |id: &str| facts.keys.iter().any(|k| k.key_id == id && k.revoked);
    let source = |id: &str| {
        facts
            .keys
            .iter()
            .find(|k| k.key_id == id)
            .map(|k| k.source.as_str())
    };
    let mut verified = 0;
    for p in &facts.plugins {
        if let Some(signer) = &p.signer
            && revoked(signer)
        {
            warnings.push(format!("{} is signed by revoked key {signer}", p.name));
        }
        let by_registry = p.owner.as_deref().is_some_and(|o| o.starts_with("reg:"));
        if by_registry {
            verified += 1;
        }
        match trust.signatures {
            SignaturePolicy::Any => {}
            SignaturePolicy::Signed if p.signer.is_none() => warnings.push(format!(
                "{} is unsigned, but plugins.signatures = \"signed\"",
                p.name
            )),
            SignaturePolicy::Trusted
                if p.signer.is_none()
                    || (!by_registry
                        && p.signer.as_deref().and_then(source) != Some("operator")) =>
            {
                warnings.push(format!(
                    "{} is not signed by a registry or operator key, but plugins.signatures = \"trusted\"",
                    p.name
                ))
            }
            _ => {}
        }
    }
    let now = Timestamp::now();
    let mut facts_line = vec![format!(
        "{} plugins, {verified} verified",
        facts.plugins.len()
    )];
    let mut info = false;
    match (&target, &facts.registry) {
        (None, _) => {
            info = true;
            facts_line.push(crate::registry::describe(trust).replace("off", "registry off"));
        }
        (Some(_), None) => {
            facts_line.push("registry not checked yet".to_owned());
            if trust.registry_check == RegistryCheck::Daily {
                warnings.push("the daily registry check has not succeeded yet".to_owned());
            }
        }
        (Some(_), Some(r)) => {
            if let Some(e) = &r.last_error {
                warnings.push(format!("the last registry check failed: {e}"));
            }
            if trust.registry_check == RegistryCheck::Daily
                && now.as_micros() - r.checked_at.as_micros() > STALE_AFTER_MICROS
            {
                warnings.push(format!(
                    "the registry was last checked {}",
                    ago(now, r.checked_at)
                ));
            }
            facts_line.push(format!(
                "registry serial {} checked {}",
                r.serial,
                ago(now, r.checked_at)
            ));
        }
    }
    if !warnings.is_empty() {
        Finding::warn(ID, warnings.join("; "))
    } else if info {
        Finding::info(ID, facts_line.join("; "))
    } else {
        Finding::ok(ID, facts_line.join("; "))
    }
}
