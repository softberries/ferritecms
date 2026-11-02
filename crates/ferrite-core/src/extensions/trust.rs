//! Plugin trust decisions (M6-2, design 14 §3–§4): who signed an upload, whose data the
//! plugin name holds, and whether the install may go ahead. The signature itself is
//! verified before core sees it (`ferrite-plugin-host::trust`); core decides with
//! committed state, under the `extensions` lock, inside the install transaction, so a
//! racing install or erase is decided on what is stored.

use std::sync::Arc;

use ferrite_domain::plugin_registry::RegistryIndex;
use ferrite_domain::plugin_trust::{
    InstallSource, KeyId, KeySource, OwnerId, PluginOwner, PublisherKey, Revocation,
    SignaturePolicy, TrustedKey,
};
use ferrite_domain::{NewAudit, Timestamp};
use ferrite_store::Repos;

use super::{deleted_marker, erased_marker};
use crate::{CoreError, events};

/// The longest label recorded for a key approved at install (the manifest's claim).
const MAX_LABEL_CHARS: usize = 100;

/// A verified signature of an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerInput {
    /// The key that verified the signature.
    pub key: PublisherKey,
    /// The minisig text.
    pub signature: String,
}

/// The configured registry with its verified cached index (the caller verified the
/// cached signature with `registry_key`).
#[derive(Debug, Clone)]
pub struct RegistryContext {
    pub url: String,
    pub registry_key: KeyId,
    pub index: Arc<RegistryIndex>,
}

/// What core needs to decide an install's trust.
#[derive(Debug, Clone)]
pub struct InstallTrust {
    pub signer: Option<SignerInput>,
    /// The manifest's `publisher` text.
    pub publisher_claim: String,
    pub policy: SignaturePolicy,
    pub source: InstallSource,
    pub registry: Option<RegistryContext>,
}

impl InstallTrust {
    /// An unsigned upload under policy `any` (the pre-M6-2 install).
    pub fn unsigned(claim: impl Into<String>) -> Self {
        Self {
            signer: None,
            publisher_claim: claim.into(),
            policy: SignaturePolicy::Any,
            source: InstallSource::Upload,
            registry: None,
        }
    }
}

/// How the signer is known to this site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignerSource {
    /// Listed by the configured registry under this publisher.
    Registry { publisher_id: String, name: String },
    /// Approved at an earlier install.
    Approved,
    /// Added by the operator.
    Operator,
    /// Not known yet: approving the report trusts it.
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerView {
    pub key_id: KeyId,
    pub source: SignerSource,
    /// The manifest's `publisher` text.
    pub claimed: String,
}

/// What an install does to the name's owner (design 14 §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerChange {
    /// No owner and no data: bound to the signer.
    Bind,
    /// No owner and no data: bound unsigned.
    BindUnsigned,
    /// The owner is one of the signer's identities.
    Same,
    /// Unsigned-owned data, signed upload: bound to the signer from now on.
    Claim,
    /// Unsigned-owned data, unsigned upload: as before M6-2.
    UnsignedUpdate,
}

impl OwnerChange {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnerChange::Bind => "bind",
            OwnerChange::BindUnsigned => "bind_unsigned",
            OwnerChange::Same => "same",
            OwnerChange::Claim => "claim",
            OwnerChange::UnsignedUpdate => "unsigned_update",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustWarning {
    Unsigned,
    NewKey,
    UnsignedDataClaimed,
    UnsignedReadsData,
}

impl TrustWarning {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustWarning::Unsigned => "unsigned",
            TrustWarning::NewKey => "new_key",
            TrustWarning::UnsignedDataClaimed => "unsigned_data_claimed",
            TrustWarning::UnsignedReadsData => "unsigned_reads_data",
        }
    }
}

/// The trust part of a permission report, and what the install writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustDecision {
    pub signer: Option<SignerView>,
    pub owner_change: OwnerChange,
    pub current_owner: Option<OwnerId>,
    pub next_owner: Option<OwnerId>,
    pub warnings: Vec<TrustWarning>,
}

/// A refusal on `signature`.
fn refuse_signature(message: impl Into<String>) -> CoreError {
    CoreError::invalid_field("signature", message)
}

/// A refusal on `publisher`.
fn refuse_publisher(message: impl Into<String>) -> CoreError {
    CoreError::invalid_field("publisher", message)
}

/// How an owner id reads in a message.
pub fn owner_label(owner: &OwnerId, registry: Option<&RegistryContext>) -> String {
    match owner {
        OwnerId::Registry {
            registry_key,
            publisher,
        } => {
            let name = registry
                .filter(|r| r.registry_key == *registry_key)
                .and_then(|r| r.index.publisher(publisher))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| publisher.clone());
            format!("{name} (registry)")
        }
        OwnerId::Key(id) => format!("key {id}"),
    }
}

