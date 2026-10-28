//! What an admin approved for a plugin (plan rulings P3, P5): its capabilities and the
//! resource limits above the host defaults, stored as the `plugins.granted` JSON.
//!
//! **Stored form (v1).** `{"v":1,"capabilities":<[capabilities] table>,"limits":
//! {"memory":n|null,"fuel":n|null},"routes":[{"cross_origin":bool,"rate_limit":s|null}]}`.
//! The capabilities table is the JSON form of a manifest's `[capabilities]` section with
//! only non-default members, and is read back through [`Manifest::parse`] (so a grant
//! obeys every manifest rule, bare hosts included). `routes` holds the distinct route
//! policies approved (ruling R7: `cross_origin` and `rate_limit` are part of the grant;
//! the paths are not, R6), sorted. `"hooks":["request.served"]` is present only when the
//! plugin subscribes to `request.served` (ruling R23: site-wide traffic metadata is part
//! of what is approved, so subscribing is widening); it is left out otherwise, so the
//! stored form and the fingerprint of a grant without it are what they were before R23.
//! Other hooks are not part of the grant. The M1c-2 form, a bare `[capabilities]` table,
//! is still read; it carries no approved limits, no route policies and no hooks.

use serde_json::{Map, Value, json};

use crate::config::HostConfig;
use crate::error::PluginError;
use crate::manifest::{Capabilities, Manifest, RouteSpec, capabilities_json, valid_rate_limit};

/// The rate limit a route without `rate_limit` gets (plan "Exact values").
pub const DEFAULT_ROUTE_RATE_LIMIT: &str = "10/min";

/// Memory a plugin may be approved for, at most (P5): 256 MiB.
pub const LIMIT_CEILING_MEMORY: u64 = 256 * 1024 * 1024;

/// Fuel a plugin may be approved for, at most, as a multiple of the host default (P5).
pub const LIMIT_CEILING_FUEL_FACTOR: u64 = 10;

/// Limits above the host defaults an admin approved; `None` approves nothing above the
/// default ([`Default`] approves nothing at all).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApprovedLimits {
    /// Bytes.
    pub memory: Option<u64>,
    pub fuel: Option<u64>,
}

/// What a route may do apart from its path (ruling R7).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RoutePolicy {
    pub cross_origin: bool,
    /// `"N/s"`, `"N/min"` or `"N/h"`; `None` = [`DEFAULT_ROUTE_RATE_LIMIT`].
    pub rate_limit: Option<String>,
}

impl RoutePolicy {
    pub fn of(spec: &RouteSpec) -> RoutePolicy {
        RoutePolicy {
            cross_origin: spec.cross_origin,
            rate_limit: spec.rate_limit.clone(),
        }
    }

    /// Whether `self` is at most as permissive as `approved` (R7): no cross-origin
    /// unless approved; a rate limit no looser (no higher rate and no larger count per
    /// window; an absent limit counts as the default); dropping an explicit limit is
    /// always looser.
    pub fn within(&self, approved: &RoutePolicy) -> bool {
        if self.cross_origin && !approved.cross_origin {
            return false;
        }
        match (&self.rate_limit, &approved.rate_limit) {
            (None, None) => true,
            (None, Some(_)) => false,
            (Some(want), have) => {
                let have = have.as_deref().unwrap_or(DEFAULT_ROUTE_RATE_LIMIT);
                match (rate(want), rate(have)) {
                    (Some((wn, ww)), Some((hn, hw))) => {
                        // wn/ww <= hn/hw, and the burst wn <= hn.
                        u128::from(wn) * u128::from(hw) <= u128::from(hn) * u128::from(ww)
                            && wn <= hn
                    }
                    _ => false,
                }
            }
        }
    }
}

/// `(count, window seconds)` of a valid `N/unit` rate limit.
fn rate(s: &str) -> Option<(u64, u64)> {
    let (n, unit) = s.split_once('/')?;
    let window = match unit {
        "s" => 1,
        "min" => 60,
        "h" => 3600,
        _ => return None,
    };
    Some((n.parse().ok()?, window))
}

/// An approved grant: capabilities, approved limits and route policies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grant {
    pub capabilities: Capabilities,
    pub limits: ApprovedLimits,
    /// Distinct, sorted.
    pub routes: Vec<RoutePolicy>,
    /// The plugin may subscribe to `request.served` (R23).
    pub request_served: bool,
}

const GRANT_VERSION: u64 = 1;

