//! The plugin registry in the install pipeline (M6-2, design 14 §5, §8): refreshing the
//! cached index, the view the Plugins screen and `ferrite plugin search` show, and
//! inspecting a registry version (fetch, size and sha256 against the index, then the
//! same signature path as an upload).

use bytes::Bytes;
use ferrite_core::{CoreError, ExtensionService, RegistryRefresh};
use ferrite_domain::plugin_registry::{
    MAX_INDEX_BYTES, RegistryIndex, RegistryPlugin, RegistryVersion, is_newer,
};
use ferrite_domain::plugin_trust::{
    InstallSource, MAX_SIGNATURE_BYTES, OwnerId, PluginOwner, RegistryCacheRow,
};
use ferrite_domain::{InstalledPlugin, Timestamp};
use ferrite_plugin_host::WORLD;
use ferrite_plugin_host::trust::{IndexError, verify_index};
use sha2::{Digest, Sha256};
use url::Url;

use super::{BundleSignature, ExtensionError, Inspected, Installer, task_error};
use crate::registry_fetch::{COMPONENT_TIMEOUT, INDEX_TIMEOUT, RegistryFetchError, resolve};

/// Most search results.
pub const MAX_REGISTRY_RESULTS: usize = 50;

/// The configured registry and its cache, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryMeta {
    pub url: String,
    /// From the cached index; `None` before the first successful refresh.
    pub name: Option<String>,
    pub serial: Option<i64>,
    pub generated_at: Option<Timestamp>,
    pub checked_at: Option<Timestamp>,
    pub last_error: Option<String>,
    pub last_error_at: Option<Timestamp>,
}

/// An installed plugin's state against the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledState {
    pub version: String,
    pub update_available: bool,
    pub publisher_matches: bool,
}

/// One plugin of the cached index, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPluginView {
    pub plugin: RegistryPlugin,
    /// The publisher's display name.
    pub publisher_name: String,
    /// The newest version this binary can run, not yanked.
    pub latest: Option<String>,
    pub installed: Option<InstalledState>,
}

/// [`Installer::registry_view`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryView {
    /// `None` when no registry is configured.
    pub registry: Option<RegistryMeta>,
    pub plugins: Vec<RegistryPluginView>,
}

