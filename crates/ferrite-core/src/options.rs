//! Site options with an in-memory cache of the autoloaded ones.

use std::{
    collections::HashMap,
    sync::{Arc, PoisonError, RwLock},
};

use ferrite_domain::OptionEntry;
use ferrite_store::Store;

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
/// write commits, so a failed write never shows up in it. Two concurrent writes of the
/// same key may leave the cache with the older one; `reload_from_event` repairs it.
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
            Ok(())
        }
        .await;
        finish(tx, done).await?;
        // Re-read what was stored (it carries `updated_at`); a failure here only leaves
        // the cache stale, which is repaired by dropping the key.
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
        match entry {
            Some(e) => {
                cache.insert(key.to_owned(), e);
            }
            None => {
                cache.remove(key);
            }
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
