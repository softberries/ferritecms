//! The artifacts GC (M4-1 Task 6, P10–P12): `gc::run` on both backends with a local media
//! store (the store-level listing is tested on local and S3 in `ferrite-media`), the
//! `artifacts.gc` handler, and `ferrite media gc` through the CLI.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use assert_cmd::cargo::cargo_bin_cmd;
use bytes::Bytes;
use ferrite::gc::{self, GcOptions, GcReport};
use ferrite::jobs::{ARTIFACTS_GC_KIND, ArtifactsGcHandler};
use ferrite_cluster::{JobError, JobHandler};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, InstalledPlugin, InstalledTheme, Job, JobId, JobStatus,
    Media, NewJob, NodeId, Timestamp,
};
use ferrite_media::{Backend, MediaConfig, MediaStore, OwnerMarker, PublicUrlMode};
use ferrite_store::{LOCK_GC, Store};
use ferrite_store_tests::fixtures::{fixture_user, new_post};
use ferrite_store_tests::import::new_run;
use ferrite_store_tests::{TestDb, backend_tests_spawning};
use serde_json::json;

const DAY: Duration = Duration::from_secs(86_400);
const HOUR: Duration = Duration::from_secs(3_600);
const WEEK: Duration = Duration::from_secs(7 * 86_400);

/// A local media store in its own folder.
struct Media0 {
    dir: tempfile::TempDir,
    store: MediaStore,
}

fn media() -> Media0 {
    let dir = tempfile::tempdir().unwrap();
    let store = MediaStore::open(MediaConfig {
        backend: Backend::Local {
            root: dir.path().to_owned(),
        },
        prefix: String::new(),
        public: PublicUrlMode::ServeViaApp {
            path_prefix: "/_media".into(),
        },
    })
    .unwrap();
    Media0 { dir, store }
}

fn later(t: Timestamp, by: Duration) -> Timestamp {
    Timestamp::from_micros(t.as_micros() + i64::try_from(by.as_micros()).unwrap()).unwrap()
}

/// An hour from now: every blob a test writes is older than "now" by then.
fn future() -> Timestamp {
    later(Timestamp::now(), HOUR)
}

fn opts(now: Timestamp, grace: Duration) -> GcOptions {
    GcOptions {
        grace,
        dry_run: false,
        now,
        claim: false,
    }
}

/// One pass with the store already owned by `store`'s install (R19: a pass that finds no
/// owner marker deletes nothing, so the tests of the deleting steps start owned).
async fn run(store: &dyn Store, media: &MediaStore, opts: &GcOptions) -> GcReport {
    if media.read_owner().await.unwrap().is_none() {
        let marker = OwnerMarker {
            install_id: gc::install_id(store).await.unwrap(),
            site_id: "test".into(),
            claimed_at: "2026-10-29T00:00:00.000000Z".into(),
        };
        assert!(media.write_owner(&marker, false).await.unwrap());
    }
    raw_run(store, media, opts).await
}

/// One pass as it comes, owner marker or not.
async fn raw_run(store: &dyn Store, media: &MediaStore, opts: &GcOptions) -> GcReport {
    gc::run(store, media, opts)
        .await
        .unwrap()
        .expect("the GC lock is free")
}

/// Stores `data` as a blob only (no row).
async fn blob(media: &MediaStore, data: &str) -> ArtifactHash {
    media
        .put_artifact(Bytes::from(data.to_owned()))
        .await
        .unwrap()
        .hash
}

/// Stores `data` as a blob and records its row of `kind`.
async fn artifact(
    store: &dyn Store,
    media: &MediaStore,
    data: &str,
    kind: ArtifactKind,
) -> Artifact {
    let hash = blob(media, data).await;
    let row = Artifact {
        hash,
        kind,
        bytes: data.len() as i64,
        mime: "application/octet-stream".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    assert!(
        store
            .read()
            .artifacts()
            .insert_if_absent(&row)
            .await
            .unwrap()
    );
    row
}

async fn row(store: &dyn Store, hash: &ArtifactHash) -> Option<Artifact> {
    store.read().artifacts().get(hash).await.unwrap()
}

async fn has_blob(media: &MediaStore, hash: &ArtifactHash) -> bool {
    media.head(hash).await.unwrap().is_some()
}

/// Writes a staging object `tmp/<name>`.
fn tmp_object(m: &Media0, name: &str) {
    let path = m.dir.path().join("tmp").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"staged").unwrap();
}

