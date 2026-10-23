//! Site options with an in-memory cache of the autoloaded ones.

use std::{
    collections::HashMap,
    sync::{Arc, PoisonError, RwLock},
};

use ferrite_domain::{NewAudit, OptionEntry};
use ferrite_store::{Repos, Store};
use serde_json::{Value, json};

use crate::{
    Actor, CoreError,
    actor::{Capability, require},
    events,
    permalink::{self, PAGE_OPTION, POST_OPTION},
    settings,
    site::FRONT_PAGE_OPTION,
    tx::finish,
    types::{begin_locked, finish_locked},
};

const MAX_KEY_LEN: usize = 200;

/// The autoloaded options, and a counter bumped by every change to them.
#[derive(Default)]
struct Cached {
    entries: HashMap<String, OptionEntry>,
    generation: u64,
}

/// One pending write: `value` `None` deletes the option.
struct Change<'a> {
    key: &'a str,
    value: Option<&'a Value>,
    autoload: bool,
}

/// Reads and writes options (writes need `manage_settings`).
///
/// Options flagged `autoload` are cached in memory; reading them never touches the
/// store. Other options are read from the store each time. The cache is updated after a
/// write commits, so a failed write never shows up in it.
///
/// **Every cache update re-reads the store**, and what it read replaces the cached
/// entry, unless the cache changed while it read (a generation counter, bumped by every
/// update, tells): then the key is dropped instead, and reads fall back to the store
/// until the key's next `options.changed` event re-caches it. A slow re-read therefore
/// never puts an older value back over a newer one. `updated_at` is not compared: it
/// comes from the writing node's clock, and nodes' clocks may differ.
///
/// **Other nodes' writes** reach the cache through
/// [`reload_from_event`](Self::reload_from_event) for each `options.changed` event (the
/// node's reloader calls it, R13) and [`reload_all`](Self::reload_all) when event history
/// was lost.
///
/// **`permalink.*` writes** ([`set`](Self::set), [`delete`](Self::delete),
/// [`set_many`](Self::set_many)) take the [`LOCK_TYPES`](ferrite_store::LOCK_TYPES)
/// lock that type and taxonomy writes take (R19) and check the resulting pair of
/// structures against the registered types and taxonomies inside the transaction
/// ([`permalink::stored_rules`] + [`permalink::check_registered`], P19), so neither
/// side can race the other into a combination the router cannot serve. A clash is
/// `InvalidField { field: "permalink.post" | "permalink.page", .. }` naming the
/// structure. They emit `invalidate [all]` (R10) besides `options.changed`; each node's
/// reloader rebuilds its permalink rules and route table from the event.
pub struct OptionService {
    store: Arc<dyn Store>,
    cache: Arc<RwLock<Cached>>,
}

impl OptionService {
    /// Loads all autoloaded options into the cache.
    pub async fn load(store: Arc<dyn Store>) -> Result<Self, CoreError> {
        let entries = autoloaded(store.as_ref()).await?;
        Ok(Self {
            store,
            cache: Arc::new(RwLock::new(Cached {
                entries,
                generation: 0,
            })),
        })
    }

    /// The option `key`: from the cache if autoloaded, else from the store (not cached).
    pub async fn get(&self, key: &str) -> Result<Option<OptionEntry>, CoreError> {
        // The guard is dropped before the await below.
        let hit = self
            .cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .get(key)
            .cloned();
        if hit.is_some() {
            return Ok(hit);
        }
        Ok(self.store.read().options().get(key).await?)
    }

    /// Sets an option and emits `options.changed` with `option:<key>` in the same
    /// transaction. Afterwards the cache holds the key if `autoload`, else it is removed.
    /// `permalink.*` values are validated (see the type docs); other keys are stored as
    /// given (this is the low-level option API; the admin's settings go through
    /// [`set_many`](Self::set_many)).
    pub async fn set(
        &self,
        actor: &Actor,
        key: &str,
        value: &Value,
        autoload: bool,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_key(key)?;
        if is_permalink(key) {
            settings::validate(key, value)?;
        }
        self.write(
            actor,
            &[Change {
                key,
                value: Some(value),
                autoload,
            }],
            false,
        )
        .await
    }

    /// Deletes an option (a missing one is a no-op) and emits `options.changed`.
    pub async fn delete(&self, actor: &Actor, key: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_key(key)?;
        self.write(
            actor,
            &[Change {
                key,
                value: None,
                autoload: false,
            }],
            false,
        )
        .await
    }

