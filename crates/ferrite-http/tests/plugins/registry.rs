//! Refreshing, inspecting and installing from a signed `file://` registry (M6-2, design
//! 14 §5, §8), on both profiles, with throwaway keys.

use std::sync::Arc;

use bytes::Bytes;
use ferrite_core::{ExtensionService, RegistryStored};
use ferrite_domain::plugin_trust::{KeySource, OwnerId};
use ferrite_http::extensions::{
    ExtensionError, Installer, RegistryTarget, TrustSettings, update_for,
};
use ferrite_http::registry_fetch::FileRegistry;
use ferrite_plugin_host::HostConfig;
use serde_json::json;

use crate::common::{Site, plugin_manifest, plugin_wasm};
use crate::profile_tests;
use crate::registry_fixture::FileRegistryFixture;

profile_tests!(
    refresh_stores_and_lists,
    refresh_tampered_index_keeps_cache,
    refresh_rollback_refused,
    refresh_applies_compromised_revocation,
    inspect_from_registry_reports_verified_publisher,
    registry_install_claims_unsigned_owner,
    component_hash_mismatch_refused,
    yanked_version_not_default_but_installable_by_version,
    other_world_skipped,
    update_available_only_for_same_publisher,
);

fn installer(site: &Site, fx: &FileRegistryFixture) -> Installer {
    Installer::new(
        site.store.clone(),
        site.media.clone(),
        HostConfig::default(),
    )
    .unwrap()
    .with_trust(TrustSettings {
        registry: Some(RegistryTarget {
            url: url::Url::parse(&fx.url()).unwrap(),
            key: fx.registry_key.key.clone(),
        }),
        source: Some(Arc::new(FileRegistry::new(fx.path().to_path_buf()))),
        ..Default::default()
    })
}

/// A registry holding `hello` 0.1.0 and 0.2.0.
fn fixture() -> FileRegistryFixture {
    let mut fx = FileRegistryFixture::new();
    let (m, w) = (plugin_manifest("hello"), plugin_wasm("hello"));
    fx.add_version("hello", "0.1.0", &m, &w, None);
    fx.add_version("hello", "0.2.0", &m, &w, None);
    fx.publish();
    fx
}

async fn install(installer: &Installer, site: &Site, version: Option<&str>) {
    let inspected = installer.inspect_registry("hello", version).await.unwrap();
    let fp = inspected.report().fingerprint.clone();
    let staged = installer.stage_plugin(inspected, &fp).await.unwrap();
    ExtensionService::new(site.store.clone())
        .install_plugin_trusted(&staged.artifact, &staged.row, &staged.trust)
        .await
        .unwrap();
}

async fn refresh_stores_and_lists(site: Site) -> Site {
    let fx = fixture();
    let i = installer(&site, &fx);
    // Before the first refresh: the registry with nothing cached.
    let view = i.registry_view(None).await.unwrap();
    assert_eq!(view.registry.unwrap().serial, None);
    assert!(view.plugins.is_empty());
    let r = i.refresh_registry().await.unwrap();
    assert_eq!(r.stored, RegistryStored::Stored);
    let view = i.registry_view(Some("HEL")).await.unwrap();
    let meta = view.registry.unwrap();
    assert_eq!(
        (meta.serial, meta.name.as_deref()),
        (Some(1), Some("Test registry"))
    );
    assert_eq!(view.plugins.len(), 1);
    assert_eq!(view.plugins[0].latest.as_deref(), Some("0.2.0"));
    assert_eq!(view.plugins[0].publisher_name, "FerriteCMS");
    assert!(view.plugins[0].installed.is_none());
    assert_eq!(
        i.refresh_registry().await.unwrap().stored,
        RegistryStored::Unchanged
    );
    site
}

