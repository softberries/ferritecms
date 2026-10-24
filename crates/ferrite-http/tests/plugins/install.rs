//! The install pipeline as a library (plan ruling P8): `Installer` inspects an upload
//! without writing anything, stages it only with the fingerprint of the report that was
//! shown (P3, R1), bounds every upload before parsing it, refuses a component that does
//! not load, and compiles at most `COMPILE_SLOTS` components at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use bytes::Bytes;
use ferrite_core::ExtensionService;
use ferrite_domain::{ArtifactHash, ArtifactKind};
use ferrite_http::api::Problem;
use ferrite_http::extensions::{
    COMPILE_SLOTS, ExtensionError, Installer, MAX_PLUGIN_BYTES, PLUGIN_MIME, THEME_MIME,
};
use ferrite_plugin_host::HostConfig;
use ferrite_theme::{MAX_BUNDLE_BYTES, ThemeSource, pack, pack_embedded};
use serde_json::json;

use crate::common::{Site, plugin_manifest, plugin_wasm};
use crate::profile_tests;

fn installer(site: &Site) -> Installer {
    Installer::new(
        site.store.clone(),
        site.media.clone(),
        HostConfig::default(),
    )
    .unwrap()
}

fn hello() -> (String, Bytes) {
    (plugin_manifest("hello"), Bytes::from(plugin_wasm("hello")))
}

