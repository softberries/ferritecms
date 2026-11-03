//! Contract tests for `0015_plugin_publishers` (M6-2, design 14 §6): publisher keys,
//! plugin name owners and the registry cache.

use ferrite_domain::Timestamp;
use ferrite_domain::plugin_trust::{
    InstallSource, KeyId, KeySource, OwnerId, PluginOwner, PublisherKey, RegistryCacheRow,
    Revocation, TrustedKey,
};
use ferrite_store::{StoreError, catalogue};

use crate::{
    TestDb,
    export::{dump, spec},
};

/// A syntactically valid minisign public key with key number `n` (no private half).
pub fn key(n: u64) -> PublisherKey {
    use base64::Engine;
    let mut bin = b"Ed".to_vec();
    bin.extend_from_slice(&n.to_le_bytes());
    bin.extend_from_slice(&[n as u8; 32]);
    PublisherKey::from_base64(&base64::engine::general_purpose::STANDARD.encode(bin)).unwrap()
}

pub(crate) fn trusted(n: u64, now: Timestamp) -> TrustedKey {
    TrustedKey {
        key: key(n),
        label: format!("key {n}"),
        source: KeySource::Approved,
        trusted_at: now,
        revoked: None,
    }
}

pub(crate) fn owner(plugin: &str, signer: Option<u64>, now: Timestamp) -> PluginOwner {
    PluginOwner {
        plugin: plugin.into(),
        owner: signer.map(|n| OwnerId::Key(key(n).key_id)),
        signer_key: signer.map(|n| key(n).key_id),
        signer_public_key: signer.map(|n| key(n).base64),
        signature: signer.map(|n| format!("untrusted comment: sig {n}\n")),
        publisher: "example.dev".into(),
        source: InstallSource::Upload,
        registry_url: None,
        bound_at: now,
        updated_at: now,
    }
}

fn registry_row(url: &str, serial: i64, now: Timestamp) -> RegistryCacheRow {
    RegistryCacheRow {
        url: url.into(),
        serial,
        index_json: format!("{{\"serial\":{serial}}}"),
        signature: "sig".into(),
        generated_at: now,
        fetched_at: now,
        checked_at: now,
        last_error: None,
        last_error_at: None,
    }
}

fn later(t: Timestamp, secs: i64) -> Timestamp {
    Timestamp::from_micros(t.as_micros() + secs * 1_000_000).unwrap()
}

pub async fn owner_round_trip_with_null_owner(db: &TestDb) {
    let repo = db.store().read().plugin_trust();
    let now = Timestamp::now();
    assert_eq!(repo.get_owner("seo").await.unwrap(), None);
    let unsigned = owner("seo", None, now);
    repo.put_owner(&unsigned).await.unwrap();
    assert_eq!(repo.get_owner("seo").await.unwrap(), Some(unsigned.clone()));

    // A claim: replaced, `bound_at` kept.
    let mut signed = owner("seo", Some(7), later(now, 5));
    signed.owner = Some(OwnerId::Registry {
        registry_key: key(9).key_id,
        publisher: "ferritecms.dev".into(),
    });
    signed.source = InstallSource::Registry;
    signed.registry_url = Some("file:///r/index.json".into());
    repo.put_owner(&signed).await.unwrap();
    let got = repo.get_owner("seo").await.unwrap().unwrap();
    assert_eq!(got.bound_at, now);
    assert_eq!(
        got,
        PluginOwner {
            bound_at: now,
            ..signed
        }
    );
    repo.put_owner(&owner("analytics", None, now))
        .await
        .unwrap();
    let names: Vec<String> = repo
        .list_owners()
        .await
        .unwrap()
        .into_iter()
        .map(|o| o.plugin)
        .collect();
    assert_eq!(names, ["analytics", "seo"]);
    assert!(repo.delete_owner("seo").await.unwrap());
    assert!(!repo.delete_owner("seo").await.unwrap());
    assert_eq!(repo.get_owner("seo").await.unwrap(), None);
    let mut bad = owner("nul", None, now);
    bad.publisher = "a\0b".into();
    assert!(matches!(
        repo.put_owner(&bad).await,
        Err(StoreError::Invalid(_))
    ));
}

