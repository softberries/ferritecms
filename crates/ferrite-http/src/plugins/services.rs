//! [`HostServices`] over the store: what plugin imports read and write.

use std::sync::Arc;

use ferrite_core::{
    DepKeys, PageItem, SiteRead, SiteReader,
    auth::SiteKey,
    events::{invalidate, option_key, post_key},
    ext_settings::{SettingDef, open_secret, plugin_option, resolve_plugin_value},
};
use ferrite_domain::{ContentId, ContentStatus};
use ferrite_plugin_host::{HostServices, KindOk, Post, SettingSpec, TermRef};
use ferrite_store::Store;

use super::apply::{mark_degraded, record, scoped_preview};

/// Most value bytes one plugin may keep in `kv` (design 05 §3).
pub const KV_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Most keys one plugin may keep in `kv` (design 05 §3).
pub const KV_MAX_KEYS: u64 = 10_000;
/// Error text for any storage failure; details go to the log, not to the guest.
const STORAGE_ERROR: &str = "storage error";
/// Option keys under this prefix (plugin settings, possibly secrets) are not readable
/// through `options.get`, whatever a manifest grants; a plugin reads its own settings
/// through `settings.get`.
const PLUGIN_OPTION_PREFIX: &str = "plugin.";

/// Store-backed services for the plugin imports. Capability and key-scope checks are
/// the host's (`ferrite-plugin-host`); this layer adds visibility:
///
/// * `content`: public items only (published, `published_at <= now`, not media), via
///   [`SiteReader`]. During a preview the previewed item is also returned by `get_post`
///   on its id (see [`super::PluginRequest`]). `get_post_by_slug` takes a page path for
///   hierarchical types (`"parent/child"`).
/// * `meta.get`: only on items `content` could return. `meta.set` writes any existing
///   item and emits `invalidate` with `post:<id>` in the same transaction.
/// * `options.get`: JSON text of the option; never `plugin.*` keys.
/// * `kv`: per plugin, quota [`KV_MAX_BYTES`] / [`KV_MAX_KEYS`] via `set_with_quota`.
/// * `settings.get`: option `plugin.<name>.settings.<key>` of the calling plugin (the
///   host passes its name and the loaded manifest's definition, and answers declared
///   keys only), resolved by `ext_settings::resolve_plugin_value` as the admin view
///   is (review R11): a `secret` only as a sealed value that opens with the site key
///   ([`with_site_key`](Self::with_site_key)) under that plugin's and key's purpose
///   (returned as a JSON string); a non-secret only unsealed and valid for its type.
///   Anything else is logged (plugin and key, never the value) and answers `None`, so
///   the host falls back to the declared default.
///
/// A store error is logged and the guest gets "not found" (reads) or a storage error
/// (writes); inside a request scope it also marks the page volatile, so the degraded
/// result gets the short TTL instead of staying cached.
///
/// `Post.author` is the author's display name (never a login or e-mail); `Post.url` is
/// absolute (site URL + path).
pub struct StoreHostServices {
    store: Arc<dyn Store>,
    reader: Arc<SiteReader>,
    /// Site URL without a trailing slash.
    site_url: String,
    /// Opens sealed plugin secrets; `None`: secrets read as unset.
    site_key: Option<SiteKey>,
}

impl StoreHostServices {
    pub fn new(store: Arc<dyn Store>, reader: Arc<SiteReader>, site_url: &str) -> Self {
        Self {
            store,
            reader,
            site_url: site_url.trim_end_matches('/').to_owned(),
            site_key: None,
        }
    }

    /// The site key `settings.get` opens sealed secrets with (the one `ExtensionAdmin`
    /// seals them with).
    pub fn with_site_key(mut self, key: SiteKey) -> Self {
        self.site_key = Some(key);
        self
    }