    /// Writes allowlisted settings ([`settings::SETTINGS`]) in one transaction (P19):
    /// `(key, Some(value))` sets the key with `autoload = true` (every render reads
    /// them, F23), `(key, None)` or a JSON `null` deletes it (back to the default).
    ///
    /// Needs `manage_settings`. Every change is validated first ([`settings::validate`]:
    /// an unknown key or a key given twice is `Invalid`, a bad value
    /// `InvalidField { field: <key>, .. }`), then in the transaction: the front page
    /// must be an existing page, and the `permalink.*` pair (stored values with these
    /// changes on top) must fit the registered types and taxonomies. Any failure writes
    /// nothing and emits nothing. On success each key gets its own `options.changed`
    /// event, `permalink.*` changes add one `invalidate [all]`, and one audit row
    /// `settings.update` lists the keys (P22). The cache is updated after the commit as
    /// [`set`](Self::set) does. An empty list is a no-op.
    pub async fn set_many(
        &self,
        actor: &Actor,
        changes: &[(String, Option<Value>)],
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        let mut pending: Vec<Change<'_>> = Vec::with_capacity(changes.len());
        for (key, value) in changes {
            let value = value.as_ref().filter(|v| !v.is_null());
            match value {
                Some(v) => settings::validate(key, v)?,
                None if settings::def(key).is_none() => {
                    return Err(CoreError::Invalid(format!("unknown setting {key:?}")));
                }
                None => {}
            }
            if pending.iter().any(|c| c.key == key) {
                return Err(CoreError::Invalid(format!("setting {key:?} given twice")));
            }
            pending.push(Change {
                key,
                value,
                autoload: true,
            });
        }
        if pending.is_empty() {
            return Ok(());
        }
        self.write(actor, &pending, true).await
    }