/// Neither a plugin nor an artifact row nor a stored blob exists for `hash`.
async fn nothing_written(site: &Site, hash: &str) {
    assert!(site.store.read().plugins().list().await.unwrap().is_empty());
    assert!(site.store.read().themes().list().await.unwrap().is_empty());
    let hash = ArtifactHash::parse(hash).unwrap();
    assert!(
        site.store
            .read()
            .artifacts()
            .get(&hash)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(site.media.head(&hash).await.unwrap(), None, "no blob");
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn inspect_writes_nothing(site: Site) -> Site {
    let (manifest, wasm) = hello();
    let inspected = installer(&site)
        .inspect_plugin(&manifest, wasm.clone())
        .await
        .unwrap();
    let report = &inspected.report;
    assert_eq!(report.plugin, "hello");
    assert_eq!(report.version, "0.1.0");
    assert_eq!(report.artifact, sha256(&wasm));
    assert_eq!(report.fingerprint.len(), 64);
    assert_eq!(report.added, report.items, "new plugin: everything is new");
    assert_eq!(inspected.manifest.plugin.name, "hello");
    assert_eq!(inspected.manifest_json["plugin"]["name"], "hello");
    nothing_written(&site, &report.artifact).await;
    site
}

async fn stage_requires_the_fingerprint(site: Site) -> Site {
    let installer = installer(&site);
    let wasm = Bytes::from(plugin_wasm("hello"));
    // 64 MiB is above the 32 MiB host default: it loads only because it is approved.
    let manifest = format!(
        "{}\n[limits]\nmemory = \"64MiB\"\n",
        plugin_manifest("hello")
    );
    let inspected = installer
        .inspect_plugin(&manifest, wasm.clone())
        .await
        .unwrap();
    // A fingerprint of another upload (another version) does not approve this one.
    let other = installer
        .inspect_plugin(
            &manifest.replace("version   = \"0.1.0\"", "version   = \"0.2.0\""),
            wasm.clone(),
        )
        .await
        .unwrap();
    assert_ne!(other.report.fingerprint, inspected.report.fingerprint);
    for wrong in ["", "nope", other.report.fingerprint.as_str()] {
        let err = installer
            .stage_plugin(inspected.clone(), wrong)
            .await
            .unwrap_err();
        assert!(matches!(err, ExtensionError::Approval(_)), "{err:?}");
        let problem = Problem::from(err);
        assert_eq!(problem.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(problem.slug, "validation");
        assert_eq!(problem.errors[0].field.as_deref(), Some("approve"));
    }
    // The fingerprint is recomputed at staging: other bytes or another manifest under
    // the same report are not approved by it.
    let mut swapped = inspected.clone();
    swapped.wasm = Bytes::from(plugin_wasm("clock"));
    let mut widened = inspected.clone();
    widened.manifest.capabilities.kv = true;
    for changed in [swapped, widened] {
        let fingerprint = changed.report.fingerprint.clone();
        let err = installer
            .stage_plugin(changed, &fingerprint)
            .await
            .unwrap_err();
        assert!(matches!(err, ExtensionError::Approval(_)), "{err:?}");
    }
    nothing_written(&site, &inspected.report.artifact).await;
    nothing_written(&site, &sha256(&plugin_wasm("clock"))).await;

    let fingerprint = inspected.report.fingerprint.clone();
    let (artifact, row) = installer
        .stage_plugin(inspected, &fingerprint)
        .await
        .unwrap();
    assert_eq!(artifact.kind, ArtifactKind::Plugin);
    assert_eq!(artifact.mime, PLUGIN_MIME);
    assert_eq!(artifact.bytes, wasm.len() as i64);
    assert_eq!(artifact.hash, row.artifact_hash);
    assert_eq!(
        site.media.head(&artifact.hash).await.unwrap(),
        Some(wasm.len() as u64)
    );
    assert_eq!(row.name, "hello");
    assert!(!row.enabled);
    assert_eq!(
        row.granted,
        json!({ "v": 1, "capabilities": { "render": { "head": true } },
                "limits": { "memory": 64 << 20, "fuel": null }, "routes": [] })
    );
    // Staging records nothing: the caller does.
    assert!(site.store.read().plugins().list().await.unwrap().is_empty());
    ExtensionService::new(site.store.clone())
        .install_plugin(&artifact, &row)
        .await
        .unwrap();
    ExtensionService::new(site.store.clone())
        .set_plugin_enabled("hello", true)
        .await
        .unwrap();
    let summary = site.load().await;
    assert_eq!(summary.loaded, ["hello"], "{:?}", summary.skipped);

    // Against the installed grant, the same upload adds nothing; more memory does.
    let again = installer
        .inspect_plugin(&manifest, wasm.clone())
        .await
        .unwrap();
    assert!(again.report.added.is_empty(), "{:?}", again.report.added);
    let more = installer
        .inspect_plugin(&manifest.replace("64MiB", "128MiB"), wasm)
        .await
        .unwrap();
    let keys: Vec<&str> = more.report.added.iter().map(|i| i.key).collect();
    assert_eq!(keys, ["limits.memory"]);
    site
}

/// A legacy row (null `granted`) counts as `Grant::legacy` of its manifest (F9, R7):
/// its capabilities are already approved, limits above the defaults are not.
async fn legacy_row_is_the_current_grant(site: Site) -> Site {
    site.install("clock").await;
    let installer = installer(&site);
    let wasm = Bytes::from(plugin_wasm("clock"));
    let same = installer
        .inspect_plugin(&plugin_manifest("clock"), wasm.clone())
        .await
        .unwrap();
    assert!(same.report.added.is_empty(), "{:?}", same.report.added);
    let manifest = format!(
        "{}\n[limits]\nmemory = \"64MiB\"\n",
        plugin_manifest("clock")
    );
    let more = installer.inspect_plugin(&manifest, wasm).await.unwrap();
    let keys: Vec<&str> = more.report.added.iter().map(|i| i.key).collect();
    assert_eq!(keys, ["limits.memory"]);
    site
}

async fn oversized_uploads_are_refused(site: Site) -> Site {
    let installer = installer(&site);
    // Zeros: anything that parsed them would fail differently.
    let bundle = Bytes::from(vec![0u8; MAX_BUNDLE_BYTES as usize + 1]);
    let err = installer.stage_theme(bundle).await.unwrap_err();
    assert!(
        matches!(err, ExtensionError::TooLarge { limit } if limit == MAX_BUNDLE_BYTES),
        "{err:?}"
    );
    let problem = Problem::from(err);
    assert_eq!(problem.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(problem.slug, "too-large");

    let wasm = Bytes::from(vec![0u8; MAX_PLUGIN_BYTES as usize + 1]);
    let err = installer
        .inspect_plugin(&plugin_manifest("hello"), wasm)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ExtensionError::TooLarge { limit } if limit == MAX_PLUGIN_BYTES),
        "{err:?}"
    );
    // A manifest over 64 KiB is refused by `Manifest::parse`.
    let huge = format!(
        "{}\n# {}\n",
        plugin_manifest("hello"),
        "x".repeat(64 * 1024)
    );
    let err = installer
        .inspect_plugin(&huge, Bytes::from(plugin_wasm("hello")))
        .await
        .unwrap_err();
    assert!(matches!(err, ExtensionError::Manifest(_)), "{err:?}");
    assert!(site.store.read().plugins().list().await.unwrap().is_empty());
    assert!(site.store.read().themes().list().await.unwrap().is_empty());
    site
}

async fn broken_component_refused_at_inspect(site: Site) -> Site {
    let installer = installer(&site);
    let undeclared = Bytes::from(plugin_wasm("undeclared-import"));
    let err = installer
        .inspect_plugin(&plugin_manifest("undeclared-import"), undeclared.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, ExtensionError::Component(_)), "{err:?}");
    assert_eq!(Problem::from(err).status, StatusCode::UNPROCESSABLE_ENTITY);
    nothing_written(&site, &sha256(&undeclared)).await;

    let garbage = Bytes::from_static(b"not wasm");
    let err = installer
        .inspect_plugin(&plugin_manifest("hello"), garbage.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, ExtensionError::Component(_)), "{err:?}");
    nothing_written(&site, &sha256(&garbage)).await;

    // Above the ceiling: refused at inspection whatever is approved (P5).
    let manifest = format!(
        "{}\n[limits]\nmemory = \"1024MiB\"\n",
        plugin_manifest("hello")
    );
    let err = installer
        .inspect_plugin(&manifest, Bytes::from(plugin_wasm("hello")))
        .await
        .unwrap_err();
    assert!(matches!(err, ExtensionError::Component(_)), "{err:?}");

    let err = installer
        .inspect_plugin(
            "[plugin]\nname = \"x\"\n",
            Bytes::from(plugin_wasm("hello")),
        )
        .await
        .unwrap_err();
    let ExtensionError::Manifest(manifest_error) = &err else {
        panic!("{err:?}");
    };
    let paths: Vec<String> = manifest_error
        .problems
        .iter()
        .map(|p| p.path.clone())
        .collect();
    let problem = Problem::from(err);
    assert_eq!(problem.status, StatusCode::UNPROCESSABLE_ENTITY);
    let fields: Vec<String> = problem
        .errors
        .iter()
        .map(|e| e.field.clone().unwrap_or_default())
        .collect();
    assert_eq!(fields, paths);
    site
}