fn tmp_exists(m: &Media0, name: &str) -> bool {
    m.dir.path().join("tmp").join(name).exists()
}

/// Every P10 reference kind keeps its row and blob through two passes with grace 0; the
/// one unreferenced row goes.
async fn gc_keeps_every_reference_kind(db: Arc<TestDb>) {
    let store = db.store();
    let m = media();
    let media = &m.store;
    let author = fixture_user(store).await;

    // Media original and its variant.
    let original = artifact(store, media, "original", ArtifactKind::Media).await;
    let variant = artifact(store, media, "variant", ArtifactKind::Variant).await;
    let mut item = new_post(&author, "photo");
    item.kind = "media".into();
    let tx = store.begin().await.unwrap();
    tx.content().insert(&item).await.unwrap();
    tx.commit().await.unwrap();
    store
        .read()
        .media()
        .insert(&Media {
            item_id: item.id,
            artifact_hash: original.hash.clone(),
            mime: "image/png".into(),
            bytes: 8,
            width: None,
            height: None,
            alt: String::new(),
            original_name: "p.png".into(),
            variants: json!({"thumb": {"hash": variant.hash.as_str(), "w": 1, "h": 1}}),
            sanitized: false,
        })
        .await
        .unwrap();
    // A plugin and a theme.
    let plugin = artifact(store, media, "plugin", ArtifactKind::Plugin).await;
    store
        .read()
        .plugins()
        .upsert(&InstalledPlugin {
            name: "p".into(),
            version: "1.0.0".into(),
            artifact_hash: plugin.hash.clone(),
            manifest: json!({"name": "p"}),
            granted: json!({}),
            enabled: true,
            disabled_reason: None,
            installed_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        })
        .await
        .unwrap();
    let theme = artifact(store, media, "theme", ArtifactKind::Theme).await;
    store
        .read()
        .themes()
        .upsert(&InstalledTheme {
            name: "t".into(),
            version: "1.0.0".into(),
            artifact_hash: theme.hash.clone(),
            manifest: json!({"name": "t"}),
            active: false,
            installed_at: Timestamp::now(),
        })
        .await
        .unwrap();
    // An import report: the manifest names the JSON and HTML files.
    let json_file = artifact(store, media, "{\"report\":1}", ArtifactKind::Import).await;
    let html_file = artifact(store, media, "<p>report</p>", ArtifactKind::Import).await;
    let manifest =
        json!({"v": 1, "json": json_file.hash.as_str(), "html": html_file.hash.as_str()})
            .to_string();
    let manifest = artifact(store, media, &manifest, ArtifactKind::Import).await;
    let run_row = new_run(author.id, "https://wp.example");
    store.read().imports().create_run(&run_row).await.unwrap();
    store
        .read()
        .imports()
        .set_report(run_row.id, manifest.hash.as_str())
        .await
        .unwrap();
    // A queued job (an import's inputs, one of them with no row) and a running job that
    // names a hash deep in its payload.
    let wxr = artifact(store, media, "<rss/>", ArtifactKind::Import).await;
    let unrecorded = blob(media, "uploads zip, not recorded yet").await;
    store
        .read()
        .jobs()
        .enqueue(&NewJob::new(
            "import.wordpress",
            json!({"v": 1, "wxr": wxr.hash.as_str(), "uploads": unrecorded.as_str()}),
        ))
        .await
        .unwrap();
    let nested = artifact(store, media, "nested", ArtifactKind::Variant).await;
    store
        .read()
        .jobs()
        .enqueue(&NewJob::new(
            "gc.test-running",
            json!({"v": 1, "deep": {"list": [1, {"x": nested.hash.as_str()}]}}),
        ))
        .await
        .unwrap();
    let claimed = store
        .read()
        .jobs()
        .claim(NodeId::new(), &["gc.test-running".into()], 1, HOUR)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    // The control: nothing names it.
    let orphan = artifact(store, media, "orphan", ArtifactKind::Plugin).await;

    let t0 = future();
    let first = run(store, media, &opts(t0, Duration::ZERO)).await;
    assert_eq!(first.marked, 1, "{first:?}");
    assert_eq!(first.deleted_rows, 0, "{first:?}");
    assert_eq!(first.deleted_blobs, 0, "{first:?}");
    let second = run(
        store,
        media,
        &opts(later(t0, Duration::from_secs(1)), Duration::ZERO),
    )
    .await;
    assert_eq!(
        second,
        GcReport {
            deleted_rows: 1,
            deleted_row_bytes: 6,
            ..GcReport::default()
        }
    );
    assert_eq!(row(store, &orphan.hash).await, None);
    assert!(!has_blob(media, &orphan.hash).await);

    for kept in [
        &original, &variant, &plugin, &theme, &json_file, &html_file, &manifest, &wxr, &nested,
    ] {
        let r = row(store, &kept.hash).await.expect("row kept");
        assert_eq!(r.orphaned_at, None, "{:?}", kept.kind);
        assert!(has_blob(media, &kept.hash).await, "{:?}", kept.kind);
    }
    assert!(has_blob(media, &unrecorded).await, "a job's row-less input");
}

