#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use ferrite_media::{Backend, MediaConfig, MediaError, MediaStore};

fn with_prefix(mut cfg: MediaConfig, prefix: &str) -> MediaConfig {
    cfg.prefix = prefix.into();
    cfg
}

backend_tests! {
    open_and_check_ok(backing) {
        let store = backing.store();
        store.check().await.expect("check");
        // probe is cleaned up: a second check works too
        store.check().await.expect("second check");
    }

    prefix_rules(backing) {
        for bad in ["/x/", "x"] {
            let err = MediaStore::open(with_prefix(backing.config.clone(), bad))
                .err()
                .expect("must fail");
            assert!(matches!(err, MediaError::Config(_)), "{bad}: {err:?}");
        }
        for ok in ["", "site/a/"] {
            let store = MediaStore::open(with_prefix(backing.config.clone(), ok)).expect("open");
            store.check().await.expect("check with prefix");
        }
    }
}

#[tokio::test]
async fn check_fails_on_unreachable_s3() {
    let cfg = common::s3_config("http://127.0.0.1:1".into(), "");
    let store = MediaStore::open(cfg).expect("open");
    let err = store.check().await.expect_err("unreachable");
    assert!(matches!(err, MediaError::Backend(_)), "{err:?}");
}

#[tokio::test]
async fn s3_requires_both_credentials() {
    let mut cfg = common::s3_config("http://127.0.0.1:1".into(), "");
    if let Backend::S3(s3) = &mut cfg.backend {
        s3.secret_access_key = None;
    }
    let err = MediaStore::open(cfg).err().expect("must fail");
    assert!(matches!(err, MediaError::Config(_)), "{err:?}");
}
