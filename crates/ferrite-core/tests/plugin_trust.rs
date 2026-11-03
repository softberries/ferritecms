//! Plugin trust decisions at install (M6-2, design 14 §3–§4), on both backends. The
//! signatures are verified before core sees them, so these tests use syntactically valid
//! keys without private halves (`ferrite_store_tests::plugin_trust::key`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, CoreError, ExtensionAdmin, ExtensionService, InstallTrust, OwnerChange, RegistryContext,
    RegistryStored, Scopes, SignerInput, SignerSource, TrustWarning, auth::SiteKey,
};
use ferrite_domain::plugin_registry::{RegistryIndex, RegistryPublisher, RevokedKey};
use ferrite_domain::plugin_trust::{
    InstallSource, KeyId, KeySource, OwnerId, PublisherKey, RegistryCacheRow, Revocation,
    SignaturePolicy,
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, AuditQuery, InstalledPlugin, NewUser, Role, Timestamp,
    UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::plugin_trust::key;
use ferrite_store_tests::{TestDb, backend_tests, backend_tests_spawning};
use serde_json::json;

const REG_URL: &str = "file:///registry/index.json";

fn artifact(seed: u8) -> Artifact {
    Artifact {
        hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind: ArtifactKind::Plugin,
        bytes: 42,
        mime: "application/wasm".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    }
}

fn row(name: &str, a: &Artifact, publisher: &str) -> InstalledPlugin {
    let now = Timestamp::now();
    InstalledPlugin {
        name: name.into(),
        version: "0.1.0".into(),
        artifact_hash: a.hash.clone(),
        manifest: json!({"plugin": {"name": name, "publisher": publisher}}),
        granted: json!({"v": 1}),
        enabled: true,
        disabled_reason: None,
        installed_at: now,
        updated_at: now,
    }
}

fn signed(n: u64, claim: &str) -> InstallTrust {
    InstallTrust {
        signer: Some(SignerInput {
            key: key(n),
            signature: format!("untrusted comment: by {n}\n"),
        }),
        ..InstallTrust::unsigned(claim)
    }
}

fn with_registry(mut t: InstallTrust, index: &RegistryIndex) -> InstallTrust {
    t.registry = Some(registry_ctx(index));
    t
}

fn registry_key() -> KeyId {
    key(0xAAAA).key_id
}

fn registry_ctx(index: &RegistryIndex) -> RegistryContext {
    RegistryContext {
        url: REG_URL.into(),
        registry_key: registry_key(),
        index: Arc::new(index.clone()),
    }
}

/// An index whose publisher `ferritecms.dev` holds `keys` and has revoked `revoked`.
fn index(serial: u64, keys: &[u64], revoked: &[(u64, bool)]) -> RegistryIndex {
    RegistryIndex {
        name: "test registry".into(),
        serial,
        generated_at: Timestamp::now(),
        publishers: vec![RegistryPublisher {
            id: "ferritecms.dev".into(),
            name: "FerriteCMS".into(),
            homepage: None,
            keys: keys.iter().map(|n| key(*n)).collect(),
            revoked_keys: revoked
                .iter()
                .map(|(n, compromised)| RevokedKey {
                    key: key(*n),
                    revoked_at: Timestamp::now(),
                    reason: "stolen".into(),
                    compromised: *compromised,
                })
                .collect(),
        }],
        plugins: vec![],
    }
}

fn cache_row(serial: i64, json: &str) -> RegistryCacheRow {
    let now = Timestamp::now();
    RegistryCacheRow {
        url: REG_URL.into(),
        serial,
        index_json: json.into(),
        signature: "sig".into(),
        generated_at: now,
        fetched_at: now,
        checked_at: now,
        last_error: None,
        last_error_at: None,
    }
}

fn field(e: CoreError) -> &'static str {
    match e {
        CoreError::InvalidField { field, .. } => field,
        other => panic!("not a field error: {other:?}"),
    }
}

async fn install(
    svc: &ExtensionService,
    name: &str,
    seed: u8,
    trust: &InstallTrust,
) -> Result<ferrite_core::TrustDecision, CoreError> {
    let a = artifact(seed);
    svc.install_plugin_trusted(&a, &row(name, &a, &trust.publisher_claim), trust)
        .await
}

