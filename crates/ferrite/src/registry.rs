//! Which plugin registry this site uses (M6-2, design 14 §5.2): the configured URL with
//! its key, or the default URL with the key embedded from `crates/ferrite/registry.pub`.
//! Until the release owner creates that key the file is a placeholder, and a site on the
//! default URL has no registry (`config check` and doctor say so).

use anyhow::{Context, Result, bail};
use ferrite_config::{DEFAULT_REGISTRY_URL, PluginTrustConfig};
use ferrite_domain::plugin_trust::PublisherKey;
use ferrite_http::extensions::{RegistryTarget, TrustSettings};
use url::Url;

/// The embedded registry index key file.
const REGISTRY_PUB: &str = include_str!("../registry.pub");

/// The embedded registry key; `None` for the placeholder.
pub fn embedded_key() -> Result<Option<PublisherKey>> {
    PublisherKey::parse(REGISTRY_PUB).context("the embedded registry key is malformed")
}

/// The registry this site uses, or `None` (off: `registry = ""`, or the default URL with
/// no key in this build and none configured).
pub fn configured(cfg: &PluginTrustConfig) -> Result<Option<RegistryTarget>> {
    configured_with(cfg, embedded_key()?)
}

fn configured_with(
    cfg: &PluginTrustConfig,
    embedded: Option<PublisherKey>,
) -> Result<Option<RegistryTarget>> {
    let Some(url) = &cfg.registry else {
        return Ok(None);
    };
    let key = match &cfg.registry_key {
        Some(b64) => Some(
            PublisherKey::from_base64(b64).context("plugins.registry_key is not a minisign key")?,
        ),
        None if url == DEFAULT_REGISTRY_URL => embedded,
        None => bail!("plugins.registry_key is required for a registry other than the default"),
    };
    let Some(key) = key else {
        return Ok(None);
    };
    let url = Url::parse(url).context("plugins.registry is not a URL")?;
    Ok(Some(RegistryTarget { url, key }))
}

/// The install pipeline's trust settings for this configuration.
pub fn trust_settings(cfg: &PluginTrustConfig) -> Result<TrustSettings> {
    let registry = configured(cfg)?;
    let source = match &registry {
        None => None,
        Some(t) => {
            match ferrite_http::registry_fetch::source_for(&t.url, env!("CARGO_PKG_VERSION")) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot build the registry client; registry checks fail on this node");
                    None
                }
            }
        }
    };
    Ok(TrustSettings {
        policy: cfg.signatures,
        registry,
        source,
    })
}

/// How `config check` and doctor describe the registry setting.
pub fn describe(cfg: &PluginTrustConfig) -> String {
    match configured(cfg) {
        Ok(Some(t)) => format!("{} (key {})", t.url, t.key.key_id),
        Ok(None) if cfg.registry.is_none() => "off".to_owned(),
        Ok(None) => "off (this build has no registry key)".to_owned(),
        Err(e) => format!("invalid ({e:#})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACEHOLDER: &str = "# placeholder\n# holds no key\n";

    fn throwaway() -> PublisherKey {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        PublisherKey::parse(&kp.pk.to_box().unwrap().into_string())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn embedded_file_parses() {
        // A placeholder or a real key; never malformed.
        embedded_key().unwrap();
        assert!(PublisherKey::parse(PLACEHOLDER).unwrap().is_none());
    }

    #[test]
    fn placeholder_means_off_for_the_default_url() {
        let cfg = PluginTrustConfig::default();
        assert!(configured_with(&cfg, None).unwrap().is_none());
        let off = PluginTrustConfig {
            registry: None,
            ..Default::default()
        };
        assert!(configured_with(&off, Some(throwaway())).unwrap().is_none());
        assert_eq!(describe(&off), "off");
    }

    #[test]
    fn a_configured_key_turns_it_on() {
        let key = throwaway();
        let cfg = PluginTrustConfig {
            registry: Some("file:///srv/registry/index.json".into()),
            registry_key: Some(key.base64.clone()),
            ..Default::default()
        };
        let t = configured_with(&cfg, None).unwrap().unwrap();
        assert_eq!(t.url.as_str(), "file:///srv/registry/index.json");
        assert_eq!(t.key, key);
        // The default URL with a configured key: on, with that key.
        let cfg = PluginTrustConfig {
            registry_key: Some(key.base64.clone()),
            ..Default::default()
        };
        let t = configured_with(&cfg, None).unwrap().unwrap();
        assert_eq!(t.url.as_str(), DEFAULT_REGISTRY_URL);
        // The default URL with the embedded key.
        let embedded = throwaway();
        let t = configured_with(&PluginTrustConfig::default(), Some(embedded.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(t.key, embedded);
    }
}
