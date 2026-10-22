#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use bytes::Bytes;
use ferrite_domain::ArtifactHash;
use ferrite_media::{Backend, MediaConfig, MediaError, MediaStore, PublicUrlMode};
use sha2::{Digest, Sha256};

fn sha_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn key_for(prefix: &str, hex: &str) -> String {
    format!("{prefix}artifacts/{}/{hex}", &hex[..2])
}

const DATA: &[u8] = b"hello ferrite media";

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

    put_get_roundtrip(backing) {
        let store = backing.store();
        let r = store.put_artifact(Bytes::from_static(DATA)).await.unwrap();
        assert_eq!(r.hash.as_str(), sha_hex(DATA));
        assert_eq!(r.bytes, DATA.len() as u64);
        assert_eq!(r.mime, "application/octet-stream");
        assert!(r.created);
        assert_eq!(&store.get(&r.hash).await.unwrap()[..], DATA);
        assert_eq!(store.head(&r.hash).await.unwrap(), Some(DATA.len() as u64));
        let path = backing.object_path(&key_for("", r.hash.as_str()));
        assert_eq!(std::fs::read(path).unwrap(), DATA);
    }

    put_same_bytes_twice_is_idempotent(backing) {
        // Both backends here are filesystem-backed (local root, s3s-fs), so the object's file
        // mtime is observable: an unchanged mtime proves the object was not rewritten.
        // `created == false` and identical content are asserted as well.
        let store = backing.store();
        let first = store.put_artifact(Bytes::from_static(DATA)).await.unwrap();
        let path = backing.object_path(&key_for("", first.hash.as_str()));
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let second = store.put_artifact(Bytes::from_static(DATA)).await.unwrap();
        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.hash, second.hash);
        assert_eq!(first.bytes, second.bytes);
        assert_eq!(first.mime, second.mime);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
        assert_eq!(std::fs::read(&path).unwrap(), DATA);
    }

    get_missing_is_not_found(backing) {
        let store = backing.store();
        let hash = ArtifactHash::parse(&"0".repeat(64)).unwrap();
        assert!(matches!(store.get(&hash).await, Err(MediaError::NotFound)));
        assert_eq!(store.head(&hash).await.unwrap(), None);
        assert!(matches!(store.get_range(&hash, 0..1).await, Err(MediaError::NotFound)));
    }

    range_reads(backing) {
        let store = backing.store();
        let r = store.put_artifact(Bytes::from_static(DATA)).await.unwrap();
        let len = DATA.len() as u64;
        assert_eq!(&store.get_range(&r.hash, 0..5).await.unwrap()[..], &DATA[..5]);
        assert_eq!(&store.get_range(&r.hash, 5..len).await.unwrap()[..], &DATA[5..]);
        assert!(store.get_range(&r.hash, 3..3).await.unwrap().is_empty());
        assert!(matches!(store.get_range(&r.hash, len..len + 1).await, Err(MediaError::Range(_))));
        #[allow(clippy::reversed_empty_ranges)]
        let inverted = 5..3;
        assert!(matches!(store.get_range(&r.hash, inverted).await, Err(MediaError::Range(_))));
    }

    delete_idempotent(backing) {
        let store = backing.store();
        let r = store.put_artifact(Bytes::from_static(DATA)).await.unwrap();
        store.delete(&r.hash).await.unwrap();
        assert_eq!(store.head(&r.hash).await.unwrap(), None);
        store.delete(&r.hash).await.unwrap();
    }

    oversized_put_artifact_is_rejected(backing) {
        let store = backing.store();
        let big = Bytes::from(vec![0u8; 8 * 1024 * 1024]);
        let err = store.put_artifact(big).await.unwrap_err();
        assert!(matches!(err, MediaError::Config(_)), "{err:?}");
    }

    sniffs_content_on_put(backing) {
        let store = backing.store();
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        let r = store.put_artifact(Bytes::copy_from_slice(&png)).await.unwrap();
        assert_eq!(r.mime, "image/png");
    }

    public_url_modes(backing) {
        let hash = ArtifactHash::parse(&sha_hex(DATA)).unwrap();
        let store = backing.store();
        let url = store.public_url(&hash, "my photo é#1?.png");
        assert_eq!(url, format!("/_media/{hash}/my%20photo%20%C3%A9%231%3F.png"));

        for (base, prefix) in [("https://cdn.example.com/m", ""), ("https://cdn.example.com/m/", "site/a/")] {
            let mut cfg = backing.config.clone();
            cfg.prefix = prefix.into();
            cfg.public = PublicUrlMode::Direct { base_url: base.parse().unwrap() };
            let store = MediaStore::open(cfg).unwrap();
            assert_eq!(
                store.public_url(&hash, "ignored name.png"),
                format!("https://cdn.example.com/m/{prefix}artifacts/{}/{hash}", &hash.as_str()[..2])
            );
        }
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
        // objects land under the prefix
        let prefixed = MediaStore::open(with_prefix(backing.config.clone(), "site/a/")).unwrap();
        let r = prefixed.put_artifact(Bytes::from_static(DATA)).await.unwrap();
        let hex = r.hash.as_str();
        assert!(backing.object_path(&key_for("site/a/", hex)).is_file());
        assert!(!backing.object_path(&key_for("", hex)).exists());
        // the probe was deleted again
        let tmp = backing.object_path("site/a/tmp");
        if tmp.exists() {
            assert_eq!(std::fs::read_dir(tmp).unwrap().count(), 0);
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