async fn refresh_tampered_index_keeps_cache(site: Site) -> Site {
    let mut fx = fixture();
    let i = installer(&site, &fx);
    i.refresh_registry().await.unwrap();
    // A newer index whose bytes do not match its signature.
    fx.serial = 2;
    fx.publish();
    let path = fx.path().join("index.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("The hello plugin", "The evil plugin")).unwrap();
    let err = i.refresh_registry().await.unwrap_err();
    assert!(matches!(err, ExtensionError::RegistryInvalid(_)), "{err:?}");
    let row = site
        .store
        .read()
        .plugin_trust()
        .get_registry(&fx.url())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.serial, 1);
    assert!(row.last_error.is_some());
    // Signed by another key: refused too.
    fx.registry_key = ferrite_test_key();
    fx.publish();
    assert!(matches!(
        i.refresh_registry().await,
        Err(ExtensionError::RegistryInvalid(_))
    ));
    site
}

fn ferrite_test_key() -> crate::registry_fixture::TestKey {
    crate::registry_fixture::TestKey::new()
}

async fn refresh_rollback_refused(site: Site) -> Site {
    let mut fx = fixture();
    fx.serial = 5;
    fx.publish();
    let i = installer(&site, &fx);
    i.refresh_registry().await.unwrap();
    fx.serial = 4;
    fx.publish();
    let err = i.refresh_registry().await.unwrap_err();
    assert!(
        matches!(&err, ExtensionError::RegistryInvalid(m) if m.contains("rollback")),
        "{err:?}"
    );
    let row = site
        .store
        .read()
        .plugin_trust()
        .get_registry(&fx.url())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.serial, 5);
    site
}

async fn refresh_applies_compromised_revocation(site: Site) -> Site {
    let mut fx = fixture();
    let i = installer(&site, &fx);
    i.refresh_registry().await.unwrap();
    install(&i, &site, None).await;
    let svc = ExtensionService::new(site.store.clone());
    svc.set_plugin_enabled("hello", true).await.unwrap();
    // The registry revokes the publisher key as compromised (and lists a new one).
    let old = fx.publisher_key.key.clone();
    fx.publisher_key = crate::registry_fixture::TestKey::new();
    fx.revoked.push((old.clone(), true));
    fx.serial = 2;
    fx.publish();
    let r = i.refresh_registry().await.unwrap();
    assert_eq!(r.disabled, ["hello"]);
    let row = site
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    assert!(!row.enabled);
    let k = svc.get_key(&old.key_id).await.unwrap().unwrap();
    assert_eq!(k.source, KeySource::Registry);
    site
}

async fn inspect_from_registry_reports_verified_publisher(site: Site) -> Site {
    let fx = fixture();
    let i = installer(&site, &fx);
    i.refresh_registry().await.unwrap();
    let inspected = i.inspect_registry("hello", None).await.unwrap();
    assert_eq!(inspected.manifest().plugin.version.to_string(), "0.2.0");
    let d = inspected.decision().as_ref().unwrap();
    let signer = d.signer.as_ref().unwrap();
    assert!(matches!(
        &signer.source,
        ferrite_core::SignerSource::Registry { publisher_id, .. } if publisher_id == "ferritecms.dev"
    ));
    assert_eq!(
        d.next_owner,
        Some(OwnerId::Registry {
            registry_key: fx.registry_key.key.key_id,
            publisher: "ferritecms.dev".into()
        })
    );
    assert_eq!(
        inspected.trust().source,
        ferrite_domain::plugin_trust::InstallSource::Registry
    );
    assert!(matches!(
        i.inspect_registry("nope", None).await,
        Err(ExtensionError::Core(ferrite_core::CoreError::NotFound))
    ));
    site
}