/// Three concurrent inspections: at most `COMPILE_SLOTS` (2) compiles run at once.
async fn compile_slots_bound_concurrency(site: Site) -> Site {
    assert_eq!(COMPILE_SLOTS, 2);
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicUsize::new(0));
    let (r, p, t) = (running.clone(), peak.clone(), total.clone());
    let installer = installer(&site).with_prepare(move |host, manifest, wasm, approved| {
        let now = r.fetch_add(1, Ordering::SeqCst) + 1;
        p.fetch_max(now, Ordering::SeqCst);
        t.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        let out = host.prepare_with(manifest, wasm, approved);
        r.fetch_sub(1, Ordering::SeqCst);
        out
    });
    let (manifest, wasm) = hello();
    let (a, b, c) = tokio::join!(
        installer.inspect_plugin(&manifest, wasm.clone()),
        installer.inspect_plugin(&manifest, wasm.clone()),
        installer.inspect_plugin(&manifest, wasm.clone()),
    );
    for r in [a, b, c] {
        r.unwrap();
    }
    assert_eq!(total.load(Ordering::SeqCst), 3);
    assert_eq!(peak.load(Ordering::SeqCst), 2, "two slots, both used");
    assert_eq!(running.load(Ordering::SeqCst), 0);
    site
}

async fn themes_stage_and_load_from_artifacts(site: Site) -> Site {
    let installer = installer(&site);
    let bundle = Bytes::from(pack_embedded("blog").unwrap());
    let (artifact, row) = installer.stage_theme(bundle.clone()).await.unwrap();
    assert_eq!(artifact.kind, ArtifactKind::Theme);
    assert_eq!(artifact.mime, THEME_MIME);
    assert_eq!(artifact.bytes, bundle.len() as i64);
    assert_eq!(artifact.hash.as_str(), sha256(&bundle));
    assert_eq!(row.name, "blog");
    assert!(!row.active);
    // F10: an array, the form T6's schema uses.
    assert_eq!(row.manifest["settings"], json!([]));
    assert!(row.manifest["images"].is_object(), "{}", row.manifest);
    assert_eq!(row.manifest["name"], "blog");
    assert!(site.store.read().themes().list().await.unwrap().is_empty());
    let (embedded, embedded_row) = installer.stage_embedded("blog").await.unwrap();
    assert_eq!(embedded.hash, artifact.hash);
    assert_eq!(embedded_row.manifest, row.manifest);
    assert!(matches!(
        installer.stage_embedded("missing").await,
        Err(ExtensionError::Theme(_))
    ));

    ExtensionService::new(site.store.clone())
        .install_theme(&artifact, &row, true)
        .await
        .unwrap();
    let theme = installer.theme_from_artifact(&row).await.unwrap();
    assert_eq!(theme.name(), "blog");

    // A row naming another theme than its bundle holds.
    let mut renamed = row.clone();
    renamed.name = "mine".into();
    let err = installer.theme_from_artifact(&renamed).await.unwrap_err();
    assert!(
        matches!(&err, ExtensionError::NameMismatch { bundle, row } if bundle == "blog" && row == "mine"),
        "{err:?}"
    );
    assert_eq!(Problem::from(err).status, StatusCode::UNPROCESSABLE_ENTITY);

    // F11: tampered bytes are an actionable 422, not a 500.
    site.tamper(&row.artifact_hash, b"not a tar");
    let err = installer.theme_from_artifact(&row).await.unwrap_err();
    assert!(matches!(err, ExtensionError::HashMismatch), "{err:?}");
    assert!(err.to_string().contains("hash mismatch"), "{err}");
    assert_eq!(Problem::from(err).status, StatusCode::UNPROCESSABLE_ENTITY);

    // A theme that does not compile is refused before anything is uploaded.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("templates")).unwrap();
    std::fs::write(
        dir.path().join("theme.toml"),
        "[theme]\nname = \"broken\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("templates/index.html"), "{% if %}").unwrap();
    let broken = Bytes::from(pack(&ThemeSource::Dir(dir.path().into())).unwrap());
    let err = installer.stage_theme(broken.clone()).await.unwrap_err();
    assert!(matches!(err, ExtensionError::Theme(_)), "{err:?}");
    assert_eq!(Problem::from(err).status, StatusCode::UNPROCESSABLE_ENTITY);
    let hash = ArtifactHash::parse(&sha256(&broken)).unwrap();
    assert_eq!(site.media.head(&hash).await.unwrap(), None, "not uploaded");
    site
}

profile_tests!(
    inspect_writes_nothing,
    stage_requires_the_fingerprint,
    legacy_row_is_the_current_grant,
    oversized_uploads_are_refused,
    broken_component_refused_at_inspect,
    compile_slots_bound_concurrency,
    themes_stage_and_load_from_artifacts,
);