/// No foreign key to `plugins`: an owner row exists with no installed row (retained data).
pub async fn owner_outlives_plugin_row(db: &TestDb) {
    let store = db.store();
    let now = Timestamp::now();
    store
        .read()
        .plugin_trust()
        .put_owner(&owner("gone", Some(1), now))
        .await
        .unwrap();
    assert_eq!(store.read().plugins().get("gone").await.unwrap(), None);
    assert!(
        store
            .read()
            .plugin_trust()
            .get_owner("gone")
            .await
            .unwrap()
            .is_some()
    );
}

pub async fn owners_signed_by_finds_current_signer(db: &TestDb) {
    let repo = db.store().read().plugin_trust();
    let now = Timestamp::now();
    repo.put_owner(&owner("b-plugin", Some(1), now))
        .await
        .unwrap();
    repo.put_owner(&owner("a-plugin", Some(1), now))
        .await
        .unwrap();
    repo.put_owner(&owner("c-plugin", Some(2), now))
        .await
        .unwrap();
    repo.put_owner(&owner("d-plugin", None, now)).await.unwrap();
    let names = |v: Vec<PluginOwner>| v.into_iter().map(|o| o.plugin).collect::<Vec<_>>();
    assert_eq!(
        names(repo.owners_signed_by(&key(1).key_id).await.unwrap()),
        ["a-plugin", "b-plugin"]
    );
    // b moves to key 2: it is now signed by 2.
    repo.put_owner(&owner("b-plugin", Some(2), now))
        .await
        .unwrap();
    assert_eq!(
        names(repo.owners_signed_by(&key(2).key_id).await.unwrap()),
        ["b-plugin", "c-plugin"]
    );
    assert!(
        repo.owners_signed_by(&key(3).key_id)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn key_upsert_and_revoke(db: &TestDb) {
    let repo = db.store().read().plugin_trust();
    let now = Timestamp::now();
    let k = trusted(1, now);
    assert_eq!(repo.get_key(&k.key.key_id).await.unwrap(), None);
    repo.upsert_key(&k).await.unwrap();
    assert_eq!(repo.get_key(&k.key.key_id).await.unwrap(), Some(k.clone()));
    let rev = Revocation {
        at: later(now, 1),
        reason: "stolen laptop".into(),
        compromised: true,
    };
    assert!(repo.revoke_key(&k.key.key_id, &rev).await.unwrap());
    assert!(!repo.revoke_key(&key(2).key_id, &rev).await.unwrap());
    let got = repo.get_key(&k.key.key_id).await.unwrap().unwrap();
    assert_eq!(got.revoked, Some(rev.clone()));
    // An operator re-trust clears the revocation.
    let cleared = TrustedKey {
        source: KeySource::Operator,
        label: "re-trusted".into(),
        ..k.clone()
    };
    repo.upsert_key(&cleared).await.unwrap();
    assert_eq!(repo.get_key(&k.key.key_id).await.unwrap(), Some(cleared));
    // A registry revocation row.
    let reg = TrustedKey {
        source: KeySource::Registry,
        revoked: Some(Revocation {
            compromised: false,
            ..rev
        }),
        ..trusted(0x10, now)
    };
    repo.upsert_key(&reg).await.unwrap();
    let ids: Vec<KeyId> = repo
        .list_keys()
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.key.key_id)
        .collect();
    let mut want = vec![key(1).key_id, key(0x10).key_id];
    want.sort_by_key(|k| k.to_string());
    assert_eq!(ids, want);
}

pub async fn public_key_unique(db: &TestDb) {
    let repo = db.store().read().plugin_trust();
    let now = Timestamp::now();
    repo.upsert_key(&trusted(1, now)).await.unwrap();
    // The same public key text under another id: only reachable by writing the row
    // directly, which is what the unique index guards.
    let mut other = trusted(1, now);
    other.key.key_id = key(2).key_id;
    let err = repo.upsert_key(&other).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::UniqueViolation { constraint } if constraint == "plugin_publishers_public_key_uq"),
        "{err:?}"
    );
}