async fn registry_install_claims_unsigned_owner(site: Site) -> Site {
    let fx = fixture();
    let i = installer(&site, &fx);
    // An unsigned v0.1.0 first.
    let inspected = i
        .inspect_plugin(&plugin_manifest("hello"), Bytes::from(plugin_wasm("hello")))
        .await
        .unwrap();
    let fp = inspected.report().fingerprint.clone();
    let staged = i.stage_plugin(inspected, &fp).await.unwrap();
    let svc = ExtensionService::new(site.store.clone());
    svc.install_plugin_trusted(&staged.artifact, &staged.row, &staged.trust)
        .await
        .unwrap();
    i.refresh_registry().await.unwrap();
    let inspected = i.inspect_registry("hello", None).await.unwrap();
    assert_eq!(
        inspected.decision().as_ref().unwrap().owner_change,
        ferrite_core::OwnerChange::Claim
    );
    let fp = inspected.report().fingerprint.clone();
    let staged = i.stage_plugin(inspected, &fp).await.unwrap();
    svc.install_plugin_trusted(&staged.artifact, &staged.row, &staged.trust)
        .await
        .unwrap();
    let o = svc.owner("hello").await.unwrap().unwrap();
    assert!(matches!(o.owner, Some(OwnerId::Registry { .. })));
    assert_eq!(o.registry_url.as_deref(), Some(fx.url().as_str()));
    site
}

async fn component_hash_mismatch_refused(site: Site) -> Site {
    let fx = fixture();
    let i = installer(&site, &fx);
    i.refresh_registry().await.unwrap();
    // The file on the registry is swapped after the index was signed.
    let wasm = fx.path().join("hello/0.2.0/hello.wasm");
    let mut bytes = std::fs::read(&wasm).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&wasm, bytes).unwrap();
    let err = i.inspect_registry("hello", None).await.unwrap_err();
    assert!(
        matches!(&err, ExtensionError::RegistryInvalid(m) if m.contains("sha256")),
        "{err:?}"
    );
    site
}

async fn yanked_version_not_default_but_installable_by_version(site: Site) -> Site {
    let mut fx = fixture();
    fx.edit("hello", "0.2.0", |v| v["yanked"] = json!("broken"));
    fx.publish();
    let i = installer(&site, &fx);
    i.refresh_registry().await.unwrap();
    let inspected = i.inspect_registry("hello", None).await.unwrap();
    assert_eq!(inspected.manifest().plugin.version.to_string(), "0.1.0");
    let inspected = i.inspect_registry("hello", Some("0.2.0")).await.unwrap();
    assert_eq!(inspected.manifest().plugin.version.to_string(), "0.2.0");
    site
}

async fn other_world_skipped(site: Site) -> Site {
    let mut fx = fixture();
    fx.edit("hello", "0.2.0", |v| {
        v["world"] = json!("ferrite:plugin/plugin@9.0.0")
    });
    fx.publish();
    let i = installer(&site, &fx);
    i.refresh_registry().await.unwrap();
    let view = i.registry_view(None).await.unwrap();
    assert_eq!(view.plugins[0].latest.as_deref(), Some("0.1.0"));
    assert!(matches!(
        i.inspect_registry("hello", Some("0.2.0")).await,
        Err(ExtensionError::RegistryInvalid(_))
    ));
    site
}

async fn update_available_only_for_same_publisher(site: Site) -> Site {
    let fx = fixture();
    let i = installer(&site, &fx);
    i.refresh_registry().await.unwrap();
    install(&i, &site, Some("0.1.0")).await;
    let view = i.registry_view(None).await.unwrap();
    let installed = view.plugins[0].installed.clone().unwrap();
    assert!(installed.update_available && installed.publisher_matches);
    let ctx = i.registry_context().await.unwrap().unwrap();
    let row = site
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    let svc = ExtensionService::new(site.store.clone());
    let owner = svc.owner("hello").await.unwrap();
    assert_eq!(
        update_for(&ctx.index, &ctx.registry_key, &row, owner.as_ref()),
        Some(("0.2.0".into(), true))
    );
    // Owned by another identity: no update offered.
    let mut other = owner.unwrap();
    other.owner = Some(OwnerId::Key(
        crate::registry_fixture::TestKey::new().key.key_id,
    ));
    assert_eq!(
        update_for(&ctx.index, &ctx.registry_key, &row, Some(&other)),
        None
    );
    // Unsigned-owned: offered, publisher not matching.
    other.owner = None;
    assert_eq!(
        update_for(&ctx.index, &ctx.registry_key, &row, Some(&other)),
        Some(("0.2.0".into(), false))
    );
    site
}