/// The update the registry offers an installed plugin (design 14 §8): a newer,
/// non-yanked version of this world, offered only when the registry publisher owns the
/// name or the name is unsigned-owned.
pub fn update_for(
    index: &RegistryIndex,
    registry_key: &ferrite_domain::plugin_trust::KeyId,
    row: &InstalledPlugin,
    owner: Option<&PluginOwner>,
) -> Option<(String, bool)> {
    let entry = index.plugin(&row.name)?;
    let newest = index.newest(&row.name, WORLD, false)?;
    if !is_newer(&newest.version, &row.version) {
        return None;
    }
    let identity = OwnerId::Registry {
        registry_key: *registry_key,
        publisher: entry.publisher.clone(),
    };
    match owner.and_then(|o| o.owner.as_ref()) {
        None => Some((newest.version.clone(), false)),
        Some(o) if *o == identity => Some((newest.version.clone(), true)),
        // A key of this registry's publisher bound before the key was listed.
        Some(OwnerId::Key(k))
            if index
                .publisher_of_key(k)
                .is_some_and(|p| p.id == entry.publisher) =>
        {
            Some((newest.version.clone(), true))
        }
        Some(_) => None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    ferrite_domain::to_hex(&Sha256::digest(bytes))
}

/// `<index>.minisig`.
fn signature_url(index: &Url) -> Result<Url, ExtensionError> {
    Url::parse(&format!("{}.minisig", index.as_str()))
        .map_err(|e| ExtensionError::RegistryInvalid(e.to_string()))
}

fn fetch_error(what: &str, e: RegistryFetchError) -> ExtensionError {
    match e {
        RegistryFetchError::Refused(m) => {
            ExtensionError::RegistryInvalid(format!("the {what} URL is refused: {m}"))
        }
        RegistryFetchError::TooLarge => {
            ExtensionError::RegistryInvalid(format!("the {what} is larger than allowed"))
        }
        RegistryFetchError::NotFound => {
            ExtensionError::RegistryUnavailable(format!("the {what} was not found"))
        }
        RegistryFetchError::Failed(m) => {
            ExtensionError::RegistryUnavailable(format!("the {what} could not be fetched: {m}"))
        }
    }
}

impl Installer {
    fn service(&self) -> ExtensionService {
        ExtensionService::new(self.store.clone())
    }

    /// Fetches the configured registry's index and signature, verifies them with the
    /// registry key, stores the copy (serial rule) and applies its revocations (design
    /// 14 §8). A failure is recorded on the cache row (which stays) and returned:
    /// `RegistryUnavailable` (fetch), `RegistryInvalid` (signature, schema, rollback),
    /// `NoRegistry`.
    pub async fn refresh_registry(&self) -> Result<RegistryRefresh, ExtensionError> {
        let (Some(target), Some(source)) = (&self.trust.registry, &self.trust.source) else {
            return Err(ExtensionError::NoRegistry);
        };
        let url = target.url.as_str().to_owned();
        let done = async {
            let index = source
                .get(&target.url, MAX_INDEX_BYTES, INDEX_TIMEOUT)
                .await
                .map_err(|e| fetch_error("index", e))?;
            let sig = source
                .get(
                    &signature_url(&target.url)?,
                    MAX_SIGNATURE_BYTES as u64,
                    INDEX_TIMEOUT,
                )
                .await
                .map_err(|e| fetch_error("index signature", e))?;
            let sig = String::from_utf8(sig.to_vec()).map_err(|_| {
                ExtensionError::RegistryInvalid("the index signature is not UTF-8".into())
            })?;
            let json = String::from_utf8(index.to_vec())
                .map_err(|_| ExtensionError::RegistryInvalid("the index is not UTF-8".into()))?;
            let key = target.key.clone();
            let (bytes, s) = (json.clone(), sig.clone());
            let parsed =
                tokio::task::spawn_blocking(move || verify_index(&key, bytes.as_bytes(), &s))
                    .await
                    .map_err(task_error)?
                    .map_err(|e| match e {
                        IndexError::Signature(_) | IndexError::Invalid(_) => {
                            ExtensionError::RegistryInvalid(e.to_string())
                        }
                    })?;
            let now = Timestamp::now();
            let row = RegistryCacheRow {
                url: url.clone(),
                serial: i64::try_from(parsed.serial).unwrap_or(i64::MAX),
                index_json: json,
                signature: sig,
                generated_at: parsed.generated_at,
                fetched_at: now,
                checked_at: now,
                last_error: None,
                last_error_at: None,
            };
            match self.service().store_registry(&row, &parsed).await {
                Ok(r) => Ok(r),
                Err(CoreError::InvalidField { message, .. }) => {
                    Err(ExtensionError::RegistryInvalid(message))
                }
                Err(e) => Err(e.into()),
            }
        }
        .await;
        if let Err(e) = &done
            && matches!(
                e,
                ExtensionError::RegistryInvalid(_) | ExtensionError::RegistryUnavailable(_)
            )
        {
            tracing::warn!(registry = %url, error = %e, "registry refresh failed");
            if let Err(rec) = self.service().registry_failed(&url, &e.to_string()).await {
                tracing::warn!(error = %rec, "cannot record the registry error");
            }
        }
        done
    }

    /// The cached index, searched (`q`: name, description, publisher name; at most
    /// [`MAX_REGISTRY_RESULTS`]), with each plugin's installed state. Reads only.
    pub async fn registry_view(&self, q: Option<&str>) -> Result<RegistryView, ExtensionError> {
        let Some(target) = &self.trust.registry else {
            return Ok(RegistryView {
                registry: None,
                plugins: Vec::new(),
            });
        };
        let cache = self.service().registry_cache(target.url.as_str()).await?;
        let ctx = self.registry_context().await?;
        let meta = RegistryMeta {
            url: target.url.to_string(),
            name: ctx.as_ref().map(|c| c.index.name.clone()),
            serial: cache.as_ref().map(|c| c.serial),
            generated_at: cache.as_ref().map(|c| c.generated_at),
            checked_at: cache.as_ref().map(|c| c.checked_at),
            last_error: cache.as_ref().and_then(|c| c.last_error.clone()),
            last_error_at: cache.as_ref().and_then(|c| c.last_error_at),
        };
        let Some(ctx) = ctx else {
            return Ok(RegistryView {
                registry: Some(meta),
                plugins: Vec::new(),
            });
        };
        let rows = self
            .store
            .read()
            .plugins()
            .list()
            .await
            .map_err(CoreError::from)?;
        let owners = self
            .store
            .read()
            .plugin_trust()
            .list_owners()
            .await
            .map_err(CoreError::from)?;
        let plugins = ctx
            .index
            .search(q.unwrap_or_default(), MAX_REGISTRY_RESULTS)
            .into_iter()
            .map(|p| {
                let installed = rows.iter().find(|r| r.name == p.name).map(|row| {
                    let owner = owners.iter().find(|o| o.plugin == row.name);
                    let update = update_for(&ctx.index, &ctx.registry_key, row, owner);
                    InstalledState {
                        version: row.version.clone(),
                        update_available: update.is_some(),
                        publisher_matches: update.as_ref().is_some_and(|(_, m)| *m)
                            || owner.and_then(|o| o.owner.as_ref())
                                == Some(&OwnerId::Registry {
                                    registry_key: ctx.registry_key,
                                    publisher: p.publisher.clone(),
                                }),
                    }
                });
                RegistryPluginView {
                    publisher_name: ctx
                        .index
                        .publisher(&p.publisher)
                        .map(|pb| pb.name.clone())
                        .unwrap_or_default(),
                    latest: ctx
                        .index
                        .newest(&p.name, WORLD, false)
                        .map(|v| v.version.clone()),
                    installed,
                    plugin: p.clone(),
                }
            })
            .collect();
        Ok(RegistryView {
            registry: Some(meta),
            plugins,
        })
    }

    /// Inspects a registry version (default: the newest this binary runs, not yanked):
    /// fetches its manifest and component, checks their sizes and sha256 against the
    /// cached index, then inspects them as a signed bundle with the entry's signature
    /// ([`inspect_bundle`](Self::inspect_bundle), source `registry`). The signer must be
    /// a current key of the entry's publisher. Nothing is written.
    pub async fn inspect_registry(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Inspected, ExtensionError> {
        let (Some(_), Some(source)) = (&self.trust.registry, &self.trust.source) else {
            return Err(ExtensionError::NoRegistry);
        };
        let Some(ctx) = self.registry_context().await? else {
            return Err(ExtensionError::RegistryUnavailable(
                "the registry has not been fetched yet; check for updates first".into(),
            ));
        };
        let entry: &RegistryVersion = match version {
            Some(v) => ctx.index.version(name, v),
            None => ctx.index.newest(name, WORLD, false),
        }
        .ok_or(ExtensionError::Core(CoreError::NotFound))?;
        if entry.world != WORLD {
            return Err(ExtensionError::RegistryInvalid(format!(
                "{name} {} is built for {}, this binary runs {WORLD}",
                entry.version, entry.world
            )));
        }
        let publisher = ctx
            .index
            .plugin(name)
            .map(|p| p.publisher.clone())
            .unwrap_or_default();
        let index_url =
            Url::parse(&ctx.url).map_err(|e| ExtensionError::RegistryInvalid(e.to_string()))?;
        let fetch = |what: &'static str, file: &ferrite_domain::plugin_registry::RegistryFile| {
            let index_url = index_url.clone();
            let file = file.clone();
            let source = source.clone();
            async move {
                let url = resolve(&index_url, &file.url).map_err(|e| fetch_error(what, e))?;
                let bytes: Bytes = source
                    .get(&url, file.bytes, COMPONENT_TIMEOUT)
                    .await
                    .map_err(|e| fetch_error(what, e))?;
                if bytes.len() as u64 != file.bytes || sha256_hex(&bytes) != file.sha256 {
                    return Err(ExtensionError::RegistryInvalid(format!(
                        "the {what} does not match the registry index (size or sha256)"
                    )));
                }
                Ok(bytes)
            }
        };
        let manifest = fetch("manifest", &entry.manifest).await?;
        let component = fetch("component", &entry.component).await?;
        let manifest = String::from_utf8(manifest.to_vec())
            .map_err(|_| ExtensionError::RegistryInvalid("the manifest is not UTF-8".into()))?;
        let inspected = self
            .inspect_bundle(
                &manifest,
                component,
                Some(BundleSignature {
                    minisig: entry.signature.clone(),
                    publisher_key: None,
                }),
                InstallSource::Registry,
            )
            .await?;
        if inspected.manifest().plugin.version.to_string() != entry.version {
            return Err(ExtensionError::RegistryInvalid(format!(
                "the registry's {name} {} manifest says version {}",
                entry.version,
                inspected.manifest().plugin.version
            )));
        }
        if inspected.manifest().plugin.name != name {
            return Err(ExtensionError::RegistryInvalid(format!(
                "the registry's {name} manifest names plugin {}",
                inspected.manifest().plugin.name
            )));
        }
        let signer = inspected.report().signer;
        let by_publisher = signer
            .and_then(|k| ctx.index.publisher_of_key(&k))
            .is_some_and(|p| p.id == publisher);
        if !by_publisher {
            return Err(ExtensionError::RegistryInvalid(format!(
                "{name} {} is not signed by a current key of its publisher {publisher}",
                entry.version
            )));
        }
        Ok(inspected)
    }
}