    fn post(&self, p: &PageItem, status: ContentStatus) -> Post {
        Post {
            id: p.item.id.to_string(),
            kind: p.item.kind.clone(),
            slug: p.item.slug.clone(),
            title: p.item.title.clone(),
            excerpt: p.item.excerpt.clone(),
            status: status.as_str().to_owned(),
            published: p.item.published_at.map(|t| t.to_rfc3339()),
            url: format!("{}{}", self.site_url, p.url),
            author: p
                .author
                .as_ref()
                .map(|a| a.display_name.clone())
                .unwrap_or_default(),
            terms: p
                .terms
                .iter()
                .map(|t| TermRef {
                    taxonomy: t.taxonomy.clone(),
                    slug: t.slug.clone(),
                    name: t.name.clone(),
                })
                .collect(),
        }
    }

    fn public_post(&self, p: &PageItem) -> Post {
        self.post(p, ContentStatus::Published)
    }

    /// Whether `meta.get` may read item `id`.
    async fn readable(&self, id: ContentId) -> bool {
        if scoped_preview(id).is_some() {
            return true;
        }
        match self.reader.is_visible(id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "plugin meta visibility check failed");
                mark_degraded();
                false
            }
        }
    }
}

/// The value of a read, recording its dependency keys into the request scope; `None`
/// (logged) on a store error.
fn take<T>(what: &str, read: Result<SiteRead<T>, ferrite_core::CoreError>) -> Option<T> {
    match read {
        Ok(r) => {
            record(r.deps);
            Some(r.value)
        }
        Err(e) => {
            tracing::warn!(error = %e, read = what, "plugin content read failed");
            mark_degraded();
            None
        }
    }
}

fn json_text(v: &serde_json::Value) -> String {
    v.to_string()
}

#[async_trait::async_trait]
impl HostServices for StoreHostServices {
    async fn get_post(&self, id: &str, kind_ok: &KindOk) -> Option<Post> {
        let id: ContentId = id.parse().ok()?;
        if let Some(p) = scoped_preview(id) {
            return Some(self.post(p.item(), p.status()));
        }
        let item = take("get_post", self.reader.by_id_where(id, kind_ok).await)??;
        Some(self.public_post(&item))
    }

    async fn get_post_by_slug(&self, kind: &str, slug: &str) -> Option<Post> {
        let parts: Vec<String> = slug.split('/').map(str::to_owned).collect();
        let item = take("get_post_by_slug", self.reader.single(kind, &parts).await)??;
        Some(self.public_post(&item))
    }

    async fn list_posts(&self, kind: &str, limit: u32, offset: u32) -> Vec<Post> {
        take("list_posts", self.reader.latest(kind, limit, offset).await)
            .unwrap_or_default()
            .iter()
            .map(|p| self.public_post(p))
            .collect()
    }

    async fn meta_get(&self, post: &str, key: &str) -> Option<String> {
        let id: ContentId = post.parse().ok()?;
        if !self.readable(id).await {
            return None;
        }
        match self.store.read().meta().get(id, key).await {
            Ok(v) => v.as_ref().map(json_text),
            Err(e) => {
                tracing::warn!(error = %e, key, "plugin meta read failed");
                mark_degraded();
                None
            }
        }
    }

    async fn meta_set(&self, post: &str, key: &str, value: &str) -> Result<(), String> {
        let id: ContentId = post.parse().map_err(|_| "invalid post id".to_owned())?;
        let value: serde_json::Value =
            serde_json::from_str(value).map_err(|_| "value is not valid JSON".to_owned())?;
        let write = async {
            let tx = self.store.begin().await?;
            if tx.content().get(id).await?.is_none() {
                tx.rollback().await?;
                return Ok(false);
            }
            tx.meta().set(id, key, &value).await?;
            tx.events().append(&invalidate([post_key(id)])).await?;
            tx.commit().await?;
            Ok::<_, ferrite_store::StoreError>(true)
        };
        match write.await {
            Ok(true) => Ok(()),
            Ok(false) => Err("no such post".to_owned()),
            Err(e) => {
                tracing::warn!(error = %e, key, "plugin meta write failed");
                mark_degraded();
                Err(STORAGE_ERROR.to_owned())
            }
        }
    }