async fn owner_of(store: &dyn Store, name: &str) -> Option<OwnerId> {
    store
        .read()
        .plugin_trust()
        .get_owner(name)
        .await
        .unwrap()
        .and_then(|o| o.owner)
}

// One test per row of design 14 §4.2's table.

pub async fn none_signed_binds(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let d = install(&svc, "seo", 1, &signed(1, "example.dev"))
        .await
        .unwrap();
    assert_eq!(d.owner_change, OwnerChange::Bind);
    assert_eq!(d.signer.unwrap().source, SignerSource::New);
    assert!(d.warnings.contains(&TrustWarning::NewKey));
    assert_eq!(
        owner_of(db.store(), "seo").await,
        Some(OwnerId::Key(key(1).key_id))
    );
    // The approved key is recorded.
    let k = db
        .store()
        .read()
        .plugin_trust()
        .get_key(&key(1).key_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (k.source, k.label.as_str()),
        (KeySource::Approved, "example.dev")
    );
}

pub async fn none_unsigned_binds_unsigned(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let d = install(&svc, "seo", 1, &InstallTrust::unsigned("x"))
        .await
        .unwrap();
    assert_eq!(d.owner_change, OwnerChange::BindUnsigned);
    assert_eq!(d.warnings, [TrustWarning::Unsigned]);
    let o = db
        .store()
        .read()
        .plugin_trust()
        .get_owner("seo")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((o.owner, o.signer_key), (None, None));
}

pub async fn unsigned_over_unsigned_is_unsigned_update(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &InstallTrust::unsigned("x"))
        .await
        .unwrap();
    let d = install(&svc, "seo", 2, &InstallTrust::unsigned("x"))
        .await
        .unwrap();
    assert_eq!(d.owner_change, OwnerChange::UnsignedUpdate);
    assert!(d.warnings.contains(&TrustWarning::UnsignedReadsData));
}

pub async fn signed_over_unsigned_claims(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &InstallTrust::unsigned("x"))
        .await
        .unwrap();
    let d = install(&svc, "seo", 2, &signed(1, "x")).await.unwrap();
    assert_eq!(d.owner_change, OwnerChange::Claim);
    assert!(d.warnings.contains(&TrustWarning::UnsignedDataClaimed));
    assert_eq!(
        owner_of(db.store(), "seo").await,
        Some(OwnerId::Key(key(1).key_id))
    );
}