/// P10's fallback: while any report manifest cannot be read (its blob missing, over
/// 1 KiB, or not `{"v":1,"json":…,"html":…}`), every `import`-kind row is kept, even one
/// nothing names; other orphans still go. Once the manifest reads, the import orphan goes.
async fn gc_unreadable_manifest_keeps_every_import_row(db: Arc<TestDb>) {
    let store = db.store();
    let m = media();
    let media = &m.store;
    let author = fixture_user(store).await;
    let run_row = new_run(author.id, "https://wp.example");
    store.read().imports().create_run(&run_row).await.unwrap();
    let loose = artifact(
        store,
        media,
        "an import file nothing names",
        ArtifactKind::Import,
    )
    .await;

    let missing = "e".repeat(64);
    let pad = "x".repeat(1100);
    let oversize =
        json!({"v": 1, "json": "a".repeat(64), "html": "b".repeat(64), "pad": pad}).to_string();
    let oversize = artifact(store, media, &oversize, ArtifactKind::Import).await;
    let wrong_shape = artifact(store, media, "{\"v\":1}", ArtifactKind::Import).await;
    let mut t = future();
    for (case, manifest) in [
        ("missing", missing.as_str()),
        ("oversize", oversize.hash.as_str()),
        ("wrong shape", wrong_shape.hash.as_str()),
    ] {
        store
            .read()
            .imports()
            .set_report(run_row.id, manifest)
            .await
            .unwrap();
        let other = artifact(
            store,
            media,
            &format!("other orphan {case}"),
            ArtifactKind::Media,
        )
        .await;
        for _ in 0..2 {
            t = later(t, Duration::from_secs(1));
            run(store, media, &opts(t, Duration::ZERO)).await;
        }
        let kept = row(store, &loose.hash).await.expect(case);
        assert_eq!(kept.orphaned_at, None, "{case}");
        assert!(has_blob(media, &loose.hash).await, "{case}");
        assert_eq!(
            row(store, &other.hash).await,
            None,
            "{case}: other orphans go"
        );
        assert!(!has_blob(media, &other.hash).await, "{case}");
    }

    // A readable manifest: the fallback is off and the loose import row goes.
    let json_file = artifact(store, media, "{}", ArtifactKind::Import).await;
    let html_file = artifact(store, media, "<p></p>", ArtifactKind::Import).await;
    let good = json!({"v": 1, "json": json_file.hash.as_str(), "html": html_file.hash.as_str()})
        .to_string();
    let good = artifact(store, media, &good, ArtifactKind::Import).await;
    store
        .read()
        .imports()
        .set_report(run_row.id, good.hash.as_str())
        .await
        .unwrap();
    for _ in 0..2 {
        t = later(t, Duration::from_secs(1));
        run(store, media, &opts(t, Duration::ZERO)).await;
    }
    assert_eq!(row(store, &loose.hash).await, None);
    assert!(!has_blob(media, &loose.hash).await);
    for kept in [&json_file, &html_file, &good] {
        assert!(row(store, &kept.hash).await.is_some());
        assert!(has_blob(media, &kept.hash).await);
    }
}