    async fn option_get(&self, key: &str) -> Option<String> {
        if key.starts_with(PLUGIN_OPTION_PREFIX) {
            return None;
        }
        option_text(self.store.as_ref(), key).await
    }

    async fn kv_get(&self, plugin: &str, key: &str) -> Option<Vec<u8>> {
        match self.store.read().plugin_kv().get(plugin, key).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(plugin, error = %e, "plugin kv read failed");
                mark_degraded();
                None
            }
        }
    }

    async fn kv_set(&self, plugin: &str, key: &str, value: Vec<u8>) -> Result<(), String> {
        let kv = self.store.read().plugin_kv();
        match kv
            .set_with_quota(plugin, key, &value, KV_MAX_BYTES, KV_MAX_KEYS)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "kv quota exceeded ({KV_MAX_BYTES} bytes, {KV_MAX_KEYS} keys)"
            )),
            Err(e) => {
                tracing::warn!(plugin, error = %e, "plugin kv write failed");
                mark_degraded();
                Err(STORAGE_ERROR.to_owned())
            }
        }
    }

    async fn kv_delete(&self, plugin: &str, key: &str) {
        if let Err(e) = self.store.read().plugin_kv().delete(plugin, key).await {
            tracing::warn!(plugin, error = %e, "plugin kv delete failed");
            mark_degraded();
        }
    }

    async fn kv_list(&self, plugin: &str, prefix: &str, limit: u32) -> Vec<String> {
        match self
            .store
            .read()
            .plugin_kv()
            .list_keys(plugin, prefix, limit)
            .await
        {
            Ok(keys) => keys,
            Err(e) => {
                tracing::warn!(plugin, error = %e, "plugin kv list failed");
                mark_degraded();
                Vec::new()
            }
        }
    }

    async fn setting_get(&self, plugin: &str, key: &str, spec: &SettingSpec) -> Option<String> {
        let option = plugin_option(plugin, key);
        // The host records nothing for settings; a settings change is an option write.
        let mut deps = DepKeys::default();
        deps.insert(option_key(&option));
        record(deps);
        let stored = match self.store.read().options().get(&option).await {
            Ok(entry) => entry?.value,
            Err(e) => {
                tracing::warn!(key = option, error = %e, "plugin setting read failed");
                mark_degraded();
                return None;
            }
        };
        let def = setting_def(key, spec);
        let resolved = resolve_plugin_value(&def, Some(&stored), |sealed| {
            let Some(site_key) = &self.site_key else {
                tracing::warn!(
                    plugin,
                    setting = key,
                    "no site key to open a sealed plugin setting"
                );
                return None;
            };
            open_secret(site_key, plugin, key, sealed).ok()
        });
        if resolved.fell_back {
            tracing::warn!(
                plugin,
                setting = key,
                kind = def.kind.as_str(),
                "a stored plugin setting does not fit its declared type (or a sealed one does not open); answering the default"
            );
        }
        resolved.value.as_ref().map(json_text)
    }
}

/// The core definition of a loaded manifest's setting (the value rules are shared,
/// `ferrite_domain::check_setting_value`).
fn setting_def(key: &str, spec: &SettingSpec) -> SettingDef {
    SettingDef {
        key: key.to_owned(),
        kind: spec.kind.setting_type(),
        label: spec.label.clone(),
        default: spec
            .default
            .as_ref()
            .and_then(|d| serde_json::to_value(d).ok()),
        options: spec.options.clone(),
    }
}

/// Option `key` as JSON text; `None` if unset or on a store error (logged).
async fn option_text(store: &dyn Store, key: &str) -> Option<String> {
    match store.read().options().get(key).await {
        Ok(entry) => entry.map(|e| json_text(&e.value)),
        Err(e) => {
            tracing::warn!(key, error = %e, "plugin option read failed");
            mark_degraded();
            None
        }
    }
}