/// Whether the name holds data an install would read: an installed row, or a deletion
/// marker (retained data).
async fn has_data(repos: &dyn Repos, name: &str) -> Result<bool, CoreError> {
    Ok(repos.plugins().get(name).await?.is_some()
        || repos.options().get(&deleted_marker(name)).await?.is_some())
}

/// Whether the name's data is being erased by a `purge=true` delete (the marker kept
/// for the final sweep).
async fn being_erased(repos: &dyn Repos, name: &str) -> Result<bool, CoreError> {
    if repos.plugins().get(name).await?.is_some() {
        return Ok(false);
    }
    let marker = repos.options().get(&deleted_marker(name)).await?;
    let erased = repos.options().get(&erased_marker(name)).await?;
    Ok(match (marker, erased) {
        (Some(m), Some(e)) => m.value.as_i64().is_some() && m.value == e.value,
        _ => false,
    })
}

/// Decides an install's trust on `repos`' state (a read connection for the report, the
/// install transaction for the write). Refusals are `InvalidField` on `signature`
/// (revoked key, policy) or `publisher` (owner mismatch, registry publisher text).
pub async fn decide(
    repos: &dyn Repos,
    name: &str,
    trust: &InstallTrust,
) -> Result<TrustDecision, CoreError> {
    let owner_row = repos.plugin_trust().get_owner(name).await?;
    let mut warnings = Vec::new();

    // The signer and its identities.
    let mut identities: Vec<OwnerId> = Vec::new();
    let signer = match &trust.signer {
        None => {
            if trust.policy != SignaturePolicy::Any {
                return Err(refuse_signature(format!(
                    "this site installs signed plugins only ([plugins] signatures = \"{}\")",
                    trust.policy.as_str()
                )));
            }
            warnings.push(TrustWarning::Unsigned);
            None
        }
        Some(s) => {
            let id = s.key.key_id;
            if let Some(reg) = &trust.registry
                && let Some((_, revoked)) = reg.index.revocation(&id)
            {
                return Err(refuse_signature(format!(
                    "the signing key {id} was revoked by the registry: {}",
                    revoked.reason
                )));
            }
            let site = repos.plugin_trust().get_key(&id).await?;
            if let Some(revoked) = site.as_ref().and_then(|k| k.revoked.as_ref()) {
                return Err(refuse_signature(format!(
                    "the signing key {id} was revoked on this site: {}",
                    revoked.reason
                )));
            }
            if let Some(site) = &site
                && site.key.base64 != s.key.base64
            {
                return Err(refuse_signature(format!(
                    "key {id} does not match the key this site trusts under that id"
                )));
            }
            let registry_publisher = trust.registry.as_ref().and_then(|reg| {
                reg.index
                    .publisher_of_key(&id)
                    .map(|p| (reg.registry_key, p.id.clone(), p.name.clone()))
            });
            identities.push(OwnerId::Key(id));
            let source = if let Some((registry_key, publisher_id, pname)) = &registry_publisher {
                if trust.publisher_claim != *publisher_id {
                    return Err(refuse_publisher(format!(
                        "the manifest says the publisher is `{}`, but the registry lists this \
                         key under `{publisher_id}`",
                        trust.publisher_claim
                    )));
                }
                identities.insert(
                    0,
                    OwnerId::Registry {
                        registry_key: *registry_key,
                        publisher: publisher_id.clone(),
                    },
                );
                SignerSource::Registry {
                    publisher_id: publisher_id.clone(),
                    name: pname.clone(),
                }
            } else {
                match site.as_ref().map(|k| k.source) {
                    Some(KeySource::Operator) => SignerSource::Operator,
                    Some(KeySource::Approved) => SignerSource::Approved,
                    // A registry revocation row is revoked (refused above); a registry
                    // row that is not revoked cannot exist, but it counts as approved.
                    Some(KeySource::Registry) => SignerSource::Approved,
                    None => SignerSource::New,
                }
            };
            if source == SignerSource::New {
                if trust.policy == SignaturePolicy::Trusted {
                    return Err(refuse_signature(format!(
                        "key {id} is new to this site, and this site installs plugins signed \
                         by trusted keys only ([plugins] signatures = \"trusted\")"
                    )));
                }
                warnings.push(TrustWarning::NewKey);
            }
            Some(SignerView {
                key_id: id,
                source,
                claimed: trust.publisher_claim.clone(),
            })
        }
    };

    // The owner rule (design 14 §4.2).
    let current_owner = owner_row.as_ref().and_then(|o| o.owner.clone());
    let unsigned_owned = match &owner_row {
        Some(row) => row.owner.is_none(),
        None => has_data(repos, name).await?,
    };
    let (owner_change, next_owner) = match (&current_owner, &signer) {
        (Some(owner), Some(_)) if identities.contains(owner) => {
            (OwnerChange::Same, Some(owner.clone()))
        }
        (Some(owner), _) => {
            if being_erased(repos, name).await? {
                return Err(refuse_publisher(
                    "this plugin's data is being erased; try again in a few minutes",
                ));
            }
            return Err(refuse_publisher(format!(
                "the data of `{name}` belongs to publisher {}. Delete the plugin and erase its \
                 data first, or install it under another name.",
                owner_label(owner, trust.registry.as_ref())
            )));
        }
        (None, Some(_)) => {
            let next = identities.first().cloned();
            if unsigned_owned {
                warnings.push(TrustWarning::UnsignedDataClaimed);
                (OwnerChange::Claim, next)
            } else {
                (OwnerChange::Bind, next)
            }
        }
        (None, None) => {
            if unsigned_owned {
                warnings.push(TrustWarning::UnsignedReadsData);
                (OwnerChange::UnsignedUpdate, None)
            } else {
                (OwnerChange::BindUnsigned, None)
            }
        }
    };
    Ok(TrustDecision {
        signer,
        owner_change,
        current_owner,
        next_owner,
        warnings,
    })
}