pub async fn registry_row_replaced_by_url(db: &TestDb) {
    let repo = db.store().read().plugin_trust();
    let now = Timestamp::now();
    let url = "https://example.dev/registry/v1/index.json";
    assert_eq!(repo.get_registry(url).await.unwrap(), None);
    repo.put_registry(&registry_row(url, 1, now)).await.unwrap();
    let two = registry_row(url, 2, later(now, 60));
    repo.put_registry(&two).await.unwrap();
    assert_eq!(repo.get_registry(url).await.unwrap(), Some(two));
    repo.put_registry(&registry_row("file:///other.json", 9, now))
        .await
        .unwrap();
    assert_eq!(repo.get_registry(url).await.unwrap().unwrap().serial, 2);
}

pub async fn registry_checked_keeps_index(db: &TestDb) {
    let repo = db.store().read().plugin_trust();
    let now = Timestamp::now();
    let url = "file:///r/index.json";
    assert!(!repo.registry_checked(url, now, None).await.unwrap());
    let row = registry_row(url, 3, now);
    repo.put_registry(&row).await.unwrap();
    let t1 = later(now, 10);
    assert!(
        repo.registry_checked(url, t1, Some("timed out"))
            .await
            .unwrap()
    );
    let got = repo.get_registry(url).await.unwrap().unwrap();
    assert_eq!(got.last_error.as_deref(), Some("timed out"));
    assert_eq!(got.last_error_at, Some(t1));
    assert_eq!(got.checked_at, now);
    assert_eq!(got.index_json, row.index_json);
    let t2 = later(now, 20);
    assert!(repo.registry_checked(url, t2, None).await.unwrap());
    let got = repo.get_registry(url).await.unwrap().unwrap();
    assert_eq!((got.last_error, got.last_error_at), (None, None));
    assert_eq!(got.checked_at, t2);
    assert_eq!(got.serial, 3);
}

/// The export takes the key and owner rows and skips the registry cache.
pub async fn export_catalogue_lists_owner_and_key_tables(db: &TestDb) {
    let store = db.store();
    let now = Timestamp::now();
    let repo = store.read().plugin_trust();
    repo.upsert_key(&trusted(1, now)).await.unwrap();
    repo.put_owner(&owner("seo", Some(1), now)).await.unwrap();
    repo.put_owner(&owner("plain", None, now)).await.unwrap();
    repo.put_registry(&registry_row("file:///r.json", 1, now))
        .await
        .unwrap();
    assert!(spec("plugin_publishers").is_exported());
    assert!(spec("plugin_owners").is_exported());
    // M6-2 review: the registry cache moves too (its serial is the copy's rollback floor).
    assert!(catalogue::table("plugin_registry").unwrap().is_exported());
    let mut reader = store.export_reader().await.unwrap();
    let keys = dump(reader.as_mut(), spec("plugin_publishers"), 10).await;
    let owners = dump(reader.as_mut(), spec("plugin_owners"), 1).await;
    let registries = dump(reader.as_mut(), spec("plugin_registry"), 10).await;
    assert_eq!(registries.len(), 1);
    assert_eq!(registries[0]["serial"], 1);
    reader.finish().await.unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["key_id"], key(1).key_id.to_string());
    assert_eq!(keys[0]["compromised"], false);
    let names: Vec<_> = owners.iter().map(|r| r["plugin"].clone()).collect();
    assert_eq!(names, ["plain", "seo"]);
    assert_eq!(owners[0]["owner"], serde_json::Value::Null);
}

/// M6-2 review: a key row can be removed (a registry collision), and an owner keeps the
/// whole signing key, not only its id.
pub async fn key_delete_and_owner_signer_key(db: &TestDb) {
    let repo = db.store().read().plugin_trust();
    let now = Timestamp::now();
    repo.upsert_key(&trusted(1, now)).await.unwrap();
    assert!(repo.delete_key(&key(1).key_id).await.unwrap());
    assert!(!repo.delete_key(&key(1).key_id).await.unwrap());
    assert_eq!(repo.get_key(&key(1).key_id).await.unwrap(), None);
    repo.put_owner(&owner("seo", Some(3), now)).await.unwrap();
    let o = repo.get_owner("seo").await.unwrap().unwrap();
    assert_eq!(o.signer(), Some(key(3)));
}
