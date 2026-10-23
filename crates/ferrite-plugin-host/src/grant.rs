//! What an admin approved for a plugin (plan rulings P3, P5): its capabilities and the
//! resource limits above the host defaults, stored as the `plugins.granted` JSON.
//!
//! **Stored form (v1).** `{"v":1,"capabilities":<[capabilities] table>,"limits":
//! {"memory":n|null,"fuel":n|null}}`. The capabilities table is the JSON form of a
//! manifest's `[capabilities]` section with only non-default members, and is read back
//! through [`Manifest::parse`] (so a grant obeys every manifest rule, bare hosts
//! included). The M1c-2 form, a bare `[capabilities]` table, is still read; it carries
//! no approved limits.

use serde_json::{Map, Value, json};

use crate::error::PluginError;
use crate::manifest::{Capabilities, Manifest, capabilities_json};

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

/// An approved grant: capabilities plus approved limits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grant {
    pub capabilities: Capabilities,
    pub limits: ApprovedLimits,
}

const GRANT_VERSION: u64 = 1;

impl Grant {
    /// Exactly what `manifest` asks for: its capabilities and its `[limits]` requests.
    pub fn of(manifest: &Manifest) -> Grant {
        Grant {
            capabilities: manifest.capabilities.clone(),
            limits: ApprovedLimits {
                memory: manifest.limits.memory,
                fuel: manifest.limits.fuel,
            },
        }
    }

    /// The stored (v1) form; see the module docs.
    pub fn to_json(&self) -> Value {
        json!({
            "v": GRANT_VERSION,
            "capabilities": capabilities_json(&self.capabilities),
            "limits": { "memory": self.limits.memory, "fuel": self.limits.fuel },
        })
    }

    /// Read a stored grant: the v1 form, or the M1c-2 bare `[capabilities]` table (no
    /// approved limits). `plugin` is the manifest's `[plugin]` table in JSON form
    /// (`row.manifest["plugin"]`), needed to validate the capabilities as a manifest.
    /// Anything malformed is an error, never an empty grant.
    pub fn from_json(plugin: &Value, granted: &Value) -> Result<Grant, String> {
        let Value::Object(object) = granted else {
            return Err("granted: expected an object".to_owned());
        };
        if !object.contains_key("v") {
            return Ok(Grant {
                capabilities: parse_capabilities(plugin, granted)?,
                limits: ApprovedLimits::default(),
            });
        }
        if object.get("v").and_then(Value::as_u64) != Some(GRANT_VERSION) {
            return Err(format!(
                "granted: unsupported version (expected v = {GRANT_VERSION})"
            ));
        }
        if let Some(key) = object
            .keys()
            .find(|k| !matches!(k.as_str(), "v" | "capabilities" | "limits"))
        {
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
        Ok(Grant {
            capabilities,
            limits,
        })
    }

    /// Whether everything `wanted` asks for is within this grant: its capabilities
    /// ([`Capabilities::is_subset_of`]) and each of its limits approved here at least as
    /// high (a limit `wanted` leaves unset needs nothing).
    pub fn covers(&self, wanted: &Grant) -> bool {
        let limit = |want: Option<u64>, have: Option<u64>| match want {
            None => true,
            Some(w) => have.is_some_and(|h| h >= w),
        };
        wanted.capabilities.is_subset_of(&self.capabilities)
            && limit(wanted.limits.memory, self.limits.memory)
            && limit(wanted.limits.fuel, self.limits.fuel)
    }
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

    #[test]
    fn covers_compares_limits() {
        let with = |memory, fuel| Grant {
            capabilities: Capabilities::default(),
            limits: ApprovedLimits { memory, fuel },
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
