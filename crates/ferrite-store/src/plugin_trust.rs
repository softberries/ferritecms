//! Plugin trust storage (`0015_plugin_publishers`, M6-2, design 14 §6): trusted and
//! revoked publisher keys, plugin name owners and the registry cache.

use async_trait::async_trait;
use ferrite_domain::Timestamp;
use ferrite_domain::plugin_trust::{KeyId, PluginOwner, RegistryCacheRow, Revocation, TrustedKey};

use crate::{StoreError, validate_text};

/// Trusted keys, owners and the registry cache. Like every repo, inside a `Tx` it reads
/// and writes through the transaction.
#[async_trait]
pub trait PluginTrustRepo: Send + Sync {
    async fn get_key(&self, key_id: &KeyId) -> Result<Option<TrustedKey>, StoreError>;
    /// Every row, by key id.
    async fn list_keys(&self) -> Result<Vec<TrustedKey>, StoreError>;
    /// Inserts a key or replaces its public key, label, source, `trusted_at` and
    /// revocation. A public key stored under another key id is `UniqueViolation`
    /// (`plugin_publishers_public_key_uq`).
    async fn upsert_key(&self, key: &TrustedKey) -> Result<(), StoreError>;
    /// Records a revocation on an existing row; `false` when the key has no row.
    async fn revoke_key(&self, key_id: &KeyId, revocation: &Revocation)
    -> Result<bool, StoreError>;

    async fn get_owner(&self, plugin: &str) -> Result<Option<PluginOwner>, StoreError>;
    /// Every owner row, by plugin name.
    async fn list_owners(&self) -> Result<Vec<PluginOwner>, StoreError>;
    /// Inserts or replaces the owner row of `owner.plugin` (`bound_at` of an existing row
    /// is kept).
    async fn put_owner(&self, owner: &PluginOwner) -> Result<(), StoreError>;
    /// `false` when the name had no row.
    async fn delete_owner(&self, plugin: &str) -> Result<bool, StoreError>;
    /// The owner rows whose installed version is signed by `key_id`, by plugin name.
    async fn owners_signed_by(&self, key_id: &KeyId) -> Result<Vec<PluginOwner>, StoreError>;

    async fn get_registry(&self, url: &str) -> Result<Option<RegistryCacheRow>, StoreError>;
    /// Inserts or replaces the cache row of `row.url`.
    async fn put_registry(&self, row: &RegistryCacheRow) -> Result<(), StoreError>;
    /// Records a check of `url` at `at`: `error` `None` sets `checked_at` and clears the
    /// last error; `Some` records it (and `last_error_at`) and leaves `checked_at`. The
    /// cached index stays either way; `false` when there is no row.
    async fn registry_checked(
        &self,
        url: &str,
        at: Timestamp,
        error: Option<&str>,
    ) -> Result<bool, StoreError>;
}

/// The NUL checks every backend runs before writing a key row.
pub fn check_trusted_key(key: &TrustedKey) -> Result<(), StoreError> {
    validate_text("publisher key", &key.key.base64)?;
    validate_text("publisher key label", &key.label)?;
    if let Some(r) = &key.revoked {
        validate_text("revocation reason", &r.reason)?;
    }
    Ok(())
}

/// The NUL checks every backend runs before writing an owner row.
pub fn check_plugin_owner(owner: &PluginOwner) -> Result<(), StoreError> {
    validate_text("plugin owner plugin", &owner.plugin)?;
    validate_text("plugin owner publisher", &owner.publisher)?;
    if let Some(s) = &owner.signature {
        validate_text("plugin owner signature", s)?;
    }
    if let Some(u) = &owner.registry_url {
        validate_text("plugin owner registry url", u)?;
    }
    if let Some(o) = &owner.owner {
        validate_text("plugin owner", &o.to_string())?;
    }
    Ok(())
}

/// The NUL checks every backend runs before writing a registry row.
pub fn check_registry_row(row: &RegistryCacheRow) -> Result<(), StoreError> {
    validate_text("registry url", &row.url)?;
    validate_text("registry index", &row.index_json)?;
    validate_text("registry signature", &row.signature)?;
    if let Some(e) = &row.last_error {
        validate_text("registry error", e)?;
    }
    Ok(())
}