/// A row nothing names is marked, survives the grace period, then goes: its row first,
/// then its blob.
async fn gc_deletes_orphan_row_then_blob_after_grace(db: Arc<TestDb>) {
    let store = db.store();
    let m = media();
    let orphan = artifact(store, &m.store, "orphan bytes", ArtifactKind::Media).await;

    let t0 = future();
    let first = run(store, &m.store, &opts(t0, WEEK)).await;
    assert_eq!(first.marked, 1, "{first:?}");
    assert_eq!(
        row(store, &orphan.hash).await.unwrap().orphaned_at,
        Some(t0)
    );

    let within = run(store, &m.store, &opts(later(t0, 6 * DAY), WEEK)).await;
    assert_eq!(within, GcReport::default());
    assert!(row(store, &orphan.hash).await.is_some());
    assert!(has_blob(&m.store, &orphan.hash).await);

    let after = run(store, &m.store, &opts(later(t0, 8 * DAY), WEEK)).await;
    assert_eq!(
        after,
        GcReport {
            deleted_rows: 1,
            deleted_row_bytes: 12,
            ..GcReport::default()
        }
    );
    assert_eq!(row(store, &orphan.hash).await, None);
    assert!(!has_blob(&m.store, &orphan.hash).await);
}

/// Recording the same bytes again clears the mark: the grace period starts over.
async fn gc_reinsert_clears_mark(db: Arc<TestDb>) {
    let store = db.store();
    let m = media();
    let orphan = artifact(store, &m.store, "reused bytes", ArtifactKind::Plugin).await;
    let t0 = future();
    run(store, &m.store, &opts(t0, WEEK)).await;
    assert_eq!(
        row(store, &orphan.hash).await.unwrap().orphaned_at,
        Some(t0)
    );

    let reinserted = store
        .read()
        .artifacts()
        .insert_if_absent(&orphan)
        .await
        .unwrap();
    assert!(!reinserted);
    assert_eq!(row(store, &orphan.hash).await.unwrap().orphaned_at, None);

    let t1 = later(t0, 8 * DAY);
    let report = run(store, &m.store, &opts(t1, WEEK)).await;
    assert_eq!(
        report,
        GcReport {
            marked: 1,
            ..GcReport::default()
        }
    );
    assert_eq!(
        row(store, &orphan.hash).await.unwrap().orphaned_at,
        Some(t1)
    );
    assert!(has_blob(&m.store, &orphan.hash).await);
}

/// A blob with no row goes once it is older than the grace period; a protected one stays.
async fn gc_unrecorded_blob_young_kept_old_deleted(db: Arc<TestDb>) {
    let store = db.store();
    let m = media();
    let stray = blob(&m.store, "an upload whose row never came").await;
    let named = blob(&m.store, "a queued job's input").await;
    store
        .read()
        .jobs()
        .enqueue(&NewJob::new(
            "media.variants",
            json!({"v": 1, "item": "x", "artifact": named.as_str()}),
        ))
        .await
        .unwrap();

    let young = run(store, &m.store, &opts(Timestamp::now(), WEEK)).await;
    assert_eq!(young, GcReport::default());
    assert!(has_blob(&m.store, &stray).await);

    let old = run(
        store,
        &m.store,
        &opts(later(Timestamp::now(), 8 * DAY), WEEK),
    )
    .await;
    assert_eq!(
        old,
        GcReport {
            deleted_blobs: 1,
            deleted_blob_bytes: 30,
            ..GcReport::default()
        }
    );
    assert!(!has_blob(&m.store, &stray).await);
    assert!(has_blob(&m.store, &named).await, "protected by the job");
}

/// `tmp/` objects (probes included) go after 24 h, whatever the grace period.
async fn gc_tmp_older_than_a_day_deleted(db: Arc<TestDb>) {
    let store = db.store();
    let m = media();
    tmp_object(&m, "0192-staged");
    tmp_object(&m, "probe-0192");

    let fresh = run(store, &m.store, &opts(later(Timestamp::now(), HOUR), WEEK)).await;
    assert_eq!(fresh.deleted_tmp, 0, "{fresh:?}");
    assert!(tmp_exists(&m, "0192-staged"));

    let stale = run(
        store,
        &m.store,
        &opts(later(Timestamp::now(), 25 * HOUR), WEEK),
    )
    .await;
    assert_eq!(
        stale,
        GcReport {
            deleted_tmp: 2,
            ..GcReport::default()
        }
    );
    assert!(!tmp_exists(&m, "0192-staged"));
    assert!(!tmp_exists(&m, "probe-0192"));
}

