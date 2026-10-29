#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ferrite_domain::{ArtifactHash, Timestamp};
use ferrite_media::{
    Backend, ConditionalPut, MediaConfig, MediaError, MediaStore, OWNER_MARKER, OwnerMarker,
    PublicUrlMode, StoredObject, validate_copy_if_not_exists,
};
use futures::TryStreamExt;
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

async fn collect(
    stream: futures::stream::BoxStream<'static, Result<Bytes, MediaError>>,
) -> Vec<u8> {
    use futures::StreamExt;
    let mut stream = stream;
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.unwrap());
    }
    out
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

    open_read_streams_large_artifact_fully_and_by_range(backing) {
        let store = backing.store();
        const MIB: usize = 1024 * 1024;
        let data = pseudo_random(20 * MIB, 31);
        let r = store.put_stream(chunked(&data, 64 * 1024), u64::MAX).await.unwrap();
        let total = data.len() as u64;

        let full = store.open_read(&r.hash, None).await.unwrap();
        assert_eq!(full.size, total);
        assert_eq!(full.range, 0..total);
        assert_eq!(collect(full.stream).await[..], data[..]);

        for range in [0..5u64, 3 * MIB as u64..7 * MIB as u64 + 11, total - 10..total] {
            let part = store.open_read(&r.hash, Some(range.clone())).await.unwrap();
            assert_eq!(part.size, total);
            assert_eq!(part.range, range);
            let got = collect(part.stream).await;
            assert_eq!(got[..], data[range.start as usize..range.end as usize]);
        }

        let empty = store.open_read(&r.hash, Some(total..total)).await.unwrap();
        assert_eq!(empty.size, total);
        assert_eq!(empty.range, total..total);
        assert!(collect(empty.stream).await.is_empty());
        let empty = store.open_read(&r.hash, Some(4..4)).await.unwrap();
        assert!(collect(empty.stream).await.is_empty());
    }

    open_read_error_parity_with_get_range(backing) {
        let store = backing.store();
        let r = store.put_artifact(Bytes::from_static(DATA)).await.unwrap();
        let len = DATA.len() as u64;
        let missing = ArtifactHash::parse(&"0".repeat(64)).unwrap();
        let ranges = [
            None,
            Some(0..1),
            Some(3..3),
            Some(len..len),
            Some(len..len + 1),
            Some(len + 5..len + 9),
            Some(0..len + 100),
            Some(std::ops::Range { start: 5, end: 3 }),
            Some(std::ops::Range { start: len + 5, end: 3 }),
        ];
        for range in ranges {
            for hash in [&r.hash, &missing] {
                let opened = store.open_read(hash, range.clone()).await.map(|_| ());
                let buffered = match &range {
                    Some(rg) => store.get_range(hash, rg.clone()).await.map(|_| ()),
                    None => store.get(hash).await.map(|_| ()),
                };
                match (opened, buffered) {
                    (Ok(()), Ok(())) => {}
                    (Err(MediaError::NotFound), Err(MediaError::NotFound)) => {}
                    (Err(MediaError::Range(a)), Err(MediaError::Range(b))) => {
                        assert_eq!(a, b, "{range:?}");
                    }
                    (a, b) => panic!("mismatch for {range:?}: open_read {a:?} vs get_range {b:?}"),
                }
            }
        }
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
        let r = store.put_stream(chunked(&data, 64 * 1024), u64::MAX).await.unwrap();
        assert_eq!(r.hash.as_str(), sha_hex(&data));
        assert_eq!(r.bytes, data.len() as u64);
        assert_eq!(r.mime, "application/octet-stream");
        assert!(r.created);
        assert_eq!(store.get(&r.hash).await.unwrap()[..], data[..]);
        assert_eq!(count_files(&backing.object_path("tmp")), 0);
        let again = store.put_stream(chunked(&data, 100_000), u64::MAX).await.unwrap();
        assert_eq!(again.hash, r.hash);
        assert!(!again.created);
        assert_eq!(count_files(&backing.object_path("tmp")), 0);
    }

    stream_sniffs_mime_from_first_chunk(backing) {
        let store = backing.store();
        let mut data = pseudo_random(9 * 1024 * 1024, 5);
        data[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let r = store.put_stream(chunked(&data, 1000), u64::MAX).await.unwrap();
        assert_eq!(r.mime, "image/png");
    }

    stream_small_uses_direct_path(backing) {
        let store = backing.store();
        let data = pseudo_random(100 * 1024, 2);
        let r = store.put_stream(chunked(&data, 4096), u64::MAX).await.unwrap();
        assert_eq!(r.hash.as_str(), sha_hex(&data));
        assert!(r.created);
        assert_eq!(store.get(&r.hash).await.unwrap()[..], data[..]);
        // The direct path never touches tmp/ at all.
        assert!(!backing.object_path("tmp").exists());
        let again = store.put_stream(chunked(&data, 4096), u64::MAX).await.unwrap();
        assert!(!again.created);
    }

    stream_empty(backing) {
        let store = backing.store();
        let r = store.put_stream(chunked(&[], 10), u64::MAX).await.unwrap();
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
            let r = store.put_stream(chunked(&data, chunk), u64::MAX).await.unwrap();
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
        let err = store.put_stream(futures::stream::iter(items), u64::MAX).await.unwrap_err();
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
            store.put_stream(Box::pin(stream), u64::MAX),
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

    stream_cap_exceeded_is_too_large(backing) {
        let store = backing.store();
        const MIB: usize = 1024 * 1024;
        let data = pseudo_random(10 * MIB, 21);
        let err = store
            .put_stream(chunked(&data, 64 * 1024), (9 * MIB) as u64)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::TooLarge { limit } if limit == (9 * MIB) as u64), "{err:?}");
        assert_eq!(count_files(backing.storage_root()), 0);
        assert_eq!(store.head(&ArtifactHash::parse(&sha_hex(&data)).unwrap()).await.unwrap(), None);
        // Exceeded while still buffering (below the 8 MiB switch).
        let small = pseudo_random(100 * 1024, 22);
        let err = store.put_stream(chunked(&small, 4096), 50 * 1024).await.unwrap_err();
        assert!(matches!(err, MediaError::TooLarge { limit: 51200 }), "{err:?}");
        assert_eq!(count_files(&backing.object_path("artifacts")), 0);
    }

    stream_cap_boundary_exact_succeeds(backing) {
        let store = backing.store();
        const MIB: usize = 1024 * 1024;
        let data = pseudo_random(9 * MIB, 23);
        let r = store.put_stream(chunked(&data, 64 * 1024), (9 * MIB) as u64).await.unwrap();
        assert_eq!(r.bytes, (9 * MIB) as u64);
        assert_eq!(r.hash.as_str(), sha_hex(&data));
        assert_eq!(count_files(&backing.object_path("tmp")), 0);
        let small = pseudo_random(1000, 24);
        let r = store.put_stream(chunked(&small, 100), 1000).await.unwrap();
        assert_eq!(r.bytes, 1000);
        let err = store.put_stream(chunked(&small, 100), 999).await.unwrap_err();
        assert!(matches!(err, MediaError::TooLarge { limit: 999 }), "{err:?}");
    }

    interrupted_stream_leaves_nothing(backing) {
        let store = backing.store();
        let data = pseudo_random(10 * 1024 * 1024, 4);
        let mut items: Vec<Result<Bytes, std::io::Error>> = data
            .chunks(64 * 1024)
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        items.push(Err(std::io::Error::other("client went away")));
        let err = store.put_stream(futures::stream::iter(items), u64::MAX).await.unwrap_err();
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
            store
                .put_stream(chunked(&data, 64 * 1024), u64::MAX)
                .await
                .unwrap()
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
        // A comma is escaped, so the URL stays one candidate inside a `srcset`.
        assert_eq!(store.public_url(&hash, "a,b 1.jpg"), format!("/_media/{hash}/a%2Cb%201.jpg"));

        for (name, want) in [
            ("a/../b", "a_.._b"),
            ("a\\b", "a_b"),
            ("x\u{1}y", "x_y"),
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
        for bad in [
            "/x/", "x", "a//b/", "//", "./", "../", "a/./b/", "a/../b/", "a b/", "é/", "a\\b/",
            "a?b/", "a%2e/", "a:b/", "a\u{0}b/",
        ] {
            let err = MediaStore::open(with_prefix(backing.config.clone(), bad))
                .err()
                .expect("must fail");
            assert!(matches!(err, MediaError::Config(_)), "{bad}: {err:?}");
        }
        for ok in ["", "site/a/", "Site_1.x-y/b.c/", "..a/", "a../"] {
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

#[test]
fn serve_via_app_path_prefix_rules() {
    for bad in ["", "/", "_media", "/_media/", "/a/b/", "//"] {
        let mut cfg = common::local_backend("").config.clone();
        cfg.public = PublicUrlMode::ServeViaApp {
            path_prefix: bad.into(),
        };
        let err = MediaStore::open(cfg).err().expect("must fail");
        assert!(matches!(err, MediaError::Config(_)), "{bad:?}: {err:?}");
    }
    for ok in ["/_media", "/a/b", "/m"] {
        let mut cfg = common::local_backend("").config.clone();
        cfg.public = PublicUrlMode::ServeViaApp {
            path_prefix: ok.into(),
        };
        MediaStore::open(cfg).unwrap_or_else(|e| panic!("{ok}: {e:?}"));
    }
}

#[test]
fn local_root_that_cannot_be_created_is_an_error() {
    // The local root is an existing regular file: it cannot be a directory.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let mut cfg = common::local_backend("").config.clone();
    cfg.backend = Backend::Local {
        root: file.join("sub"),
    };
    let err = MediaStore::open(cfg).err().expect("must fail");
    assert!(
        matches!(err, MediaError::Io(_) | MediaError::Config(_)),
        "{err:?}"
    );
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

// ---- listing and copying between stores (M4-1 Task 7) ----

async fn listed(store: &ferrite_media::MediaStore) -> Vec<StoredObject> {
    let mut all: Vec<StoredObject> = store.list_artifacts().try_collect().await.unwrap();
    all.sort_by(|a, b| a.key.cmp(&b.key));
    all
}

async fn check_list(backing: common::Backing, prefix: &str) {
    let store = backing.store();
    let mut hashes = Vec::new();
    for i in 0..3u8 {
        let data = Bytes::from(vec![i; 100 + usize::from(i)]);
        let r = store.put_artifact(data).await.unwrap();
        hashes.push((r.hash, r.bytes));
    }
    // Not artifacts: staging leftovers (outside `artifacts/`) are not listed; a stray name
    // inside `artifacts/` is listed with no hash.
    for rel in ["tmp/leftover", "artifacts/zz/not-a-hash"] {
        let path = backing.object_path(&format!("{prefix}{rel}"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"junk").unwrap();
    }
    let all = listed(&store).await;
    assert_eq!(all.len(), 4, "{all:?}");
    let now = Timestamp::now().as_micros();
    for (hash, bytes) in &hashes {
        let o = all
            .iter()
            .find(|o| o.hash.as_ref() == Some(hash))
            .unwrap_or_else(|| panic!("{hash} not listed"));
        assert_eq!(o.size, *bytes);
        assert_eq!(
            o.key,
            format!("{prefix}artifacts/{}/{hash}", &hash.as_str()[..2])
        );
        let age_secs = (now - o.last_modified.as_micros()) / 1_000_000;
        assert!(age_secs.abs() < 300, "{age_secs}");
    }
    let stray = all.iter().find(|o| o.hash.is_none()).unwrap();
    assert_eq!(stray.key, format!("{prefix}artifacts/zz/not-a-hash"));
    assert_eq!(stray.size, 4);
}

#[tokio::test]
async fn list_artifacts_local() {
    check_list(common::local_backend(""), "").await;
}

#[tokio::test]
async fn list_artifacts_local_with_prefix() {
    check_list(common::local_backend("site/a/"), "site/a/").await;
}

#[tokio::test]
async fn list_artifacts_s3() {
    check_list(common::s3_backend("").await, "").await;
}

#[tokio::test]
async fn list_artifacts_s3_with_prefix() {
    check_list(common::s3_backend("site/a/").await, "site/a/").await;
}

#[tokio::test]
async fn list_artifacts_of_an_empty_store_is_empty() {
    let backing = common::local_backend("");
    assert!(listed(&backing.store()).await.is_empty());
}

// ---- the owner marker (M4-1 R16) ----

fn marker(install: &str) -> OwnerMarker {
    OwnerMarker {
        install_id: install.into(),
        site_id: "site".into(),
        claimed_at: "2026-10-29T00:00:00.000000Z".into(),
    }
}

async fn check_owner(backing: common::Backing, prefix: &str) {
    let store = backing.store();
    assert_eq!(store.read_owner().await.unwrap(), None);
    // Only when absent: the first writer wins, a second is told so.
    assert!(store.write_owner(&marker("a"), false).await.unwrap());
    assert!(!store.write_owner(&marker("b"), false).await.unwrap());
    assert_eq!(store.read_owner().await.unwrap(), Some(marker("a")));
    // Replace (a claim) overwrites.
    assert!(store.write_owner(&marker("b"), true).await.unwrap());
    assert_eq!(store.read_owner().await.unwrap(), Some(marker("b")));
    let path = backing.object_path(&format!("{prefix}{OWNER_MARKER}"));
    assert!(path.is_file(), "{}", path.display());
    // Never listed as an artifact or a staging object.
    store
        .put_artifact(Bytes::from_static(b"owned"))
        .await
        .unwrap();
    assert_eq!(listed(&store).await.len(), 1);
    let tmp: Vec<StoredObject> = store.list_tmp().try_collect().await.unwrap();
    assert!(tmp.is_empty(), "{tmp:?}");
    // Something else at its key is an error, not an absent marker.
    std::fs::write(&path, b"not json").unwrap();
    assert!(matches!(
        store.read_owner().await,
        Err(MediaError::Backend(_))
    ));
}

#[tokio::test]
async fn owner_marker_local() {
    check_owner(common::local_backend(""), "").await;
}

#[tokio::test]
async fn owner_marker_local_with_prefix() {
    check_owner(common::local_backend("site/a/"), "site/a/").await;
}

#[tokio::test]
async fn owner_marker_s3() {
    check_owner(common::s3_backend("").await, "").await;
}

#[tokio::test]
async fn owner_marker_s3_with_prefix() {
    check_owner(common::s3_backend("site/a/").await, "site/a/").await;
}

// ---- the `tmp/` listing and `delete_key` (M4-1 Task 6, the artifacts GC) ----

async fn check_tmp(backing: common::Backing, prefix: &str) {
    let store = backing.store();
    // An artifact is not a staging object.
    store
        .put_artifact(Bytes::from_static(b"an artifact"))
        .await
        .unwrap();
    for rel in ["tmp/leftover-a", "tmp/leftover-b"] {
        let path = backing.object_path(&format!("{prefix}{rel}"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"staged").unwrap();
    }
    let mut tmp: Vec<StoredObject> = store.list_tmp().try_collect().await.unwrap();
    tmp.sort_by(|a, b| a.key.cmp(&b.key));
    let keys: Vec<&str> = tmp.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            format!("{prefix}tmp/leftover-a"),
            format!("{prefix}tmp/leftover-b")
        ]
    );
    let now = Timestamp::now().as_micros();
    for o in &tmp {
        assert_eq!(o.hash, None);
        assert_eq!(o.size, 6);
        let age_secs = (now - o.last_modified.as_micros()) / 1_000_000;
        assert!(age_secs.abs() < 300, "{age_secs}");
    }

    // `delete_key` deletes a `tmp/` object, is idempotent, and refuses anything else.
    store.delete_key(&tmp[0].key).await.unwrap();
    store.delete_key(&tmp[0].key).await.unwrap();
    let left: Vec<StoredObject> = store.list_tmp().try_collect().await.unwrap();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].key, tmp[1].key);
    let artifact = listed(&store).await.remove(0);
    for outside in [
        artifact.key.clone(),
        format!("{prefix}tmp"),
        format!("{prefix}tmpx/a"),
        format!("{prefix}tmp/../artifacts/x"),
        "elsewhere/tmp/a".to_owned(),
    ] {
        let err = store.delete_key(&outside).await.unwrap_err();
        assert!(matches!(err, MediaError::Config(_)), "{outside}: {err:?}");
    }
    assert_eq!(listed(&store).await.len(), 1, "the artifact stays");
}

#[tokio::test]
async fn list_tmp_and_delete_key_local() {
    check_tmp(common::local_backend(""), "").await;
}

#[tokio::test]
async fn list_tmp_and_delete_key_local_with_prefix() {
    check_tmp(common::local_backend("site/a/"), "site/a/").await;
}

#[tokio::test]
async fn list_tmp_and_delete_key_s3() {
    check_tmp(common::s3_backend("").await, "").await;
}

#[tokio::test]
async fn list_tmp_and_delete_key_s3_with_prefix() {
    check_tmp(common::s3_backend("site/a/").await, "site/a/").await;
}

async fn check_copy(from: common::Backing, to: common::Backing) {
    let (src, dst) = (from.store(), to.store());
    let small = Bytes::from_static(b"small artifact");
    let big = Bytes::from(vec![7u8; 9 * 1024 * 1024]);
    for data in [small, big] {
        let r = src.put_artifact(data.clone()).await.unwrap();
        assert!(dst.head(&r.hash).await.unwrap().is_none());
        let n = src.copy_to(&dst, &r.hash).await.unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(dst.head(&r.hash).await.unwrap(), Some(n));
        assert_eq!(dst.get(&r.hash).await.unwrap(), data);
    }
}

#[tokio::test]
async fn copy_local_to_s3() {
    check_copy(common::local_backend(""), common::s3_backend("p/").await).await;
}

#[tokio::test]
async fn copy_s3_to_local() {
    check_copy(common::s3_backend("").await, common::local_backend("")).await;
}

#[tokio::test]
async fn copy_of_a_missing_artifact_is_not_found() {
    let (a, b) = (common::local_backend(""), common::local_backend(""));
    let hash = ArtifactHash::parse(&sha_hex(b"nothing")).unwrap();
    let err = a.store().copy_to(&b.store(), &hash).await.unwrap_err();
    assert!(matches!(err, MediaError::NotFound), "{err:?}");
}

#[tokio::test]
async fn copy_of_a_corrupt_source_fails_and_reports_the_mismatch() {
    let (a, b) = (common::local_backend(""), common::local_backend(""));
    let hex = sha_hex(b"expected");
    let path = a.object_path(&format!("artifacts/{}/{hex}", &hex[..2]));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"something else").unwrap();
    let hash = ArtifactHash::parse(&hex).unwrap();
    let err = a.store().copy_to(&b.store(), &hash).await.unwrap_err();
    assert!(matches!(err, MediaError::Backend(_)), "{err:?}");
    assert!(b.store().head(&hash).await.unwrap().is_none());
}

// ---- S3 `copy_if_not_exists` / `conditional_put` ----

#[test]
fn copy_if_not_exists_values() {
    for ok in [
        "multipart",
        " multipart ",
        "header:x-amz-copy-source-if-none-match:*",
        "header: x-goog-if-generation-match : 0",
        "header-with-status:x-ms-blah:*:409",
        "header-with-status:cf-if-none-match:*:412",
    ] {
        assert_eq!(validate_copy_if_not_exists(ok), Ok(()), "{ok}");
    }
    for bad in [
        "",
        "header",
        "header:only-name",
        "header-with-status:a:b",
        "header-with-status:a:b:not-a-status",
        "etag",
        "Multipart",
    ] {
        assert!(validate_copy_if_not_exists(bad).is_err(), "{bad:?}");
    }
}

fn with_options(
    mut cfg: MediaConfig,
    copy: Option<&str>,
    put: Option<ConditionalPut>,
) -> Result<MediaStore, MediaError> {
    let Backend::S3(s3) = &mut cfg.backend else {
        panic!("s3 config expected");
    };
    s3.copy_if_not_exists = copy.map(str::to_owned);
    s3.conditional_put = put;
    MediaStore::open(cfg)
}

#[tokio::test]
async fn an_invalid_copy_if_not_exists_is_a_config_error() {
    let cfg = common::s3_config("http://127.0.0.1:1".into(), "");
    let err = with_options(cfg, Some("nonsense"), Some(ConditionalPut::Etag))
        .err()
        .expect("must fail");
    assert!(matches!(err, MediaError::Config(_)), "{err:?}");
}

// The next two tests show that the options are accepted and the store still works with them.
// s3s-fs gives no request log, so they cannot tell whether the server honoured `multipart` or
// `disabled`, or whether placement fell back to head + copy; that is untested here.
#[tokio::test]
async fn multipart_copy_places_streamed_artifacts() {
    let backing = common::s3_backend("").await;
    let store = with_options(
        backing.config.clone(),
        Some("multipart"),
        Some(ConditionalPut::Etag),
    )
    .expect("open");
    let data = Bytes::from(vec![3u8; 9 * 1024 * 1024]);
    let first = store.put_artifact(data.clone()).await.unwrap();
    assert!(first.created);
    let again = store.put_artifact(data.clone()).await.unwrap();
    assert!(!again.created);
    assert_eq!(store.get(&first.hash).await.unwrap(), data);
}

#[tokio::test]
async fn disabled_conditional_put_still_stores_small_artifacts() {
    let backing = common::s3_backend("").await;
    let store =
        with_options(backing.config.clone(), None, Some(ConditionalPut::Disabled)).expect("open");
    let data = Bytes::from_static(b"no conditional put");
    let first = store.put_artifact(data.clone()).await.unwrap();
    assert!(first.created);
    assert_eq!(store.get(&first.hash).await.unwrap(), data);
    assert!(!store.put_artifact(data).await.unwrap().created);
}
