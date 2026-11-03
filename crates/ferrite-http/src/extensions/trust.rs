//! Signed plugins in the install pipeline (M6-2, design 14 §2–§4): the signature parts of
//! an upload or the files of a folder are verified here, before core decides the owner
//! rule, and the permission report's fingerprint carries the signer (D11).

use std::sync::Arc;

use ferrite_core::{
    CoreError, ExtensionService, InstallTrust, RegistryContext, SignerInput, TrustDecision,
};
use ferrite_domain::plugin_registry::RegistryIndex;
use ferrite_domain::plugin_trust::{
    InstallSource, KeyId, MAX_PUBLISHER_KEY_BYTES, MAX_SIGNATURE_BYTES, PluginOwner, PublisherKey,
    SignaturePolicy, TrustedKey,
};
use ferrite_plugin_host::trust::{TrustError, verify_bundle, verify_index};

use super::{ExtensionError, Installer, RegistryTarget};
use crate::registry_fetch::RegistrySource;

/// A signature sent with a plugin: the minisig text and, optionally, the publisher's
/// public key file (trust on first use).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleSignature {
    pub minisig: String,
    pub publisher_key: Option<String>,
}

impl BundleSignature {
    /// From the raw parts or files, checking their sizes and UTF-8: a bad signature part
    /// is a `Trust` refusal (field `signature`), a bad key part `PublisherKey`.
    pub fn from_bytes(
        minisig: &[u8],
        publisher_key: Option<&[u8]>,
    ) -> Result<BundleSignature, ExtensionError> {
        if minisig.len() > MAX_SIGNATURE_BYTES {
            return Err(ExtensionError::Trust(TrustError::TooLarge));
        }
        let minisig = std::str::from_utf8(minisig)
            .map_err(|_| ExtensionError::Trust(TrustError::Malformed("not UTF-8".into())))?
            .to_owned();
        let publisher_key = match publisher_key {
            None => None,
            Some(k) if k.len() > MAX_PUBLISHER_KEY_BYTES => {
                return Err(ExtensionError::PublisherKey(format!(
                    "the publisher key is larger than {MAX_PUBLISHER_KEY_BYTES} bytes"
                )));
            }
            Some(k) => Some(
                std::str::from_utf8(k)
                    .map_err(|_| ExtensionError::PublisherKey("not UTF-8".into()))?
                    .to_owned(),
            ),
        };
        Ok(BundleSignature {
            minisig,
            publisher_key,
        })
    }
}

/// The operator's trust configuration as the pipeline uses it.
#[derive(Clone, Default)]
pub struct TrustSettings {
    /// `[plugins] signatures`.
    pub policy: SignaturePolicy,
    /// The configured registry, if any.
    pub registry: Option<RegistryTarget>,
    /// Where the registry's files are fetched from (`None`: no registry, or no client
    /// could be built on this node; refreshes and registry installs then fail).
    pub source: Option<Arc<dyn RegistrySource>>,
}

impl std::fmt::Debug for TrustSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustSettings")
            .field("policy", &self.policy)
            .field("registry", &self.registry)
            .field("source", &self.source.is_some())
            .finish()
    }
}

/// What a listing needs to describe installed plugins' trust: the owner rows, the site's
/// key rows and the verified cached index.
#[derive(Debug, Clone, Default)]
pub struct TrustContext {
    pub policy: SignaturePolicy,
    pub owners: Vec<PluginOwner>,
    pub keys: Vec<TrustedKey>,
    pub registry: Option<RegistryContext>,
}

impl TrustContext {
    pub fn owner(&self, name: &str) -> Option<&PluginOwner> {
        self.owners.iter().find(|o| o.plugin == name)
    }

    pub fn key(&self, id: &KeyId) -> Option<&TrustedKey> {
        self.keys.iter().find(|k| k.key.key_id == *id)
    }

    /// Whether the key that signed `owner`'s installed version is revoked here or by the
    /// registry (the registry's revocations matched by the whole key).
    pub fn revoked(&self, owner: &PluginOwner) -> bool {
        let Some(id) = owner.signer_key else {
            return false;
        };
        self.key(&id).is_some_and(|k| k.revoked.is_some())
            || owner.signer().is_some_and(|k| {
                self.registry
                    .as_ref()
                    .is_some_and(|r| r.index.revocation(&k).is_some())
            })
    }
}