/// A dry run reports every class and changes nothing.
async fn gc_dry_run_changes_nothing(db: Arc<TestDb>) {
    let store = db.store();
    let m = media();
    let marked_long_ago = artifact(store, &m.store, "marked long ago", ArtifactKind::Media).await;
    let t0 = future();
    run(store, &m.store, &opts(t0, WEEK)).await;
    let unmarked = artifact(store, &m.store, "not marked yet", ArtifactKind::Media).await;
    let stray = blob(&m.store, "stray").await;
    tmp_object(&m, "leftover");

    let t1 = later(t0, 8 * DAY);
    let report = gc::run(
        store,
        &m.store,
        &GcOptions {
            grace: WEEK,
            dry_run: true,
            now: t1,
            claim: false,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        report,
        GcReport {
            marked: 1,
            unmarked: 0,
            deleted_rows: 1,
            deleted_row_bytes: 15,
            deleted_blobs: 1,
            deleted_blob_bytes: 5,
            deleted_tmp: 1,
            failed: 0,
            deletes_skipped: None,
            claimed_from: None,
        }
    );
    assert_eq!(
        row(store, &marked_long_ago.hash).await.unwrap().orphaned_at,
        Some(t0)
    );
    assert_eq!(row(store, &unmarked.hash).await.unwrap().orphaned_at, None);
    for hash in [&marked_long_ago.hash, &unmarked.hash, &stray] {
        assert!(has_blob(&m.store, hash).await);
    }
    assert!(tmp_exists(&m, "leftover"));
}

/// A second database (`B`) of the same backend, for the ownership tests.
async fn second(db: &TestDb) -> TestDb {
    if db.is_sqlite() {
        TestDb::sqlite().await
    } else {
        TestDb::postgres().await.expect("postgres is configured")
    }
}

/// R16: two sites (two databases) on one media store, as a restored or staging copy is.
/// Site A's first pass writes the owner marker and deletes nothing (R19); its next pass
/// reads it and may delete. Site B then marks its rows and cleans `tmp/` but deletes
/// nothing (not A's row-less blobs, not its own expired rows) and says whose store it is.
/// A dry-run claim writes nothing and names the owner it would replace; `claim` takes the
/// store over, after which B deletes and A no longer does.
async fn gc_second_site_on_one_store_deletes_nothing(db: Arc<TestDb>) {
    let a = db.store();
    let other = second(&db).await;
    let b = other.store();
    let m = media();
    let install_a = gc::install_id(a).await.unwrap();
    let install_b = gc::install_id(b).await.unwrap();
    assert_ne!(install_a, install_b);

    let a_row = artifact(a, &m.store, "uploaded by site A", ArtifactKind::Media).await;
    let b_row = artifact(b, &m.store, "uploaded by site B", ArtifactKind::Media).await;
    let t0 = future();
    let first = raw_run(a, &m.store, &opts(t0, WEEK)).await;
    assert!(
        first
            .deletes_skipped
            .as_deref()
            .is_some_and(|w| w.contains("wrote ferrite-owner.json")),
        "{first:?}"
    );
    let marker = m.store.read_owner().await.unwrap().expect("marker written");
    assert_eq!(marker.install_id, install_a);
    assert!(m.dir.path().join("ferrite-owner.json").is_file());
    let owned = raw_run(a, &m.store, &opts(t0, WEEK)).await;
    assert_eq!(
        owned.deletes_skipped, None,
        "the next pass reads its marker"
    );

    // B, a month later: A's row-less (for B) blob and B's own expired row both stay.
    tmp_object(&m, "leftover");
    let month = later(t0, 30 * DAY);
    raw_run(b, &m.store, &opts(t0, DAY)).await;
    let skipped = raw_run(b, &m.store, &opts(month, DAY)).await;
    assert!(
        skipped
            .deletes_skipped
            .as_deref()
            .is_some_and(|w| w.contains(&install_a)),
        "{skipped:?}"
    );
    assert_eq!(
        (skipped.deleted_rows, skipped.deleted_blobs),
        (0, 0),
        "{skipped:?}"
    );
    assert!(
        skipped
            .to_string()
            .contains("deletes skipped: the media store belongs to install"),
        "{skipped}"
    );
    assert!(has_blob(&m.store, &a_row.hash).await);
    assert!(has_blob(&m.store, &b_row.hash).await);
    assert!(row(b, &b_row.hash).await.is_some());
    assert!(!tmp_exists(&m, "leftover"), "tmp/ is still cleaned");

    // A dry-run claim reports what B could delete, names A, and writes no marker.
    let dry = gc::run(
        b,
        &m.store,
        &GcOptions {
            grace: DAY,
            dry_run: true,
            now: month,
            claim: true,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(dry.deletes_skipped, None);
    assert!(
        dry.claimed_from
            .as_deref()
            .is_some_and(|f| f.contains(&install_a)),
        "{dry:?}"
    );
    assert_eq!((dry.deleted_rows, dry.deleted_blobs), (1, 1), "{dry:?}");
    assert_eq!(
        m.store.read_owner().await.unwrap().unwrap().install_id,
        install_a
    );

    // The claim: B owns the store and deletes; A no longer does.
    let claimed = raw_run(
        b,
        &m.store,
        &GcOptions {
            claim: true,
            ..opts(month, DAY)
        },
    )
    .await;
    assert_eq!(claimed.deletes_skipped, None);
    assert!(
        claimed
            .to_string()
            .contains(&format!("claimed the media store from install {install_a}")),
        "{claimed}"
    );
    assert_eq!(
        (claimed.deleted_rows, claimed.deleted_blobs),
        (1, 1),
        "{claimed:?}"
    );
    assert_eq!(
        m.store.read_owner().await.unwrap().unwrap().install_id,
        install_b
    );
    assert!(!has_blob(&m.store, &a_row.hash).await);
    assert!(!has_blob(&m.store, &b_row.hash).await);
    let after = raw_run(a, &m.store, &opts(later(month, DAY), DAY)).await;
    assert!(
        after
            .deletes_skipped
            .as_deref()
            .is_some_and(|w| w.contains(&install_b)),
        "{after:?}"
    );
    assert!(
        row(a, &a_row.hash).await.is_some(),
        "A's row is not deleted"
    );
    other.teardown().await;
}

/// R19 (N1): two installs that both find no owner marker, on a store whose conditional
/// create is not honoured (simulated by removing A's marker before B's first pass, which a
/// `conditional_put = "disabled"` head-then-put race allows), both write one and neither
/// deletes in that pass; then the install whose marker stands (B, the last writer)
/// deletes from its next pass, and A never does.
async fn gc_first_passes_without_a_marker_delete_nothing(db: Arc<TestDb>) {
    let a = db.store();
    let other = second(&db).await;
    let b = other.store();
    let m = media();
    let stray = blob(&m.store, "row-less for both").await;
    let now = later(future(), 30 * DAY);

    let first_a = raw_run(a, &m.store, &opts(now, DAY)).await;
    std::fs::remove_file(m.dir.path().join("ferrite-owner.json")).unwrap();
    let first_b = raw_run(b, &m.store, &opts(now, DAY)).await;
    for r in [&first_a, &first_b] {
        assert!(r.deletes_skipped.is_some(), "{r:?}");
        assert_eq!((r.deleted_rows, r.deleted_blobs), (0, 0), "{r:?}");
    }
    assert!(has_blob(&m.store, &stray).await);

    for _ in 0..2 {
        let again = raw_run(a, &m.store, &opts(now, DAY)).await;
        assert!(again.deletes_skipped.is_some(), "{again:?}");
        assert_eq!(again.deleted_blobs, 0, "{again:?}");
        assert!(has_blob(&m.store, &stray).await);
    }
    let owner = raw_run(b, &m.store, &opts(now, DAY)).await;
    assert_eq!(owner.deletes_skipped, None);
    assert_eq!(owner.deleted_blobs, 1, "{owner:?}");
    assert!(!has_blob(&m.store, &stray).await);
    other.teardown().await;
}

/// R19 (N6): a marker that cannot be read skips the deletes, with the reason; `--claim`
/// rewrites it and names what it replaced, and that pass deletes once it reads the claim
/// back. N4: a dry run on a database without an install id stores none and writes no
/// marker.
async fn gc_unreadable_marker_skips_deletes(db: Arc<TestDb>) {
    let store = db.store();
    let m = media();
    let stray = blob(&m.store, "row-less").await;
    let now = later(future(), 30 * DAY);
    std::fs::write(m.dir.path().join("ferrite-owner.json"), b"{not json").unwrap();
    let skipped = raw_run(store, &m.store, &opts(now, DAY)).await;
    assert!(
        skipped
            .deletes_skipped
            .as_deref()
            .is_some_and(|w| w.contains("cannot be read")),
        "{skipped:?}"
    );
    assert_eq!(skipped.deleted_blobs, 0);
    assert!(has_blob(&m.store, &stray).await);
    let claimed = raw_run(
        store,
        &m.store,
        &GcOptions {
            claim: true,
            ..opts(now, DAY)
        },
    )
    .await;
    assert!(
        claimed
            .claimed_from
            .as_deref()
            .is_some_and(|f| f.contains("unreadable")),
        "{claimed:?}"
    );
    assert_eq!(claimed.deleted_blobs, 1, "{claimed:?}");

    let other = second(&db).await;
    other
        .exec_raw("DELETE FROM schema_meta WHERE key = 'install_id'")
        .await;
    let fresh = media();
    let dry = gc::run(
        other.store(),
        &fresh.store,
        &GcOptions {
            dry_run: true,
            ..opts(now, DAY)
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(dry.deletes_skipped.is_some(), "{dry:?}");
    assert_eq!(
        other
            .store()
            .read()
            .schema_meta()
            .get("install_id")
            .await
            .unwrap(),
        None
    );
    assert_eq!(fresh.store.read_owner().await.unwrap(), None);
    other.teardown().await;
}

fn gc_job() -> Job {
    let now = Timestamp::now();
    Job {
        id: JobId::new(),
        kind: ARTIFACTS_GC_KIND.into(),
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

/// One GC at a time: while the lock is held a run is refused (`None`) and the job is
/// `Busy`; once it is free both run.
async fn gc_concurrent_runs_one_busy(db: Arc<TestDb>) {
    let store = db.shared_store();
    let m = media();
    let media_store = Arc::new(
        MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: m.dir.path().to_owned(),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .unwrap(),
    );
    let handler = ArtifactsGcHandler::new(store.clone(), media_store.clone(), WEEK);
    assert_eq!(handler.kind(), "artifacts.gc");
    assert_eq!(handler.timeout(), Duration::from_secs(3_600));

    let held = store.try_lock(LOCK_GC).await.unwrap().expect("lock free");
    let refused = gc::run(store.as_ref(), &m.store, &opts(Timestamp::now(), WEEK))
        .await
        .unwrap();
    assert_eq!(refused, None);
    let job = handler.run(&gc_job()).await;
    assert!(matches!(job, Err(JobError::Busy(_))), "{job:?}");
    held.release().await.unwrap();

    // Two at once: each either runs or is refused, and at least one runs.
    let now = opts(Timestamp::now(), WEEK);
    let (a, b) = tokio::join!(
        gc::run(store.as_ref(), &m.store, &now),
        gc::run(store.as_ref(), &media_store, &now),
    );
    let ran = [a.unwrap(), b.unwrap()]
        .iter()
        .filter(|r| r.is_some())
        .count();
    assert!(ran >= 1);
    handler.run(&gc_job()).await.unwrap();
}

backend_tests_spawning!(
    gc_keeps_every_reference_kind,
    gc_unreadable_manifest_keeps_every_import_row,
    gc_deletes_orphan_row_then_blob_after_grace,
    gc_reinsert_clears_mark,
    gc_unrecorded_blob_young_kept_old_deleted,
    gc_tmp_older_than_a_day_deleted,
    gc_dry_run_changes_nothing,
    gc_concurrent_runs_one_busy,
    gc_second_site_on_one_store_deletes_nothing,
    gc_first_passes_without_a_marker_delete_nothing,
    gc_unreadable_marker_skips_deletes,
);

// ---- `ferrite media gc` ----

const SITE_URL: &str = "http://127.0.0.1:8080";
/// The cluster site key (64 hex characters).
const SITE_KEY: &str = "abababababababababababababababababababababababababababababababab";

async fn ferrite(args: Vec<String>, env: Vec<(String, String)>) -> Output {
    tokio::task::spawn_blocking(move || {
        let mut cmd = cargo_bin_cmd!("ferrite");
        cmd.env_clear().args(args).envs(env);
        cmd.output().unwrap()
    })
    .await
    .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn args(list: &[&str], data: &Path) -> Vec<String> {
    let mut out: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
    out.push("--data".into());
    out.push(data.to_str().unwrap().to_owned());
    out
}

const EMPTY_LINE: &str = "marked 0, unmarked 0, deleted rows 0 (0 bytes), deleted unrecorded blobs 0 (0 bytes), deleted tmp 0";

/// SQLite: 0 with the counts line (dry run too), 1 on an error, 2 on a bad flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn media_gc_cli_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let init = ferrite(args(&["init", "--url", SITE_URL], data), Vec::new()).await;
    assert_eq!(init.status.code(), Some(0), "{}", text(&init));
    // A staging leftover from long ago goes; a fresh one stays.
    let media_root = data.join("media");
    std::fs::create_dir_all(media_root.join("tmp")).unwrap();
    std::fs::write(media_root.join("tmp/fresh"), b"x").unwrap();
    std::fs::write(media_root.join("tmp/old"), b"x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(media_root.join("tmp/old"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - 2 * DAY)
        .unwrap();

    // A dry run on a store without an owner marker writes none, says why it would delete
    // nothing, and logs no error: nothing is wrong with a store no pass has claimed yet.
    let out = ferrite(args(&["media", "gc", "--dry-run"], data), Vec::new()).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains("; deletes skipped: the media store has no owner marker yet"),
        "{}",
        text(&out)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("ERROR"),
        "{}",
        text(&out)
    );
    assert!(!media_root.join("ferrite-owner.json").exists());

    let out = ferrite(args(&["media", "gc"], data), Vec::new()).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    // The first pass writes the owner marker and deletes nothing but tmp/ (R19).
    let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    assert!(
        line.starts_with(&EMPTY_LINE.replace("deleted tmp 0", "deleted tmp 1"))
            && line.contains("; deletes skipped: the media store had no owner marker"),
        "{line}"
    );
    assert!(media_root.join("ferrite-owner.json").is_file());
    assert!(media_root.join("tmp/fresh").exists());
    assert!(!media_root.join("tmp/old").exists());

    let out = ferrite(
        args(&["media", "gc", "--dry-run", "--grace-days", "1"], data),
        Vec::new(),
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("dry run: {EMPTY_LINE}")
    );

    for bad in ["0", "366"] {
        let out = ferrite(
            args(&["media", "gc", "--grace-days", bad], data),
            Vec::new(),
        )
        .await;
        assert_eq!(out.status.code(), Some(2), "{bad}: {}", text(&out));
    }

    let missing = data.join("nowhere");
    let out = ferrite(args(&["media", "gc"], &missing), Vec::new()).await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
}

/// Postgres: another GC holding the lock is exit 3.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn media_gc_cli_busy_exit_3() {
    let Some(db) = TestDb::postgres().await else {
        return;
    };
    let (url, schema) = db.pg_location().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!(
            "[site]\nurl = \"{SITE_URL}/\"\n[server]\nlisten = \"127.0.0.1:0\"\n\
             [database]\nurl = \"{url}\"\nschema = \"{schema}\"\n\
             [media]\nlocal_shared = true\n[cluster]\nmode = \"cluster\"\n"
        ),
    )
    .unwrap();
    let env = vec![("FERRITE_AUTH__SECRET_KEY".to_owned(), SITE_KEY.to_owned())];

    let held = db
        .store()
        .try_lock(LOCK_GC)
        .await
        .unwrap()
        .expect("lock free");
    let out = ferrite(args(&["media", "gc"], dir.path()), env.clone()).await;
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    held.release().await.unwrap();

    let out = ferrite(args(&["media", "gc"], dir.path()), env.clone()).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .starts_with(&format!("{EMPTY_LINE}; deletes skipped:")),
        "{}",
        text(&out)
    );
    let out = ferrite(args(&["media", "gc"], dir.path()), env).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), EMPTY_LINE);
    db.teardown().await;
}

/// `serve` schedules `artifacts.gc` daily (its handler is in the job kinds list, which
/// `tests/node.rs` `job_kinds_are_in_meta` checks on a running node).
#[test]
fn serve_registers_artifacts_gc() {
    let periodics = ferrite::jobs::core_periodics();
    let gc = periodics
        .iter()
        .find(|p| p.kind == ARTIFACTS_GC_KIND)
        .expect("artifacts.gc is scheduled");
    assert_eq!(gc.every, DAY);
    for kind in ["content.publish_due", "plugin.purge_sweep"] {
        assert!(periodics.iter().any(|p| p.kind == kind), "{kind}");
    }
}