/// The one hook that is part of a grant (R23).
pub(crate) const GRANTED_HOOK: &str = "request.served";

impl Grant {
    /// Exactly what `manifest` asks for: its capabilities, its `[limits]` requests and
    /// the policies of its routes.
    pub fn of(manifest: &Manifest) -> Grant {
        let mut routes: Vec<RoutePolicy> = manifest.routes.values().map(RoutePolicy::of).collect();
        routes.sort();
        routes.dedup();
        Grant {
            capabilities: manifest.capabilities.clone(),
            limits: ApprovedLimits {
                memory: manifest.limits.memory,
                fuel: manifest.limits.fuel,
            },
            routes,
            request_served: manifest.hooks.contains_key(GRANTED_HOOK),
        }
    }

    /// What a legacy row with a null `granted` counts as (ruling R7): [`Grant::of`] its
    /// manifest with no limit above the host defaults approved, so it loads with its
    /// `[limits]` capped at the defaults until an admin approves more.
    pub fn legacy(manifest: &Manifest) -> Grant {
        Grant {
            limits: ApprovedLimits::default(),
            ..Grant::of(manifest)
        }
    }

    /// The stored (v1) form; see the module docs.
    pub fn to_json(&self) -> Value {
        let mut doc = json!({
            "v": GRANT_VERSION,
            "capabilities": capabilities_json(&self.capabilities),
            "limits": { "memory": self.limits.memory, "fuel": self.limits.fuel },
            "routes": self.routes.iter().map(|r| json!({
                "cross_origin": r.cross_origin,
                "rate_limit": r.rate_limit,
            })).collect::<Vec<_>>(),
        });
        if self.request_served
            && let Value::Object(o) = &mut doc
        {
            o.insert("hooks".to_owned(), json!([GRANTED_HOOK]));
        }
        doc
    }

    /// Read a stored grant: the v1 form, or the M1c-2 bare `[capabilities]` table (no
    /// approved limits, no route policies). `plugin` is the manifest's `[plugin]` table in JSON form
    /// (`row.manifest["plugin"]`), needed to validate the capabilities as a manifest.
    /// Anything malformed is an error, never an empty grant.
    pub fn from_json(plugin: &Value, granted: &Value) -> Result<Grant, String> {
        let Value::Object(object) = granted else {
            return Err("granted: expected an object".to_owned());
        };
        if !object.contains_key("v") {
            return Ok(Grant {
                capabilities: parse_capabilities(plugin, granted)?,
                ..Grant::default()
            });
        }
        if object.get("v").and_then(Value::as_u64) != Some(GRANT_VERSION) {
            return Err(format!(
                "granted: unsupported version (expected v = {GRANT_VERSION})"
            ));
        }
        if let Some(key) = object.keys().find(|k| {
            !matches!(
                k.as_str(),
                "v" | "capabilities" | "limits" | "routes" | "hooks"
            )
        }) {
            return Err(format!("granted.{key}: unknown key"));
        }
        let capabilities = match object.get("capabilities") {
            Some(c @ Value::Object(_)) => parse_capabilities(plugin, c)?,
            _ => return Err("granted.capabilities: expected an object".to_owned()),
        };
        let limits = match object.get("limits") {
            Some(Value::Object(l)) => parse_limits(l)?,
            _ => return Err("granted.limits: expected an object".to_owned()),
        };
        let routes = match object.get("routes") {
            Some(Value::Array(r)) => parse_routes(r)?,
            _ => return Err("granted.routes: expected an array".to_owned()),
        };
        let request_served = match object.get("hooks") {
            None => false,
            Some(Value::Array(h)) if h.iter().all(|v| v.as_str() == Some(GRANTED_HOOK)) => {
                !h.is_empty()
            }
            Some(_) => {
                return Err(format!(
                    "granted.hooks: expected an array holding only \"{GRANTED_HOOK}\""
                ));
            }
        };
        Ok(Grant {
            capabilities,
            limits,
            routes,
            request_served,
        })
    }

    /// Whether `wanted`'s capabilities, route policies and `request.served` subscription
    /// are within this grant
    /// ([`Capabilities::is_subset_of`]; each route policy [`RoutePolicy::within`] an
    /// approved one). Limits are not compared: [`PluginHost::prepare_with`](crate::PluginHost::prepare_with)
    /// (crate::PluginHost::prepare_with) applies them.
    pub fn covers_access(&self, wanted: &Grant) -> bool {
        wanted.capabilities.is_subset_of(&self.capabilities)
            && wanted.routes.iter().all(|r| self.covers_route(r))
            && (!wanted.request_served || self.request_served)
    }