    /// The one write path: [`write_changes`] in one transaction (under
    /// [`LOCK_TYPES`](ferrite_store::LOCK_TYPES) when a `permalink.*` key changes), then
    /// the cache.
    async fn write(
        &self,
        actor: &Actor,
        changes: &[Change<'_>],
        settings_write: bool,
    ) -> Result<(), CoreError> {
        if changes.iter().any(|c| is_permalink(c.key)) {
            let (guard, tx) = begin_locked(self.store.as_ref()).await?;
            let done = write_changes(tx.as_ref(), actor, changes, settings_write).await;
            finish_locked(guard, tx, done).await?;
        } else {
            let tx = self.store.begin().await?;
            let done = write_changes(tx.as_ref(), actor, changes, settings_write).await;
            finish(tx, done).await?;
        }
        for c in changes {
            self.refresh_after_write(c).await;
        }
        Ok(())
    }

    /// The cache after a committed write of `c`: the stored entry if it is autoloaded,
    /// else nothing, under the generation guard (see the type docs). If the re-read
    /// fails the key is dropped, so reads fall back to the store.
    async fn refresh_after_write(&self, c: &Change<'_>) {
        if c.value.is_none() || !c.autoload {
            self.store_guarded(c.key, self.generation(), None);
            return;
        }
        let seen = self.generation();
        match self.store.read().options().get(c.key).await {
            Ok(entry) => self.store_guarded(c.key, seen, entry.filter(|e| e.autoload)),
            Err(e) => {
                tracing::warn!(key = c.key, error = %e, "option cache refresh failed; dropping key");
                self.store_guarded(c.key, seen, None);
            }
        }
    }

    /// Puts `entry` (`None`: removes the key) into the cache if its generation is still
    /// `seen`, else drops the key; bumps the generation either way.
    fn store_guarded(&self, key: &str, seen: u64, entry: Option<OptionEntry>) {
        let mut cache = self.cache.write().unwrap_or_else(PoisonError::into_inner);
        put_guarded(&mut cache, key, seen, entry);
    }

    /// Re-reads the given keys from the store into the cache, for `options.changed`
    /// events (from any node, R13). Takes event keys (`option:<key>`); others are
    /// ignored.
    ///
    /// Each key is re-read under the generation guard (see the type docs). A store error
    /// drops the key (and the keys after it) and is returned.
    pub async fn reload_from_event(&self, keys: &[String]) -> Result<(), CoreError> {
        let keys: Vec<&str> = keys
            .iter()
            .filter_map(|raw| raw.strip_prefix("option:"))
            .collect();
        for (i, key) in keys.iter().enumerate() {
            let seen = self.generation();
            let entry = match self.store.read().options().get(key).await {
                Ok(entry) => entry.filter(|e| e.autoload),
                Err(e) => {
                    let mut cache = self.cache.write().unwrap_or_else(PoisonError::into_inner);
                    cache.generation += 1;
                    for key in &keys[i..] {
                        cache.entries.remove(*key);
                    }
                    return Err(e.into());
                }
            };
            self.store_guarded(key, seen, entry);
        }
        Ok(())
    }

    /// Re-reads every autoloaded option (after the node lost event history). The cache
    /// is replaced by what the store holds, or emptied if it changed during the read or
    /// the read failed (the error is returned); reads then fall back to the store.
    pub async fn reload_all(&self) -> Result<(), CoreError> {
        let seen = self.generation();
        let read = autoloaded(self.store.as_ref()).await;
        let mut cache = self.cache.write().unwrap_or_else(PoisonError::into_inner);
        let unchanged = cache.generation == seen;
        cache.generation += 1;
        match read {
            Ok(entries) if unchanged => {
                cache.entries = entries;
                Ok(())
            }
            Ok(_) => {
                cache.entries.clear();
                Ok(())
            }
            Err(e) => {
                cache.entries.clear();
                Err(e)
            }
        }
    }

    fn generation(&self) -> u64 {
        self.cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .generation
    }
}

/// [`OptionService::store_guarded`] on the locked cache.
fn put_guarded(cache: &mut Cached, key: &str, seen: u64, entry: Option<OptionEntry>) {
    match entry {
        Some(e) if cache.generation == seen => {
            cache.entries.insert(key.to_owned(), e);
        }
        _ => {
            cache.entries.remove(key);
        }
    }
    cache.generation += 1;
}

async fn autoloaded(store: &dyn Store) -> Result<HashMap<String, OptionEntry>, CoreError> {
    Ok(store
        .read()
        .options()
        .autoloaded()
        .await?
        .into_iter()
        .map(|e| (e.key.clone(), e))
        .collect())
}

/// Inside the write's transaction: the checks that need the store (`permalink.*` pair;
/// for a `settings_write`, the front page), each set or delete with its
/// `options.changed`, `invalidate [all]` for `permalink.*`, and for a `settings_write`
/// the audit row `settings.update`.
async fn write_changes(
    repos: &dyn Repos,
    actor: &Actor,
    changes: &[Change<'_>],
    settings_write: bool,
) -> Result<(), CoreError> {
    let permalinks: Vec<&Change<'_>> = changes.iter().filter(|c| is_permalink(c.key)).collect();
    if !permalinks.is_empty() {
        check_permalinks(repos, &permalinks).await?;
    }
    if settings_write {
        for c in changes {
            if c.key == FRONT_PAGE_OPTION
                && let Some(v) = c.value
            {
                settings::check_front_page(repos, v).await?;
            }
        }
    }
    for c in changes {
        match c.value {
            Some(v) => repos.options().set(c.key, v, c.autoload).await?,
            None => repos.options().delete(c.key).await?,
        }
        repos
            .events()
            .append(&events::options_changed(c.key))
            .await?;
    }
    if !permalinks.is_empty() {
        repos
            .events()
            .append(&events::invalidate([events::ALL_KEY.to_owned()]))
            .await?;
    }
    if settings_write {
        let keys: Vec<&str> = changes.iter().map(|c| c.key).collect();
        repos
            .audit()
            .append(&NewAudit {
                actor_id: Some(actor.user),
                action: "settings.update".to_owned(),
                target_kind: "settings".to_owned(),
                target_id: None,
                details: json!({ "keys": keys }),
            })
            .await?;
    }
    Ok(())
}

fn is_permalink(key: &str) -> bool {
    key == POST_OPTION || key == PAGE_OPTION
}

/// The `permalink.*` pair with `changes` on top fits the registered types and
/// taxonomies (read through `repos`, the write's transaction). A clash blames the
/// changed key whose structure the message names (else the first changed key); stored
/// options that no longer parse are plain `Invalid`.
async fn check_permalinks(repos: &dyn Repos, changes: &[&Change<'_>]) -> Result<(), CoreError> {
    let overrides: Vec<(&str, Option<&str>)> = changes
        .iter()
        .map(|c| (c.key, c.value.and_then(Value::as_str)))
        .collect();
    let rules = permalink::stored_rules(repos, &overrides).await?;
    permalink::check_registered(repos, &rules)
        .await
        .map_err(|e| match e {
            CoreError::Invalid(message) => {
                let blamed = changes
                    .iter()
                    .find(|c| {
                        c.value
                            .and_then(Value::as_str)
                            .is_some_and(|s| message.contains(&format!("{s:?}")))
                    })
                    .or(changes.first());
                let field = match blamed.map(|c| c.key) {
                    Some(PAGE_OPTION) => PAGE_OPTION,
                    _ => POST_OPTION,
                };
                CoreError::InvalidField { field, message }
            }
            e => e,
        })
}

fn check_key(key: &str) -> Result<(), CoreError> {
    if key.is_empty() || key.len() > MAX_KEY_LEN || key.chars().any(char::is_control) {
        return Err(CoreError::Invalid(format!(
            "option key must be 1-{MAX_KEY_LEN} bytes without control characters"
        )));
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
    fn guarded_puts_drop_the_key_when_the_cache_moved() {
        let mut cache = Cached::default();
        // Unchanged generation: the re-read wins, even with an older `updated_at`.
        put_guarded(&mut cache, "k", 0, Some(entry(2, 2_000_000)));
        put_guarded(&mut cache, "k", 1, Some(entry(1, 1_000_000)));
        assert_eq!(cache.entries["k"].value, json!(1));
        assert_eq!(cache.generation, 2);
        // Another update in between (generation 2 != 1): dropped, not guessed.
        put_guarded(&mut cache, "k", 1, Some(entry(3, 3_000_000)));
        assert!(cache.entries.is_empty());
        // Removal always applies.
        put_guarded(&mut cache, "k", 3, Some(entry(4, 0)));
        put_guarded(&mut cache, "k", 4, None);
        assert!(cache.entries.is_empty());
    }
}