pub async fn same_signer_is_same(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    let d = install(&svc, "seo", 2, &signed(1, "x")).await.unwrap();
    assert_eq!(d.owner_change, OwnerChange::Same);
    assert_eq!(d.signer.unwrap().source, SignerSource::Approved);
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

pub async fn stripped_signature_after_signed_owner_refused(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    store
        .read()
        .plugin_kv()
        .set("seo", "state", b"secret")
        .await
        .unwrap();
    let err = install(&svc, "seo", 2, &InstallTrust::unsigned("x"))
        .await
        .unwrap_err();
    assert_eq!(field(err), "publisher");
    // The installed version keeps running, its data untouched.
    let row = store.read().plugins().get("seo").await.unwrap().unwrap();
    assert_eq!(row.artifact_hash, artifact(1).hash);
    assert_eq!(
        store.read().plugin_kv().get("seo", "state").await.unwrap(),
        Some(b"secret".to_vec())
    );
}

pub async fn other_key_same_name_refused_even_if_approved_new(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    let err = install(&svc, "seo", 2, &signed(2, "x")).await.unwrap_err();
    assert_eq!(field(err), "publisher");
    // The refused install wrote nothing: key 2 is not trusted.
    assert!(
        store
            .read()
            .plugin_trust()
            .get_key(&key(2).key_id)
            .await
            .unwrap()
            .is_none()
    );
}

pub async fn registry_identity_survives_key_rotation(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let one = index(1, &[1], &[]);
    let d = install(
        &svc,
        "seo",
        1,
        &with_registry(signed(1, "ferritecms.dev"), &one),
    )
    .await
    .unwrap();
    assert_eq!(d.owner_change, OwnerChange::Bind);
    let reg = OwnerId::Registry {
        registry_key: registry_key(),
        publisher: "ferritecms.dev".into(),
    };
    assert_eq!(d.next_owner, Some(reg.clone()));
    assert!(matches!(
        d.signer.unwrap().source,
        SignerSource::Registry { .. }
    ));
    // The registry lists a second key: an update signed by it keeps the binding.
    let two = index(2, &[1, 2], &[]);
    let d = install(
        &svc,
        "seo",
        2,
        &with_registry(signed(2, "ferritecms.dev"), &two),
    )
    .await
    .unwrap();
    assert_eq!(d.owner_change, OwnerChange::Same);
    assert_eq!(owner_of(db.store(), "seo").await, Some(reg));
    // Without the registry, key 2 is just a new key: refused.
    let err = install(&svc, "seo", 3, &signed(2, "ferritecms.dev"))
        .await
        .unwrap_err();
    assert_eq!(field(err), "publisher");
}

pub async fn registry_publisher_text_must_match(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let err = install(
        &svc,
        "seo",
        1,
        &with_registry(signed(1, "evil.dev"), &index(1, &[1], &[])),
    )
    .await
    .unwrap_err();
    assert_eq!(field(err), "publisher");
}

pub async fn policy_signed_refuses_unsigned(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let mut t = InstallTrust::unsigned("x");
    t.policy = SignaturePolicy::Signed;
    assert_eq!(
        field(install(&svc, "seo", 1, &t).await.unwrap_err()),
        "signature"
    );
    let mut t = signed(1, "x");
    t.policy = SignaturePolicy::Signed;
    install(&svc, "seo", 1, &t).await.unwrap();
}

pub async fn policy_trusted_refuses_new_key(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let mut t = signed(1, "x");
    t.policy = SignaturePolicy::Trusted;
    assert_eq!(
        field(install(&svc, "seo", 1, &t).await.unwrap_err()),
        "signature"
    );
    // An operator key passes.
    svc.trust_key(&key(1), "ops", None).await.unwrap();
    let d = install(&svc, "seo", 1, &t).await.unwrap();
    assert_eq!(d.signer.unwrap().source, SignerSource::Operator);
}

pub async fn revoked_key_refuses_install(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    svc.revoke_key(
        &key(1),
        &Revocation {
            at: Timestamp::now(),
            reason: "retired".into(),
            compromised: false,
        },
        "k",
        KeySource::Approved,
    )
    .await
    .unwrap();
    assert_eq!(
        field(install(&svc, "seo", 1, &signed(1, "x")).await.unwrap_err()),
        "signature"
    );
    // Revoked by the registry: refused too.
    let t = with_registry(signed(3, "ferritecms.dev"), &index(1, &[], &[(3, false)]));
    assert_eq!(
        field(install(&svc, "other", 1, &t).await.unwrap_err()),
        "signature"
    );
    // The operator re-trusts it.
    svc.trust_key(&key(1), "back", None).await.unwrap();
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
}

pub async fn compromised_revocation_disables_and_emits_plugins_changed(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    install(&svc, "other", 2, &signed(2, "x")).await.unwrap();
    svc.set_plugin_enabled("seo", true).await.unwrap();
    svc.set_plugin_enabled("other", true).await.unwrap();
    let seq = store.read().events().max_seq().await.unwrap();
    let disabled = svc
        .revoke_key(
            &key(1),
            &Revocation {
                at: Timestamp::now(),
                reason: "leaked".into(),
                compromised: true,
            },
            "k",
            KeySource::Approved,
        )
        .await
        .unwrap();
    assert_eq!(disabled, ["seo"]);
    let row = store.read().plugins().get("seo").await.unwrap().unwrap();
    assert!(!row.enabled);
    assert_eq!(
        row.disabled_reason.as_deref(),
        Some("publisher key revoked: leaked")
    );
    assert!(
        store
            .read()
            .plugins()
            .get("other")
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
    let events = store.read().events().since(seq, 10).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == "plugins.changed" && e.keys == ["seo"])
    );
    // An admin can enable it again knowingly.
    svc.set_plugin_enabled("seo", true).await.unwrap();
}