    /// Whether route policy `r` is within one approved here.
    pub fn covers_route(&self, r: &RoutePolicy) -> bool {
        self.routes.iter().any(|have| r.within(have))
    }

    /// [`covers_access`](Self::covers_access), and each of `wanted`'s limits approved
    /// here at least as high (a limit `wanted` leaves unset needs nothing).
    pub fn covers(&self, wanted: &Grant) -> bool {
        let limit = |want: Option<u64>, have: Option<u64>| match want {
            None => true,
            Some(w) => have.is_some_and(|h| h >= w),
        };
        self.covers_access(wanted)
            && limit(wanted.limits.memory, self.limits.memory)
            && limit(wanted.limits.fuel, self.limits.fuel)
    }
}

fn parse_routes(items: &[Value]) -> Result<Vec<RoutePolicy>, String> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Value::Object(o) = item else {
            return Err("granted.routes: expected objects".to_owned());
        };
        if let Some(key) = o
            .keys()
            .find(|k| !matches!(k.as_str(), "cross_origin" | "rate_limit"))
        {
            return Err(format!("granted.routes.{key}: unknown key"));
        }
        let Some(Value::Bool(cross_origin)) = o.get("cross_origin") else {
            return Err("granted.routes.cross_origin: expected a boolean".to_owned());
        };
        let rate_limit = match o.get("rate_limit") {
            Some(Value::Null) => None,
            Some(Value::String(r)) if valid_rate_limit(r) => Some(r.clone()),
            _ => {
                return Err(
                    "granted.routes.rate_limit: expected null or \"N/s\", \"N/min\", \"N/h\""
                        .to_owned(),
                );
            }
        };
        out.push(RoutePolicy {
            cross_origin: *cross_origin,
            rate_limit,
        });
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn parse_capabilities(plugin: &Value, capabilities: &Value) -> Result<Capabilities, String> {
    let doc = json!({ "plugin": plugin, "capabilities": capabilities });
    Manifest::from_json(&doc)
        .map(|m| m.capabilities)
        .map_err(|e| format!("granted: {e}"))
}

fn parse_limits(limits: &Map<String, Value>) -> Result<ApprovedLimits, String> {
    if let Some(key) = limits
        .keys()
        .find(|k| !matches!(k.as_str(), "memory" | "fuel"))
    {
        return Err(format!("granted.limits.{key}: unknown key"));
    }
    let one = |key: &str| -> Result<Option<u64>, String> {
        match limits.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .filter(|n| *n > 0)
                .map(Some)
                .ok_or_else(|| format!("granted.limits.{key}: expected a positive integer")),
        }
    };
    Ok(ApprovedLimits {
        memory: one("memory")?,
        fuel: one("fuel")?,
    })
}

/// The default and ceiling one limit is checked against.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LimitBounds {
    pub(crate) default: u64,
    pub(crate) ceiling: u64,
}

impl LimitBounds {
    /// The memory and fuel bounds of a host with `cfg`: the memory ceiling is
    /// [`LIMIT_CEILING_MEMORY`] (never below the default), or the default on a pooling
    /// host (its pool slots are that size), so whatever a report allows can load.
    pub(crate) fn of_host(cfg: &HostConfig) -> (Self, Self) {
        let memory_default = u64::try_from(cfg.memory_bytes).unwrap_or(u64::MAX);
        let memory = if cfg.pooling {
            Self::fixed(memory_default)
        } else {
            Self::memory(memory_default)
        };
        (memory, Self::fuel(cfg.fuel))
    }

    /// Memory: the default, and [`LIMIT_CEILING_MEMORY`] (never below the default).
    pub(crate) fn memory(default: u64) -> Self {
        Self {
            default,
            ceiling: LIMIT_CEILING_MEMORY.max(default),
        }
    }

    /// A ceiling equal to the default: nothing above it, approved or not.
    pub(crate) fn fixed(default: u64) -> Self {
        Self {
            default,
            ceiling: default,
        }
    }

    /// Fuel: the default, and [`LIMIT_CEILING_FUEL_FACTOR`] times it.
    pub(crate) fn fuel(default: u64) -> Self {
        Self {
            default,
            ceiling: default.saturating_mul(LIMIT_CEILING_FUEL_FACTOR),
        }
    }

