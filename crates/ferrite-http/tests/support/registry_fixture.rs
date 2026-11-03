//! A `file://` plugin registry made in a test (M6-2): a folder with plugin files,
//! `index.json` and `index.json.minisig`, signed with throwaway keys made here (a
//! publisher key and a registry key). Shared by the ferrite-http and ferrite tests
//! (`#[path]`).

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use ferrite_domain::plugin_trust::PublisherKey;
use ferrite_plugin_host::trust::signing_payload;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A throwaway minisign key pair.
pub struct TestKey {
    pub kp: minisign::KeyPair,
    pub key: PublisherKey,
}

impl TestKey {
    pub fn new() -> Self {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let key = PublisherKey::parse(&kp.pk.to_box().unwrap().into_string())
            .unwrap()
            .unwrap();
        Self { kp, key }
    }

    pub fn sign(&self, data: &[u8]) -> String {
        minisign::sign(Some(&self.kp.pk), &self.kp.sk, data, None, None)
            .unwrap()
            .into_string()
    }

    /// The public key file (`ferrite-plugin.pub`).
    pub fn public_file(&self) -> String {
        self.key.to_file()
    }
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// See the module docs.
pub struct FileRegistryFixture {
    pub dir: tempfile::TempDir,
    pub registry_key: TestKey,
    pub publisher_key: TestKey,
    pub serial: u64,
    /// The publisher's revoked keys: `(key, compromised)`.
    pub revoked: Vec<(PublisherKey, bool)>,
    /// The publisher's extra current keys.
    pub extra_keys: Vec<PublisherKey>,
    plugins: Vec<(String, Vec<Value>)>,
}

impl FileRegistryFixture {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            registry_key: TestKey::new(),
            publisher_key: TestKey::new(),
            serial: 1,
            revoked: Vec::new(),
            extra_keys: Vec::new(),
            plugins: Vec::new(),
        }
    }

    /// The index URL (`file:///…/index.json`).
    pub fn url(&self) -> String {
        url::Url::from_file_path(self.dir.path().join("index.json"))
            .unwrap()
            .to_string()
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Adds `name` `version` (the manifest text's version is set to `version`), signed
    /// by `signer` (default: the publisher key); returns the files' folder.
    pub fn add_version(
        &mut self,
        name: &str,
        version: &str,
        manifest: &str,
        wasm: &[u8],
        signer: Option<&TestKey>,
    ) -> PathBuf {
        let manifest = set_version(manifest, version);
        let folder = self.dir.path().join(name).join(version);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("ferrite-plugin.toml"), &manifest).unwrap();
        std::fs::write(folder.join(format!("{name}.wasm")), wasm).unwrap();
        let payload = signing_payload(manifest.as_bytes(), wasm);
        let signature = signer
            .unwrap_or(&self.publisher_key)
            .sign(payload.as_bytes());
        let entry = json!({
            "version": version,
            "world": "ferrite:plugin/plugin@0.1.0",
            "published_at": "2026-11-20T10:00:00Z",
            "manifest": {"url": format!("{name}/{version}/ferrite-plugin.toml"), "sha256": sha(manifest.as_bytes()), "bytes": manifest.len()},
            "component": {"url": format!("{name}/{version}/{name}.wasm"), "sha256": sha(wasm), "bytes": wasm.len()},
            "signature": signature,
            "yanked": null,
        });
        match self.plugins.iter_mut().find(|(n, _)| n == name) {
            Some((_, versions)) => versions.push(entry),
            None => self.plugins.push((name.to_owned(), vec![entry])),
        }
        folder
    }

    /// Edits the entry of `name` `version` (e.g. to yank it or change its world).
    pub fn edit(&mut self, name: &str, version: &str, f: impl FnOnce(&mut Value)) {
        let (_, versions) = self.plugins.iter_mut().find(|(n, _)| n == name).unwrap();
        let v = versions
            .iter_mut()
            .find(|v| v["version"] == version)
            .unwrap();
        f(v);
    }

    /// The index document.
    pub fn index(&self) -> Value {
        let mut keys = vec![self.publisher_key.key.base64.clone()];
        keys.extend(self.extra_keys.iter().map(|k| k.base64.clone()));
        json!({
            "schema": 1,
            "name": "Test registry",
            "serial": self.serial,
            "generated_at": "2026-11-20T10:00:00Z",
            "publishers": [{
                "id": "ferritecms.dev",
                "name": "FerriteCMS",
                "keys": keys,
                "revoked_keys": self.revoked.iter().map(|(k, c)| json!({
                    "key": k.base64, "revoked_at": "2026-11-21T00:00:00Z",
                    "reason": "leaked", "compromised": c,
                })).collect::<Vec<_>>(),
            }],
            "plugins": self.plugins.iter().map(|(name, versions)| json!({
                "name": name,
                "publisher": "ferritecms.dev",
                "description": format!("The {name} plugin"),
                "license": "AGPL-3.0-only",
                "versions": versions,
            })).collect::<Vec<_>>(),
        })
    }

    /// Writes `index.json` and its signature.
    pub fn publish(&self) {
        let bytes = serde_json::to_vec_pretty(&self.index()).unwrap();
        std::fs::write(self.dir.path().join("index.json"), &bytes).unwrap();
        std::fs::write(
            self.dir.path().join("index.json.minisig"),
            self.registry_key.sign(&bytes),
        )
        .unwrap();
    }
}

/// `manifest` with its `version` line set to `version`.
pub fn set_version(manifest: &str, version: &str) -> String {
    manifest
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("version") && l.contains('=') {
                format!("version   = \"{version}\"")
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}