/// Writes what `decision` says inside the install transaction: a new key approved by
/// this install, and the owner row.
pub(super) async fn write_trust(
    repos: &dyn Repos,
    name: &str,
    trust: &InstallTrust,
    decision: &TrustDecision,
    now: Timestamp,
) -> Result<(), CoreError> {
    if let (Some(view), Some(input)) = (&decision.signer, &trust.signer)
        && view.source == SignerSource::New
    {
        let label: String = trust
            .publisher_claim
            .chars()
            .take(MAX_LABEL_CHARS)
            .collect();
        repos
            .plugin_trust()
            .upsert_key(&TrustedKey {
                key: input.key.clone(),
                label,
                source: KeySource::Approved,
                trusted_at: now,
                revoked: None,
            })
            .await?;
    }
    repos
        .plugin_trust()
        .put_owner(&PluginOwner {
            plugin: name.to_owned(),
            owner: decision.next_owner.clone(),
            signer_key: trust.signer.as_ref().map(|s| s.key.key_id),
            signature: trust.signer.as_ref().map(|s| s.signature.clone()),
            publisher: trust.publisher_claim.clone(),
            source: trust.source,
            registry_url: trust
                .registry
                .as_ref()
                .map(|r| r.url.clone())
                .filter(|_| trust.source == InstallSource::Registry),
            bound_at: now,
            updated_at: now,
        })
        .await?;
    Ok(())
}

/// Records a revocation of `key` (inserting its row when the site has none) and, when
/// `compromised`, disables every installed plugin whose current version it signed, with
/// `plugins.changed` per plugin. Returns the disabled plugins. `newly_compromised_only`:
/// skip the disabling when the row was already revoked as compromised (a registry check
/// that sees the same revocation again).
pub(super) async fn write_revocation(
    repos: &dyn Repos,
    key: &PublisherKey,
    revocation: &Revocation,
    label_if_new: &str,
    source_if_new: KeySource,
    audit: Option<NewAudit>,
) -> Result<Vec<String>, CoreError> {
    let trust = repos.plugin_trust();
    let before = trust.get_key(&key.key_id).await?;
    let was_compromised = before
        .as_ref()
        .and_then(|k| k.revoked.as_ref())
        .is_some_and(|r| r.compromised);
    match &before {
        Some(_) => {
            trust.revoke_key(&key.key_id, revocation).await?;
        }
        None => {
            trust
                .upsert_key(&TrustedKey {
                    key: key.clone(),
                    label: label_if_new.chars().take(MAX_LABEL_CHARS).collect(),
                    source: source_if_new,
                    trusted_at: revocation.at,
                    revoked: Some(revocation.clone()),
                })
                .await?;
        }
    }
    let mut disabled = Vec::new();
    if revocation.compromised && !was_compromised {
        let reason = format!("publisher key revoked: {}", revocation.reason);
        for owner in trust.owners_signed_by(&key.key_id).await? {
            let Some(row) = repos.plugins().get_for_update(&owner.plugin).await? else {
                continue;
            };
            if !row.enabled {
                continue;
            }
            repos
                .plugins()
                .set_enabled(&owner.plugin, false, Some(&reason))
                .await?;
            repos
                .events()
                .append(&events::plugins_changed(&owner.plugin))
                .await?;
            disabled.push(owner.plugin);
        }
    }
    super::append_audit(repos, audit).await?;
    Ok(disabled)
}