impl Installer {
    /// The installer with the operator's trust settings (default: policy `any`, no
    /// registry).
    pub fn with_trust(mut self, trust: TrustSettings) -> Self {
        self.trust = trust;
        self
    }

    /// The trust settings.
    pub fn trust_settings(&self) -> &TrustSettings {
        &self.trust
    }

    /// The configured registry with its cached index, verified again with the
    /// configured key (a cache written under another key, or a tampered row, is not
    /// used). `None` without a registry or a usable cache.
    pub async fn registry_context(&self) -> Result<Option<RegistryContext>, ExtensionError> {
        let Some(target) = &self.trust.registry else {
            return Ok(None);
        };
        let url = target.url.as_str();
        let Some(row) = self
            .store
            .read()
            .plugin_trust()
            .get_registry(url)
            .await
            .map_err(CoreError::from)?
        else {
            return Ok(None);
        };
        let key = target.key.clone();
        let parsed = tokio::task::spawn_blocking(move || {
            verify_index(&key, row.index_json.as_bytes(), &row.signature)
        })
        .await
        .map_err(super::task_error)?;
        match parsed {
            Ok(index) => Ok(Some(RegistryContext {
                url: url.to_owned(),
                registry_key: target.key.key_id,
                index: Arc::new(index),
            })),
            Err(e) => {
                tracing::warn!(registry = %url, error = %e, "the cached registry index does not verify; ignoring it");
                Ok(None)
            }
        }
    }

    /// Everything a listing needs to show installed plugins' trust.
    pub async fn trust_context(&self) -> Result<TrustContext, ExtensionError> {
        let read = self.store.read().plugin_trust();
        Ok(TrustContext {
            policy: self.trust.policy,
            owners: read.list_owners().await.map_err(CoreError::from)?,
            keys: read.list_keys().await.map_err(CoreError::from)?,
            registry: self.registry_context().await?,
        })
    }

    /// Verifies `signature` over the two files (design 14 §2.3) against the given key,
    /// the site's keys and the cached index's keys; returns the signer.
    pub(super) async fn verify_signature(
        &self,
        manifest: &[u8],
        component: &[u8],
        signature: &BundleSignature,
        registry: Option<&RegistryIndex>,
    ) -> Result<SignerInput, ExtensionError> {
        let mut candidates: Vec<PublisherKey> = self
            .store
            .read()
            .plugin_trust()
            .list_keys()
            .await
            .map_err(CoreError::from)?
            .into_iter()
            .map(|k| k.key)
            .collect();
        if let Some(index) = registry {
            for p in &index.publishers {
                candidates.extend(p.keys.iter().cloned());
                candidates.extend(p.revoked_keys.iter().map(|r| r.key.clone()));
            }
        }
        if let Some(text) = &signature.publisher_key {
            match PublisherKey::parse(text) {
                Ok(Some(key)) => candidates.push(key),
                Ok(None) => {
                    return Err(ExtensionError::PublisherKey(
                        "the publisher key part holds no key".into(),
                    ));
                }
                Err(e) => return Err(ExtensionError::PublisherKey(e.to_string())),
            }
        }
        let (m, c, sig) = (
            manifest.to_vec(),
            component.to_vec(),
            signature.minisig.clone(),
        );
        let verified =
            tokio::task::spawn_blocking(move || verify_bundle(&m, &c, &sig, &candidates))
                .await
                .map_err(super::task_error)?
                .map_err(ExtensionError::Trust)?;
        Ok(SignerInput {
            key: verified.key,
            signature: signature.minisig.clone(),
        })
    }

    /// The trust input of an upload and its decision on committed state.
    pub(super) async fn decide_trust(
        &self,
        name: &str,
        claim: &str,
        signer: Option<SignerInput>,
        source: InstallSource,
        registry: Option<RegistryContext>,
    ) -> Result<(InstallTrust, TrustDecision), ExtensionError> {
        let trust = InstallTrust {
            signer,
            publisher_claim: claim.to_owned(),
            policy: self.trust.policy,
            source,
            registry,
        };
        let decision = ExtensionService::new(self.store.clone())
            .trust_preview(name, &trust)
            .await?;
        Ok((trust, decision))
    }
}
