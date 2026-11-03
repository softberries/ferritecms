//! `plugins.registry_check` (M6-2, design 14 §8, ruling P5) against a signed `file://`
//! registry made with throwaway keys, on both backends.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../ferrite-http/tests/support/registry_fixture.rs"]
mod registry_fixture;

use std::sync::Arc;

use ferrite::jobs::{REGISTRY_CHECK_KIND, RegistryCheckHandler};
use ferrite_cluster::{JobError, JobHandler};
use ferrite_domain::{Job, JobId, JobStatus, Timestamp};
use ferrite_http::extensions::{Installer, RegistryTarget, TrustSettings};
use ferrite_http::registry_fetch::FileRegistry;
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_plugin_host::HostConfig;
use ferrite_store_tests::{TestDb, backend_tests};
use registry_fixture::FileRegistryFixture;
use serde_json::json;

const MANIFEST: &str = "[plugin]\nname      = \"hello\"\nversion   = \"0.1.0\"\npublisher = \"ferritecms.dev\"\nworld     = \"ferrite:plugin/plugin@0.1.0\"\n";

fn job() -> Job {
    let now = Timestamp::now();
    Job {
        id: JobId::new(),
        kind: REGISTRY_CHECK_KIND.into(),
        payload: json!({"v": 1}),
        unique_key: None,
        run_at: now,
        priority: 0,
        status: JobStatus::Running,
        attempts: 1,
        max_attempts: 3,
        locked_by: None,
        locked_until: None,
        last_error: None,
        created_at: now,
        finished_at: None,
    }
}

fn media(dir: &std::path::Path) -> Arc<MediaStore> {
    Arc::new(
        MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: dir.join("media"),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .unwrap(),
    )
}

fn handler(
    db: &TestDb,
    dir: &std::path::Path,
    fx: Option<&FileRegistryFixture>,
) -> RegistryCheckHandler {
    let trust = match fx {
        None => TrustSettings::default(),
        Some(fx) => TrustSettings {
            registry: Some(RegistryTarget {
                url: url::Url::parse(&fx.url()).unwrap(),
                key: fx.registry_key.key.clone(),
            }),
            source: Some(Arc::new(FileRegistry::new(fx.path().to_path_buf()))),
            ..Default::default()
        },
    };
    let installer = Installer::new(db.shared_store(), media(dir), HostConfig::default())
        .unwrap()
        .with_trust(trust);
    RegistryCheckHandler {
        installer: Arc::new(installer),
    }
}

async fn job_refreshes_cache_from_file_registry(db: &TestDb) {
    let dir = tempfile::tempdir().unwrap();
    let mut fx = FileRegistryFixture::new();
    fx.add_version("hello", "0.1.0", MANIFEST, b"\0asm", None);
    fx.publish();
    let h = handler(db, dir.path(), Some(&fx));
    h.run(&job()).await.unwrap();
    let row = db
        .store()
        .read()
        .plugin_trust()
        .get_registry(&fx.url())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.serial, 1);
    assert_eq!(row.last_error, None);
}

async fn job_without_registry_is_done(db: &TestDb) {
    let dir = tempfile::tempdir().unwrap();
    handler(db, dir.path(), None).run(&job()).await.unwrap();
}

async fn job_records_invalid_index_and_stops(db: &TestDb) {
    let dir = tempfile::tempdir().unwrap();
    let mut fx = FileRegistryFixture::new();
    fx.add_version("hello", "0.1.0", MANIFEST, b"\0asm", None);
    fx.publish();
    let h = handler(db, dir.path(), Some(&fx));
    h.run(&job()).await.unwrap();
    // A newer index whose bytes no longer match its signature.
    fx.serial = 2;
    fx.publish();
    let path = fx.path().join("index.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("Test registry", "Tampered")).unwrap();
    let err = h.run(&job()).await.unwrap_err();
    assert!(matches!(err, JobError::Permanent(_)), "{err:?}");
    let row = db
        .store()
        .read()
        .plugin_trust()
        .get_registry(&fx.url())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.serial, 1);
    assert!(row.last_error.is_some());
    // A registry that cannot be read is a retry.
    std::fs::remove_file(fx.path().join("index.json.minisig")).unwrap();
    std::fs::remove_file(&path).unwrap();
    let err = h.run(&job()).await.unwrap_err();
    assert!(matches!(err, JobError::Retry(_)), "{err:?}");
}

backend_tests!(
    job_refreshes_cache_from_file_registry,
    job_without_registry_is_done,
    job_records_invalid_index_and_stops,
);
