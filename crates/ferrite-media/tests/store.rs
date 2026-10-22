#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
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

/// Deterministic xorshift64 bytes.
fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn chunked(
    data: &[u8],
    size: usize,
) -> futures::stream::Iter<std::vec::IntoIter<Result<Bytes, std::io::Error>>> {
    let items: Vec<_> = data
        .chunks(size)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    futures::stream::iter(items)
}

/// Number of regular files below `dir` (0 if it does not exist).
fn count_files(dir: &std::path::Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.map(|e| e.unwrap().path())
        .map(|p| if p.is_dir() { count_files(&p) } else { 1 })
        .sum()
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
        assert!(store.get_range(&r.hash, len..len).await.unwrap().is_empty());
        assert!(matches!(store.get_range(&r.hash, len..len + 1).await, Err(MediaError::Range(_))));
        let inverted = std::ops::Range { start: 5, end: 3 };
        assert!(matches!(store.get_range(&r.hash, inverted).await, Err(MediaError::Range(_))));
    }

    delete_idempotent(backing) {
        let store = backing.store();
        let r = store.put_artifact(Bytes::from_static(DATA)).await.unwrap();
        store.delete(&r.hash).await.unwrap();
        assert_eq!(store.head(&r.hash).await.unwrap(), None);
        store.delete(&r.hash).await.unwrap();
    }

    put_artifact_at_limit_delegates_to_stream(backing) {
        let store = backing.store();
        let data = pseudo_random(8 * 1024 * 1024, 3);
        let r = store.put_artifact(Bytes::from(data.clone())).await.unwrap();
        assert_eq!(r.hash.as_str(), sha_hex(&data));
        assert_eq!(r.bytes, data.len() as u64);
        assert!(r.created);
        assert_eq!(store.get(&r.hash).await.unwrap()[..], data[..]);
        assert_eq!(count_files(&backing.object_path("tmp")), 0);
    }

    stream_large_roundtrip(backing) {
        let store = backing.store();
        let data = pseudo_random(20 * 1024 * 1024, 1);
        let r = store.put_stream(chunked(&data, 64 * 1024)).await.unwrap();
        assert_eq!(r.hash.as_str(), sha_hex(&data));
        assert_eq!(r.bytes, data.len() as u64);
        assert_eq!(r.mime, "application/octet-stream");
        assert!(r.created);
        assert_eq!(store.get(&r.hash).await.unwrap()[..], data[..]);
        assert_eq!(count_files(&backing.object_path("tmp")), 0);
        let again = store.put_stream(chunked(&data, 100_000)).await.unwrap();
        assert_eq!(again.hash, r.hash);
        assert!(!again.created);
        assert_eq!(count_files(&backing.object_path("tmp")), 0);
    }

    stream_sniffs_mime_from_first_chunk(backing) {
        let store = backing.store();
        let mut data = pseudo_random(9 * 1024 * 1024, 5);
        data[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let r = store.put_stream(chunked(&data, 1000)).await.unwrap();
        assert_eq!(r.mime, "image/png");
    }

    stream_small_uses_direct_path(backing) {
        let store = backing.store();
        let data = pseudo_random(100 * 1024, 2);
        let r = store.put_stream(chunked(&data, 4096)).await.unwrap();
        assert_eq!(r.hash.as_str(), sha_hex(&data));
        assert!(r.created);
        assert_eq!(store.get(&r.hash).await.unwrap()[..], data[..]);
        // The direct path never touches tmp/ at all.
        assert!(!backing.object_path("tmp").exists());
        let again = store.put_stream(chunked(&data, 4096)).await.unwrap();
        assert!(!again.created);
    }

    stream_empty(backing) {
        let store = backing.store();
        let r = store.put_stream(chunked(&[], 10)).await.unwrap();
        assert_eq!(r.bytes, 0);
        assert_eq!(r.hash.as_str(), sha_hex(b""));
    }

    stream_boundaries(backing) {
        let store = backing.store();
        const MIB: usize = 1024 * 1024;
        for (len, chunk, seed) in [
            (8 * MIB, 64 * 1024, 11),
            (8 * MIB + 1, 64 * 1024, 12),
            (9 * MIB, 3 * MIB, 13),
            (8 * MIB - 1, 64 * 1024, 14),
        ] {
            let data = pseudo_random(len, seed);
            let r = store.put_stream(chunked(&data, chunk)).await.unwrap();
            assert_eq!(r.hash.as_str(), sha_hex(&data), "len {len}");
            assert_eq!(r.bytes, len as u64);
            assert_eq!(store.get(&r.hash).await.unwrap()[..], data[..], "len {len}");
            assert_eq!(count_files(&backing.object_path("tmp")), 0);
        }
    }

    interrupted_during_buffering_leaves_nothing(backing) {
        let store = backing.store();
        let data = pseudo_random(2 * 1024 * 1024, 6);
        let mut items: Vec<Result<Bytes, std::io::Error>> = data
            .chunks(64 * 1024)
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        items.push(Err(std::io::Error::other("boom")));
        let err = store.put_stream(futures::stream::iter(items)).await.unwrap_err();
        assert!(matches!(err, MediaError::Io(_)), "{err:?}");
        assert_eq!(count_files(&backing.object_path("tmp")), 0);
        assert_eq!(count_files(&backing.object_path("artifacts")), 0);
    }

    cancelled_upload_cleans_staging(backing) {
        let store = backing.store();
        let data = pseudo_random(20 * 1024 * 1024, 8);
        let chunks: Vec<Bytes> = data.chunks(64 * 1024).map(Bytes::copy_from_slice).collect();
        // ~10 MiB flow immediately, then the producer stalls forever.
        let fast = 160;
        let stream = futures::StreamExt::then(
            futures::stream::iter(chunks.into_iter().enumerate()),
            move |(i, c)| async move {
                if i >= fast {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
                Ok::<_, std::io::Error>(c)
            },
        );
        let res = tokio::time::timeout(
            Duration::from_millis(1500),
            store.put_stream(Box::pin(stream)),
        )
        .await;
        assert!(res.is_err(), "upload should have been cancelled by the timeout");
        let mut left = usize::MAX;
        for _ in 0..50 {
            left = count_files(backing.storage_root());
            if left == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(left, 0);
        assert_eq!(count_files(&backing.object_path("artifacts")), 0);
    }

    interrupted_stream_leaves_nothing(backing) {
        let store = backing.store();
        let data = pseudo_random(10 * 1024 * 1024, 4);
        let mut items: Vec<Result<Bytes, std::io::Error>> = data
            .chunks(64 * 1024)
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        items.push(Err(std::io::Error::other("client went away")));
        let err = store.put_stream(futures::stream::iter(items)).await.unwrap_err();
        assert!(matches!(err, MediaError::Io(_)), "{err:?}");
        assert_eq!(count_files(&backing.object_path("tmp")), 0);
        assert_eq!(count_files(&backing.object_path("artifacts")), 0);
        assert_eq!(store.head(&ArtifactHash::parse(&sha_hex(&data)).unwrap()).await.unwrap(), None);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_payload_local() {
    concurrent_same_payload(common::local_backend(""), true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_payload_s3() {
    concurrent_same_payload(common::s3_backend("").await, false).await;
}

async fn concurrent_same_payload(backing: common::Backing, local: bool) {
    let store = Arc::new(backing.store());
    let data = Arc::new(pseudo_random(10 * 1024 * 1024, 9));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        let data = data.clone();
        tasks.push(tokio::spawn(async move {
            store.put_stream(chunked(&data, 64 * 1024)).await.unwrap()
        }));
    }
    let mut refs = Vec::new();
    for t in tasks {
        refs.push(t.await.unwrap());
    }
    assert!(refs.iter().all(|r| r.hash.as_str() == sha_hex(&data)));
    assert!(refs.iter().all(|r| r.bytes == data.len() as u64));
    let creators = refs.iter().filter(|r| r.created).count();
    if local {
        assert_eq!(creators, 1);
    } else {
        // The S3 test server lacks copy-if-not-exists; the head+copy fallback may race.
        assert!(creators >= 1);
    }
    assert_eq!(count_files(&backing.object_path("artifacts")), 1);
    assert_eq!(count_files(&backing.object_path("tmp")), 0);
    assert_eq!(store.get(&refs[0].hash).await.unwrap()[..], data[..]);
}

backend_tests! {
    just_below_limit_is_accepted(backing) {
        let store = backing.store();
        let data = Bytes::from(vec![7u8; 8 * 1024 * 1024 - 1]);
        let r = store.put_artifact(data).await.unwrap();
        assert_eq!(r.bytes, 8 * 1024 * 1024 - 1);
        assert!(r.created);
    }

    known_answer_sha256(backing) {
        let store = backing.store();
        let r = store.put_artifact(Bytes::from_static(b"abc")).await.unwrap();
        assert_eq!(
            r.hash.as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
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

        for (name, want) in [
            ("a/../b", "a%2F..%2Fb"),
            ("..", "file"),
            (".", "file"),
            ("", "file"),
        ] {
            assert_eq!(store.public_url(&hash, name), format!("/_media/{hash}/{want}"));
        }

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