pub async fn retired_revocation_keeps_running(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    svc.set_plugin_enabled("seo", true).await.unwrap();
    let disabled = svc
        .revoke_key(
            &key(1),
            &Revocation {
                at: Timestamp::now(),
                reason: "rotated".into(),
                compromised: false,
            },
            "k",
            KeySource::Approved,
        )
        .await
        .unwrap();
    assert!(disabled.is_empty());
    assert!(
        store
            .read()
            .plugins()
            .get("seo")
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
    assert_eq!(
        field(install(&svc, "seo", 2, &signed(1, "x")).await.unwrap_err()),
        "signature"
    );
}

pub async fn erase_frees_name(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    let deleted = svc.delete_plugin("seo").await.unwrap();
    // Retained data stays bound.
    assert!(owner_of(store, "seo").await.is_some());
    let deleted_at = Timestamp::from_micros(
        store
            .read()
            .options()
            .get("plugin.seo.deleted_at")
            .await
            .unwrap()
            .unwrap()
            .value
            .as_i64()
            .unwrap(),
    )
    .unwrap();
    assert!(svc.purge_plugin_data("seo", deleted_at, &[]).await.unwrap());
    drop(deleted);
    assert!(
        store
            .read()
            .plugin_trust()
            .get_owner("seo")
            .await
            .unwrap()
            .is_none()
    );
    // Any publisher may now bind the name.
    let d = install(&svc, "seo", 2, &signed(2, "y")).await.unwrap();
    assert_eq!(d.owner_change, OwnerChange::Bind);
}

pub async fn purge_true_delete_keeps_binding_until_final_sweep(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    svc.remove_plugin("seo", true).await.unwrap();
    // The marker is kept for the final sweep: still bound, with the "being erased" text.
    assert!(owner_of(store, "seo").await.is_some());
    let err = install(&svc, "seo", 2, &signed(2, "y")).await.unwrap_err();
    match err {
        CoreError::InvalidField { field, message } => {
            assert_eq!(field, "publisher");
            assert!(message.contains("being erased"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    // The delete's job (the final sweep) frees it.
    let marker = store
        .read()
        .options()
        .get("plugin.seo.deleted_at")
        .await
        .unwrap()
        .unwrap();
    let deleted_at = Timestamp::from_micros(marker.value.as_i64().unwrap()).unwrap();
    assert!(svc.purge_plugin_data("seo", deleted_at, &[]).await.unwrap());
    assert!(owner_of(store, "seo").await.is_none());
    install(&svc, "seo", 2, &signed(2, "y")).await.unwrap();
}

pub async fn retained_data_bound_refuses_other_publisher(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    svc.delete_plugin("seo").await.unwrap();
    assert_eq!(
        field(install(&svc, "seo", 2, &signed(2, "y")).await.unwrap_err()),
        "publisher"
    );
    assert_eq!(
        field(
            install(&svc, "seo", 2, &InstallTrust::unsigned("y"))
                .await
                .unwrap_err()
        ),
        "publisher"
    );
    // The same publisher reinstalls and keeps its data.
    let d = install(&svc, "seo", 3, &signed(1, "x")).await.unwrap();
    assert_eq!(d.owner_change, OwnerChange::Same);
}

pub async fn unbind_keeps_data(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    store
        .read()
        .plugin_kv()
        .set("seo", "k", b"v")
        .await
        .unwrap();
    assert!(svc.unbind("seo").await.unwrap());
    assert!(!svc.unbind("seo").await.unwrap());
    assert_eq!(
        store.read().plugin_kv().get("seo", "k").await.unwrap(),
        Some(b"v".to_vec())
    );
    // The data is unsigned-owned now: another key claims it.
    let d = install(&svc, "seo", 2, &signed(2, "y")).await.unwrap();
    assert_eq!(d.owner_change, OwnerChange::Claim);
}

pub async fn serial_rollback_refused(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let i = index(5, &[1], &[]);
    let r = svc
        .store_registry(&cache_row(5, "five"), &i, None)
        .await
        .unwrap();
    assert_eq!(r.stored, RegistryStored::Stored);
    let r = svc
        .store_registry(&cache_row(5, "five"), &i, None)
        .await
        .unwrap();
    assert_eq!(r.stored, RegistryStored::Unchanged);
    assert_eq!(
        field(
            svc.store_registry(&cache_row(4, "four"), &i, None)
                .await
                .unwrap_err()
        ),
        "serial"
    );
    assert_eq!(
        svc.registry_cache(REG_URL).await.unwrap().unwrap().serial,
        5
    );
    let r = svc
        .store_registry(&cache_row(6, "six"), &i, None)
        .await
        .unwrap();
    assert_eq!(r.stored, RegistryStored::Stored);
}

pub async fn same_serial_other_bytes_refused(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let i = index(5, &[1], &[]);
    svc.store_registry(&cache_row(5, "five"), &i, None)
        .await
        .unwrap();
    assert_eq!(
        field(
            svc.store_registry(&cache_row(5, "FIVE"), &i, None)
                .await
                .unwrap_err()
        ),
        "serial"
    );
    assert_eq!(
        svc.registry_cache(REG_URL)
            .await
            .unwrap()
            .unwrap()
            .index_json,
        "five"
    );
}

pub async fn registry_revocations_applied_once(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    let one = index(1, &[1], &[]);
    install(
        &svc,
        "seo",
        1,
        &with_registry(signed(1, "ferritecms.dev"), &one),
    )
    .await
    .unwrap();
    svc.set_plugin_enabled("seo", true).await.unwrap();
    let two = index(2, &[], &[(1, true)]);
    let r = svc
        .store_registry(&cache_row(2, "two"), &two, None)
        .await
        .unwrap();
    assert_eq!(r.disabled, ["seo"]);
    let k = svc.get_key(&key(1).key_id).await.unwrap().unwrap();
    assert_eq!(k.source, KeySource::Registry);
    assert!(k.revoked.unwrap().compromised);
    // Seen again (enabled again by an admin meanwhile): not disabled a second time.
    svc.set_plugin_enabled("seo", true).await.unwrap();
    let r = svc
        .store_registry(&cache_row(3, "three"), &index(3, &[], &[(1, true)]), None)
        .await
        .unwrap();
    assert!(r.disabled.is_empty());
    assert!(
        store
            .read()
            .plugins()
            .get("seo")
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
}

pub async fn concurrent_installs_of_two_publishers_one_wins(db: std::sync::Arc<TestDb>) {
    let svc = Arc::new(ExtensionService::new(db.shared_store()));
    let a = {
        let svc = svc.clone();
        tokio::spawn(async move { install(&svc, "seo", 1, &signed(1, "x")).await })
    };
    let b = {
        let svc = svc.clone();
        tokio::spawn(async move { install(&svc, "seo", 2, &signed(2, "y")).await })
    };
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    assert!(a.is_ok() != b.is_ok(), "{a:?} {b:?}");
    let loser = a.err().or(b.err()).unwrap();
    assert_eq!(field(loser), "publisher");
}

async fn admin_actor(store: &dyn Store) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Actor::new(id, Role::Admin)
}

pub async fn audit_install_names_signer(db: &TestDb) {
    let store = db.store();
    let admin = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
    let actor = admin_actor(store).await;
    let a = artifact(1);
    admin
        .install_plugin_trusted(&actor, &a, &row("seo", &a, "x"), &signed(1, "x"))
        .await
        .unwrap();
    let page = store
        .read()
        .audit()
        .list(&AuditQuery {
            action: Some("plugin.install".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    let d = &page.items[0].details;
    assert_eq!(d["signer"], key(1).key_id.to_string());
    assert_eq!(d["owner"], format!("key:{}", key(1).key_id));
    assert_eq!(d["source"], "upload");
    // Revocation is audited too, and needs `extensions:write`.
    let readonly = actor.with_scopes(Scopes::from_strs(&["extensions:read".to_owned()]).unwrap());
    let rev = Revocation {
        at: Timestamp::now(),
        reason: "r".into(),
        compromised: false,
    };
    assert!(matches!(
        admin
            .revoke_key(&readonly, &key(1), &rev, "k", KeySource::Approved)
            .await,
        Err(CoreError::Forbidden(_))
    ));
    admin
        .revoke_key(&actor, &key(1), &rev, "k", KeySource::Approved)
        .await
        .unwrap();
    let page = store
        .read()
        .audit()
        .list(&AuditQuery {
            action: Some("plugin.publisher.revoke".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
}

pub async fn existing_rows_without_owner_are_unsigned_owned(db: &TestDb) {
    let store = db.store();
    // A row installed before M6-2: no owner row.
    let a = artifact(1);
    store.read().artifacts().insert_if_absent(&a).await.unwrap();
    store
        .read()
        .plugins()
        .upsert(&row("seo", &a, "x"))
        .await
        .unwrap();
    let svc = ExtensionService::new(db.shared_store());
    let d = svc
        .trust_preview("seo", &InstallTrust::unsigned("x"))
        .await
        .unwrap();
    assert_eq!(d.owner_change, OwnerChange::UnsignedUpdate);
    let d = install(&svc, "seo", 2, &signed(1, "x")).await.unwrap();
    assert_eq!(d.owner_change, OwnerChange::Claim);
    assert_eq!(d.current_owner, None);
    let o = store
        .read()
        .plugin_trust()
        .get_owner("seo")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(o.source, InstallSource::Upload);
}

// ---- M6-2 review fixes ------------------------------------------------------------------

/// A key with key `n`'s id but other key bytes (anyone can pick a key id).
fn forged(n: u64) -> PublisherKey {
    use base64::Engine;
    let real = key(n);
    let mut bin = base64::engine::general_purpose::STANDARD
        .decode(&real.base64)
        .unwrap();
    bin[20] ^= 0xff;
    let k =
        PublisherKey::from_base64(&base64::engine::general_purpose::STANDARD.encode(bin)).unwrap();
    assert_eq!(k.key_id, real.key_id);
    k
}

fn signed_by(k: &PublisherKey, claim: &str) -> InstallTrust {
    InstallTrust {
        signer: Some(SignerInput {
            key: k.clone(),
            signature: "untrusted comment: forged\n".into(),
        }),
        ..InstallTrust::unsigned(claim)
    }
}

/// CRITICAL: a TOFU-approved key with the registry key's id but other bytes never gets
/// the registry identity, and with the index cached it is refused outright.
pub async fn forged_key_with_registry_key_id_is_not_the_registry(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    // Before the index is fetched: an admin approves the forged key for `foo`.
    let d = install(&svc, "foo", 1, &signed_by(&forged(1), "ferritecms.dev"))
        .await
        .unwrap();
    assert_eq!(d.next_owner, Some(OwnerId::Key(key(1).key_id)));
    // With the index cached, the forged key is refused for any name, whatever the claim.
    let idx = index(1, &[1], &[]);
    for (name, claim) in [("bar", "ferritecms.dev"), ("foo", "ferritecms.dev")] {
        let err = install(
            &svc,
            name,
            2,
            &with_registry(signed_by(&forged(1), claim), &idx),
        )
        .await
        .unwrap_err();
        assert_eq!(field(err), "signature", "{name}");
    }
    // A name bound to the registry publisher by the genuine key cannot be updated by it.
    install(
        &svc,
        "seo",
        3,
        &with_registry(signed(1, "ferritecms.dev"), &idx),
    )
    .await
    .unwrap_err(); // the site row under that id holds the forged key: refused too
}

/// Refresh removes a colliding site key, disables what it signed and audits it; the
/// genuine registry key works afterwards.
pub async fn refresh_removes_colliding_key_and_disables_its_plugins(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "foo", 1, &signed_by(&forged(1), "ferritecms.dev"))
        .await
        .unwrap();
    svc.set_plugin_enabled("foo", true).await.unwrap();
    let idx = index(1, &[1], &[]);
    let r = svc
        .store_registry(&cache_row(1, "one"), &idx, None)
        .await
        .unwrap();
    assert_eq!(r.collisions, [key(1).key_id]);
    assert_eq!(r.disabled, ["foo"]);
    assert!(svc.get_key(&key(1).key_id).await.unwrap().is_none());
    let row = store.read().plugins().get("foo").await.unwrap().unwrap();
    assert!(!row.enabled);
    assert!(
        row.disabled_reason
            .unwrap()
            .contains("has the id of the registry's key")
    );
    let audit = store
        .read()
        .audit()
        .list(&AuditQuery {
            action: Some("plugin.publisher.collision".into()),
            limit: 5,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(audit.items.len(), 1);
    // The genuine key now installs another plugin with the registry identity.
    let d = install(
        &svc,
        "seo",
        2,
        &with_registry(signed(1, "ferritecms.dev"), &idx),
    )
    .await
    .unwrap();
    assert!(matches!(d.next_owner, Some(OwnerId::Registry { .. })));
}

/// The operator cannot trust a key whose id the cached index lists under other bytes.
pub async fn trust_key_refuses_a_registry_id_collision(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let idx = index(1, &[1], &[]);
    let err = svc
        .trust_key(&forged(1), "ops", Some(&idx))
        .await
        .unwrap_err();
    assert_eq!(field(err), "key");
    svc.trust_key(&key(1), "ops", Some(&idx)).await.unwrap();
}

/// MINOR 1: `trusted` refuses a key an admin approved earlier (only registry and operator
/// keys pass).
pub async fn policy_trusted_refuses_approved_key(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    install(&svc, "seo", 1, &signed(1, "x")).await.unwrap();
    let mut t = signed(1, "x");
    t.policy = SignaturePolicy::Trusted;
    assert_eq!(
        field(install(&svc, "seo", 2, &t).await.unwrap_err()),
        "signature"
    );
}

/// MINOR 3: a cached row that does not verify under the current registry key (re-keyed)
/// gives no serial floor.
pub async fn stale_cache_is_replaced_without_the_serial_rule(db: &TestDb) {
    let svc = ExtensionService::new(db.shared_store());
    let i = index(9, &[1], &[]);
    svc.store_registry(&cache_row(9, "old-key"), &i, None)
        .await
        .unwrap();
    // Without the stale mark the lower serial is a rollback.
    assert_eq!(
        field(
            svc.store_registry(&cache_row(2, "new-key"), &i, None)
                .await
                .unwrap_err()
        ),
        "serial"
    );
    let r = svc
        .store_registry(&cache_row(2, "new-key"), &i, Some("old-key"))
        .await
        .unwrap();
    assert_eq!(r.stored, RegistryStored::Stored);
    assert_eq!(
        svc.registry_cache(REG_URL).await.unwrap().unwrap().serial,
        2
    );
    // A stale mark naming other bytes than the cached row's changes nothing.
    assert_eq!(
        field(
            svc.store_registry(&cache_row(1, "x"), &i, Some("old-key"))
                .await
                .unwrap_err()
        ),
        "serial"
    );
}

backend_tests!(
    none_signed_binds,
    none_unsigned_binds_unsigned,
    unsigned_over_unsigned_is_unsigned_update,
    signed_over_unsigned_claims,
    same_signer_is_same,
    stripped_signature_after_signed_owner_refused,
    other_key_same_name_refused_even_if_approved_new,
    registry_identity_survives_key_rotation,
    registry_publisher_text_must_match,
    policy_signed_refuses_unsigned,
    policy_trusted_refuses_new_key,
    revoked_key_refuses_install,
    compromised_revocation_disables_and_emits_plugins_changed,
    retired_revocation_keeps_running,
    erase_frees_name,
    purge_true_delete_keeps_binding_until_final_sweep,
    retained_data_bound_refuses_other_publisher,
    unbind_keeps_data,
    serial_rollback_refused,
    same_serial_other_bytes_refused,
    registry_revocations_applied_once,
    audit_install_names_signer,
    existing_rows_without_owner_are_unsigned_owned,
    forged_key_with_registry_key_id_is_not_the_registry,
    refresh_removes_colliding_key_and_disables_its_plugins,
    trust_key_refuses_a_registry_id_collision,
    policy_trusted_refuses_approved_key,
    stale_cache_is_replaced_without_the_serial_rule,
);

backend_tests_spawning!(concurrent_installs_of_two_publishers_one_wins);
