//! Site options with an in-memory cache of the autoloaded ones.

use std::{
    collections::HashMap,
    sync::{Arc, PoisonError, RwLock},
};

use ferrite_domain::OptionEntry;
use ferrite_store::{Repos, Store};

use crate::{
    Actor, CoreError,
    actor::{Capability, require},
    events,
    tx::finish,
};

const MAX_KEY_LEN: usize = 200;

type Cache = Arc<RwLock<HashMap<String, OptionEntry>>>;

/// Reads and writes options (writes need `manage_settings`).
///
/// Options flagged `autoload` are cached in memory; reading them never touches the
/// store. Other options are read from the store each time. The cache is updated after a
/// write commits, so a failed write never shows up in it. A value is only replaced by
/// one with the same or a newer `updated_at`, so a slow re-read of an older write cannot
/// overwrite a newer cached value. Removals (delete, `autoload = false`) always apply.
pub struct OptionService {
    store: Arc<dyn Store>,
    cache: Cache,
}

impl OptionService {
    /// Loads all autoloaded options into the cache.
    pub async fn load(store: Arc<dyn Store>) -> Result<Self, CoreError> {
        let cache: HashMap<String, OptionEntry> = store
            .read()
            .options()
            .autoloaded()
            .await?
            .into_iter()
            .map(|e| (e.key.clone(), e))
            .collect();
        Ok(Self {
            store,
            cache: Arc::new(RwLock::new(cache)),
        })
    }

    /// The option `key`: from the cache if autoloaded, else from the store (not cached).
    pub async fn get(&self, key: &str) -> Result<Option<OptionEntry>, CoreError> {
        // The guard is dropped before the await below.
        let hit = self
            .cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned();
        if hit.is_some() {
            return Ok(hit);
        }
        Ok(self.store.read().options().get(key).await?)
    }

    /// Sets an option and emits `options.changed` with `option:<key>` in the same
    /// transaction. Afterwards the cache holds the key if `autoload`, else it is removed.
    pub async fn set(
        &self,
        actor: &Actor,
        key: &str,
        value: &serde_json::Value,
        autoload: bool,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_key(key)?;
        let tx = self.store.begin().await?;
        let done = async {
            tx.options().set(key, value, autoload).await?;
            tx.events().append(&events::options_changed(key)).await?;
            emit_all_for_permalinks(tx.as_ref(), key).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await?;
        // Re-read what was stored (it carries `updated_at`). If that fails the key is
        // dropped from the cache, so reads fall back to the store.
        let stored = if autoload {
            match self.store.read().options().get(key).await {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!(key, error = %e, "option cache refresh failed; dropping key");
                    None
                }
            }
        } else {
            None
        };
        self.apply(key, stored);
        Ok(())
    }

    /// Deletes an option (a missing one is a no-op) and emits `options.changed`.
    pub async fn delete(&self, actor: &Actor, key: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_key(key)?;
        let tx = self.store.begin().await?;
        let done = async {
            tx.options().delete(key).await?;
            tx.events().append(&events::options_changed(key)).await?;
            emit_all_for_permalinks(tx.as_ref(), key).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await?;
        self.apply(key, None);
        Ok(())
    }

    /// Re-reads the given keys from the store into the cache, for `options.changed`
    /// events from other nodes. Takes event keys (`option:<key>`); others are ignored.
    pub async fn reload_from_event(&self, keys: &[String]) -> Result<(), CoreError> {
        for raw in keys {
            let Some(key) = raw.strip_prefix("option:") else {
                continue;
            };
            let entry = self
                .store
                .read()
                .options()
                .get(key)
                .await?
                .filter(|e| e.autoload);
            self.apply(key, entry);
        }
        Ok(())
    }

    fn apply(&self, key: &str, entry: Option<OptionEntry>) {
        let mut cache = self.cache.write().unwrap_or_else(PoisonError::into_inner);
        merge(&mut cache, key, entry);
    }
}

/// Stores `entry` unless the cache holds a strictly newer one; `None` removes the key.
fn merge(cache: &mut HashMap<String, OptionEntry>, key: &str, entry: Option<OptionEntry>) {
    match entry {
        Some(e) => {
            if cache
                .get(key)
                .is_some_and(|old| old.updated_at > e.updated_at)
            {
                return;
            }
            cache.insert(key.to_owned(), e);
        }
        None => {
            cache.remove(key);
        }
    }
}

fn check_key(key: &str) -> Result<(), CoreError> {
    if key.is_empty() || key.len() > MAX_KEY_LEN || key.chars().any(char::is_control) {
        return Err(CoreError::Invalid(format!(
            "option key must be 1-{MAX_KEY_LEN} bytes without control characters"
        )));
    }
    Ok(())
}

/// Every URL changes with a `permalink.*` option: flush everything.
async fn emit_all_for_permalinks(repos: &dyn Repos, key: &str) -> Result<(), CoreError> {
    if key.starts_with("permalink.") {
        repos
            .events()
            .append(&events::invalidate([events::ALL_KEY.to_owned()]))
            .await?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use ferrite_domain::Timestamp;
    use serde_json::json;

    use super::*;

    fn entry(value: i64, micros: i64) -> OptionEntry {
        OptionEntry {
            key: "k".into(),
            value: json!(value),
            autoload: true,
            updated_at: Timestamp::from_micros(micros).expect("valid micros"),
        }
    }

    #[test]
    fn merge_keeps_the_newer_value() {
        let mut cache = HashMap::new();
        merge(&mut cache, "k", Some(entry(2, 2_000_000)));
        // An older re-read does not overwrite.
        merge(&mut cache, "k", Some(entry(1, 1_000_000)));
        assert_eq!(cache["k"].value, json!(2));
        // Same or newer does.
        merge(&mut cache, "k", Some(entry(3, 2_000_000)));
        assert_eq!(cache["k"].value, json!(3));
        merge(&mut cache, "k", Some(entry(4, 3_000_000)));
        assert_eq!(cache["k"].value, json!(4));
        // Removal always applies.
        merge(&mut cache, "k", None);
        assert!(cache.is_empty());
    }
}