    /// The effective limit (P5): the default when nothing is requested; the request
    /// when it is at most the default, or at most both the ceiling and the approved
    /// value. Above the ceiling is refused whatever was approved.
    pub(crate) fn effective(
        self,
        limit: &'static str,
        requested: Option<u64>,
        approved: Option<u64>,
    ) -> Result<u64, PluginError> {
        let Some(requested) = requested else {
            return Ok(self.default);
        };
        if requested <= self.default {
            return Ok(requested);
        }
        self.within_ceiling(limit, requested)?;
        if approved.is_some_and(|a| requested <= a) {
            Ok(requested)
        } else {
            Err(PluginError::LimitAboveDefault {
                limit,
                requested,
                default: self.default,
            })
        }
    }

    /// `LimitAboveCeiling` when `requested` is above the ceiling.
    pub(crate) fn within_ceiling(
        self,
        limit: &'static str,
        requested: u64,
    ) -> Result<(), PluginError> {
        if requested > self.ceiling {
            return Err(PluginError::LimitAboveCeiling {
                limit,
                requested,
                ceiling: self.ceiling,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::DESIGN_EXAMPLE;

    #[test]
    fn grant_of_the_design_example_round_trips() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        let g = Grant::of(&m);
        let plugin = json!({
            "name": "seo-pro",
            "version": "1.4.0",
            "publisher": "example.dev",
            "world": "ferrite:plugin/plugin@0.1.0",
        });
        assert_eq!(Grant::from_json(&plugin, &g.to_json()), Ok(g.clone()));
        assert!(g.covers(&g));
        assert!(g.covers(&Grant::default()));
        assert!(!Grant::default().covers(&g));
    }

    /// Ruling R23: `request.served` is part of the grant, written only when set.
    #[test]
    fn request_served_is_granted() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        let plugin = json!({
            "name": "seo-pro",
            "version": "1.4.0",
            "publisher": "example.dev",
            "world": "ferrite:plugin/plugin@0.1.0",
        });
        let with = Grant::of(&m);
        assert!(with.request_served, "the design example subscribes");
        assert_eq!(with.to_json()["hooks"], json!(["request.served"]));
        let without = Grant {
            request_served: false,
            ..with.clone()
        };
        assert!(
            without.to_json().get("hooks").is_none(),
            "left out when unset"
        );
        assert_eq!(
            Grant::from_json(&plugin, &without.to_json()),
            Ok(without.clone())
        );
        assert!(with.covers_access(&without));
        assert!(!without.covers_access(&with), "subscribing is widening");
        // Only `request.served` may be listed.
        let mut bad = with.to_json();
        bad["hooks"] = json!(["post.saved"]);
        assert!(Grant::from_json(&plugin, &bad).is_err());
        // A legacy null grant follows its manifest.
        assert!(Grant::legacy(&m).request_served);
    }

    #[test]
    fn covers_compares_limits() {
        let with = |memory, fuel| Grant {
            capabilities: Capabilities::default(),
            limits: ApprovedLimits { memory, fuel },
            routes: Vec::new(),
            request_served: false,
        };
        assert!(with(Some(64), None).covers(&with(Some(48), None)));
        assert!(!with(Some(48), None).covers(&with(Some(64), None)));
        assert!(!with(None, None).covers(&with(Some(1), None)));
        assert!(with(None, None).covers(&with(None, None)));
        assert!(!with(Some(64), None).covers(&with(None, Some(1))));
    }

    #[test]
    fn effective_limits() {
        let b = LimitBounds::memory(32);
        assert_eq!(b.effective("memory", None, None), Ok(32));
        assert_eq!(b.effective("memory", Some(16), None), Ok(16));
        assert_eq!(b.effective("memory", Some(64), Some(64)), Ok(64));
        assert!(matches!(
            b.effective("memory", Some(64), None),
            Err(PluginError::LimitAboveDefault { .. })
        ));
        assert!(matches!(
            b.effective("memory", Some(LIMIT_CEILING_MEMORY + 1), Some(u64::MAX)),
            Err(PluginError::LimitAboveCeiling { .. })
        ));
        // A default above the ceiling is still allowed in full.
        let big = LimitBounds::memory(LIMIT_CEILING_MEMORY * 2);
        assert_eq!(
            big.effective("memory", Some(LIMIT_CEILING_MEMORY * 2), None),
            Ok(LIMIT_CEILING_MEMORY * 2)
        );
        assert_eq!(LimitBounds::fuel(u64::MAX).ceiling, u64::MAX);
    }
}
