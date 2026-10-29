//! `media.variants` jobs (M2a-2b P8, F17): an upload recorded through `MediaService`
//! gets its sizes from the node's runner; the node's image slots bound processing; a
//! stale job writes nothing; an undecodable, oversized or panicking image ends the job
//! without breaking the worker or the original's URL.

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use ferrite::jobs::ImageProcessor;
use ferrite_core::{
    MediaService, NewUpload,
    jobs::{MEDIA_VARIANTS_VERSION, MediaVariants, media_variants_job},
};
use ferrite_domain::{ContentId, JobId};
use ferrite_media::{VariantSpec, process_image};

use super::*;

/// How long one run of the real processor on a 2000×1200 PNG may take in an
/// unoptimised build on a shared runner.
const PROCESS: Duration = Duration::from_secs(90);

/// A `w`×`h` RGB PNG (a gradient, so it compresses but is not flat).
fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
    });
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn services(store: &Arc<dyn Store>) -> MediaService {
    let rules: Arc<PermalinkRules> = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    MediaService::new(store.clone(), content, rules)
}

/// Stores `data` and records it as a published upload named `name`; `raster` enqueues
/// its variants job in the upload's `Tx`.
async fn record(
    site: &Site,
    store: &Arc<dyn Store>,
    actor: &Actor,
    data: Vec<u8>,
    name: &str,
    raster: bool,
) -> (ContentId, ArtifactHash) {
    let stored = site.media().put_artifact(Bytes::from(data)).await.unwrap();
    let artifact = Artifact {
        hash: stored.hash.clone(),
        kind: ArtifactKind::Media,
        bytes: i64::try_from(stored.bytes).unwrap(),
        mime: stored.mime.clone(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    let item = services(store)
        .record_upload(
            actor,
            NewUpload {
                artifact,
                mime: stored.mime,
                bytes: stored.bytes,
                original_name: name.into(),
                title: None,
                alt: String::new(),
                sanitized: false,
                raster,
            },
        )
        .await
        .unwrap();
    (item.item.id, stored.hash)
}

/// Enqueues the variants job of `item` for `artifact` (due now) and returns its id.
async fn variants_job(store: &Arc<dyn Store>, item: ContentId, artifact: &ArtifactHash) -> JobId {
    store
        .read()
        .jobs()
        .enqueue(&media_variants_job(&MediaVariants {
            v: MEDIA_VARIANTS_VERSION,
            item,
            artifact: artifact.to_string(),
        }))
        .await
        .unwrap()
        .unwrap()
}

async fn job(store: &Arc<dyn Store>, id: JobId) -> Job {
    store.read().jobs().get(id).await.unwrap().unwrap()
}

async fn media_row(store: &Arc<dyn Store>, item: ContentId) -> ferrite_domain::Media {
    store.read().media().get(item).await.unwrap().unwrap()
}

/// A processor seam counting calls and the most calls in flight at once, optionally
/// held at a gate until [`Gate::open`].
#[derive(Default)]
struct Gate {
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    closed: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn closed() -> Arc<Self> {
        let gate = Self::default();
        *gate.closed.lock().unwrap() = true;
        Arc::new(gate)
    }

    fn open(&self) {
        *self.closed.lock().unwrap() = false;
        self.opened.notify_all();
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The real processor behind the gate.
    fn processor(self: &Arc<Self>) -> ImageProcessor {
        let gate = self.clone();
        Arc::new(move |data: &[u8], specs: &[VariantSpec]| {
            gate.calls.fetch_add(1, Ordering::SeqCst);
            let now = gate.active.fetch_add(1, Ordering::SeqCst) + 1;
            gate.max_active.fetch_max(now, Ordering::SeqCst);
            let mut closed = gate.closed.lock().unwrap();
            while *closed {
                closed = gate.opened.wait(closed).unwrap();
            }
            drop(closed);
            let out = process_image(data, specs);
            gate.active.fetch_sub(1, Ordering::SeqCst);
            out
        })
    }
}

/// Opens the gate when dropped, so a failing test never leaves a blocking thread
/// parked (the runtime would wait for it).
struct OpenOnDrop(Arc<Gate>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

fn opts(processor: Option<ImageProcessor>) -> NodeOptions {
    NodeOptions {
        periodics: Some(Vec::new()),
        image_processor: processor,
        ..NodeOptions::default()
    }
}

/// A 2000×1200 PNG recorded through `MediaService`: the upload's job runs on the node
/// and records the core sizes plus the blog theme's (`card`, `hero`), the original's
/// dimensions, and variant files (artifacts of kind `variant`) that `/_media` serves.
async fn upload_gets_variants_through_the_runner(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site.start(&db, opts(None)).await;
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let (item, _) = record(&site, &store, &actor, png(2000, 1200), "wide.png", true).await;
    eventually(PROCESS, "variants recorded", || {
        let store = store.clone();
        async move { media_row(&store, item).await.width.is_some() }
    })
    .await;
    let row = media_row(&store, item).await;
    assert_eq!((row.width, row.height), (Some(2000), Some(1200)));
    let variants = row.variants.as_object().unwrap();
    let mut names: Vec<&str> = variants.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["card", "hero", "large", "medium", "thumbnail"]);
    let dims = |name: &str| (variants[name]["w"].clone(), variants[name]["h"].clone());
    assert_eq!(dims("thumbnail"), (json!(150), json!(150)));
    assert_eq!(dims("medium"), (json!(768), json!(461)));
    assert_eq!(dims("large"), (json!(1600), json!(960)));
    assert_eq!(dims("card"), (json!(640), json!(360)));
    assert_eq!(dims("hero"), (json!(1280), json!(640)));
    for (name, v) in variants {
        assert_eq!(v["mime"], "image/png", "{name}");
        let hash = ArtifactHash::parse(v["hash"].as_str().unwrap()).unwrap();
        let stored = store.read().artifacts().get(&hash).await.unwrap().unwrap();
        assert_eq!(stored.kind, ArtifactKind::Variant, "{name}");
        let (status, body) = get(node.addr, &format!("/_media/{hash}/wide.png")).await;
        assert_eq!(status, 200, "{name}");
        assert!(!body.is_empty(), "{name}");
    }
    assert_eq!(
        counts(store.as_ref()).await.get(&JobStatus::Done),
        Some(&1),
        "one variants job, done"
    );
    node.shutdown().await.unwrap();
}

/// F17 / M1b R4: with `media.image_workers = 1` (the default) a second job finds the
/// node's only image slot taken while the first decodes, is put back `Busy` (attempt
/// refunded, due again later) instead of decoding beside it or holding a worker, and
/// runs once the slot is free. The processor never runs twice at once.
async fn image_workers_bound_processing(db: Arc<TestDb>) {
    let site = Site::new(&db);
    assert_eq!(site.config.uploads.image_workers, 1);
    assert!(
        site.config.cluster.workers >= 2,
        "a second worker can claim"
    );
    let gate = Gate::closed();
    let _open = OpenOnDrop(gate.clone());
    let node = site.start(&db, opts(Some(gate.processor()))).await;
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let (first, _) = record(&site, &store, &actor, png(300, 200), "one.png", true).await;
    eventually(Duration::from_secs(10), "first job processing", || async {
        gate.calls() == 1
    })
    .await;
    // The second job is enqueued by hand so the test can watch its row.
    let (second, hash) = record(&site, &store, &actor, png(320, 200), "two.png", false).await;
    let id = variants_job(&store, second, &hash).await;
    eventually(Duration::from_secs(10), "second job released busy", || {
        let store = store.clone();
        async move {
            let j = job(&store, id).await;
            let backoff = j.run_at.as_micros() - j.created_at.as_micros();
            j.status == JobStatus::Queued && j.attempts == 0 && backoff >= 900_000
        }
    })
    .await;
    assert_eq!(gate.calls(), 1, "no second decode while the slot is taken");
    gate.open();
    // The runner marks the job done after the handler recorded the row, so wait for both.
    eventually(
        Duration::from_secs(30),
        "both recorded, second job done",
        || {
            let store = store.clone();
            async move {
                media_row(&store, first).await.width.is_some()
                    && media_row(&store, second).await.width.is_some()
                    && job(&store, id).await.status == JobStatus::Done
            }
        },
    )
    .await;
    let j = job(&store, id).await;
    assert_eq!(j.attempts, 1, "the busy release refunded its attempt");
    assert_eq!(gate.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(gate.calls(), 2);
    node.shutdown().await.unwrap();
}

/// P8: a job for an original the row no longer holds is done without decoding, and
/// `record_variants` for an old artifact writes nothing.
async fn replaced_file_drops_stale_variants(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let gate = Arc::new(Gate::default());
    let node = site.start(&db, opts(Some(gate.processor()))).await;
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    // The job names an original the row no longer holds: the row's file was replaced
    // after the upload (`media` rows keep their artifact through `update`, so the
    // replaced state is the row holding `current` while the job names `old`).
    let (item, current) = record(&site, &store, &actor, png(300, 200), "current.png", false).await;
    let (_, old) = record(&site, &store, &actor, png(310, 200), "old.png", false).await;
    assert_eq!(media_row(&store, item).await.artifact_hash, current);
    let id = variants_job(&store, item, &old).await;
    eventually(Duration::from_secs(10), "stale job done", || {
        let store = store.clone();
        async move { job(&store, id).await.status == JobStatus::Done }
    })
    .await;
    assert_eq!(gate.calls(), 0, "nothing decoded for a stale job");
    let before = media_row(&store, item).await;
    assert_eq!(before.variants, json!({}));
    assert_eq!(before.width, None);

    let recorded = services(&store)
        .record_variants(item, &old, 300, 200, json!({"thumbnail": {}}), &[])
        .await
        .unwrap();
    assert!(!recorded, "the row holds another original");
    assert_eq!(media_row(&store, item).await, before);
    node.shutdown().await.unwrap();
}

/// An image that does not decode ends its job `dead` at the first attempt with the
/// reason in `last_error`; the original stays served and the worker keeps running
/// jobs.
async fn undecodable_image_is_permanent(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site.start(&db, opts(None)).await;
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    // PNG magic, then garbage: sniffed (and stored) as `image/png`.
    let mut corrupt = png(64, 64);
    corrupt.truncate(40);
    corrupt.extend((0..4000u32).map(|i| (i * 7 % 251) as u8));
    let (item, hash) = record(&site, &store, &actor, corrupt, "broken.png", false).await;
    assert_eq!(media_row(&store, item).await.mime, "image/png");
    let id = variants_job(&store, item, &hash).await;
    eventually(Duration::from_secs(10), "job dead", || {
        let store = store.clone();
        async move { job(&store, id).await.status == JobStatus::Dead }
    })
    .await;
    let j = job(&store, id).await;
    assert_eq!(j.attempts, 1, "permanent: no retries");
    let error = j.last_error.unwrap();
    assert!(error.contains("image"), "{error}");
    let row = media_row(&store, item).await;
    assert_eq!((row.width, row.variants.clone()), (None, json!({})));
    let (status, _) = get(node.addr, &format!("/_media/{hash}/broken.png")).await;
    assert_eq!(status, 200, "the original is still served");

    // The worker survived: a good image after it is processed.
    let (good, _) = record(&site, &store, &actor, png(200, 100), "good.png", true).await;
    eventually(Duration::from_secs(30), "next job processed", || {
        let store = store.clone();
        async move { media_row(&store, good).await.width == Some(200) }
    })
    .await;
    node.shutdown().await.unwrap();
}

/// Task 4 carry (P8): an original over 64 MiB is stored but gets no variants: its job
/// is done without reading or decoding it.
async fn oversized_original_is_skipped(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let gate = Arc::new(Gate::default());
    let node = site.start(&db, opts(Some(gate.processor()))).await;
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let mut big = png(16, 16);
    big.resize(
        usize::try_from(ferrite_http::media::MAX_PROCESS_BYTES).unwrap() + 1,
        0,
    );
    let (item, _) = record(&site, &store, &actor, big, "huge.png", true).await;
    eventually(Duration::from_secs(10), "job done", || async {
        counts(store.as_ref()).await.get(&JobStatus::Done) == Some(&1)
    })
    .await;
    assert_eq!(gate.calls(), 0);
    let row = media_row(&store, item).await;
    assert_eq!((row.width, row.variants), (None, json!({})));
    node.shutdown().await.unwrap();
}

/// A processor that panics (a decoder bug) ends that job `dead`; the blocking pool and
/// the job worker carry on, and the image slot is free again.
async fn processing_panic_is_permanent(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let processor: ImageProcessor = Arc::new(move |data: &[u8], specs: &[VariantSpec]| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("decoder bug");
        }
        process_image(data, specs)
    });
    let node = site.start(&db, opts(Some(processor))).await;
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let (item, hash) = record(&site, &store, &actor, png(200, 100), "p.png", false).await;
    let id = variants_job(&store, item, &hash).await;
    eventually(Duration::from_secs(10), "job dead", || {
        let store = store.clone();
        async move { job(&store, id).await.status == JobStatus::Dead }
    })
    .await;
    let j = job(&store, id).await;
    assert_eq!(j.attempts, 1);
    assert!(j.last_error.unwrap().contains("panicked"));
    let (next, _) = record(&site, &store, &actor, png(210, 100), "q.png", true).await;
    eventually(Duration::from_secs(30), "next job processed", || {
        let store = store.clone();
        async move { media_row(&store, next).await.width == Some(210) }
    })
    .await;
    node.shutdown().await.unwrap();
}

/// F17 (review M2): the runner drops `run` mid-decode (lease loss, timeout, shutdown);
/// the blocking decode carries on and keeps the image slot until it ends, so another job
/// is `Busy` meanwhile and the slot frees itself afterwards.
async fn dropped_run_keeps_the_slot_until_the_decode_ends(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let (item, hash) = record(&site, &store, &actor, png(200, 100), "d.png", false).await;
    let queued = job(&store, variants_job(&store, item, &hash).await).await;
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let gate = Gate::closed();
    let _open = OpenOnDrop(gate.clone());
    let handler = Arc::new(
        ferrite::jobs::MediaVariantsHandler::new(
            store.clone(),
            site.media(),
            Arc::new(services(&store)),
            slots.clone(),
            Arc::new(ferrite_media::default_variants),
        )
        .with_processor(gate.processor()),
    );
    let (h, j) = (handler.clone(), queued.clone());
    let running = tokio::spawn(async move { h.run(&j).await });
    eventually(Duration::from_secs(10), "decode started", || async {
        gate.calls() == 1
    })
    .await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert_eq!(
        slots.available_permits(),
        0,
        "the decode still holds the slot"
    );
    assert!(matches!(handler.run(&queued).await, Err(JobError::Busy(_))));
    gate.open();
    eventually(Duration::from_secs(10), "slot released", || async {
        slots.available_permits() == 1
    })
    .await;
    assert_eq!(gate.calls(), 1);
    assert_eq!(
        media_row(&store, item).await.width,
        None,
        "dropped: nothing recorded"
    );
}

/// The handler's own outcomes, without a node: a payload it cannot read, a bad hash
/// and a missing item.
async fn variants_payload_outcomes(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let store = db.shared_store();
    let handler = ferrite::jobs::MediaVariantsHandler::new(
        store.clone(),
        site.media(),
        Arc::new(services(&store)),
        Arc::new(tokio::sync::Semaphore::new(1)),
        Arc::new(ferrite_media::default_variants),
    );
    let run = |payload: Value| {
        let store = store.clone();
        let handler = &handler;
        async move {
            let id = store
                .read()
                .jobs()
                .enqueue(&NewJob::new("media.variants", payload))
                .await
                .unwrap()
                .unwrap();
            let job = store.read().jobs().get(id).await.unwrap().unwrap();
            handler.run(&job).await
        }
    };
    let item = ContentId::new();
    assert!(matches!(
        run(json!({"v": 1, "item": "nope"})).await,
        Err(JobError::Permanent(_))
    ));
    assert!(matches!(
        run(json!({"v": 1, "item": item.to_string(), "artifact": "../x"})).await,
        Err(JobError::Permanent(_))
    ));
    // No media row: nothing to do.
    assert!(
        run(json!({"v": 1, "item": item.to_string(), "artifact": "ab".repeat(32)}))
            .await
            .is_ok()
    );
}

backend_tests_spawning!(
    upload_gets_variants_through_the_runner,
    image_workers_bound_processing,
    replaced_file_drops_stale_variants,
    undecodable_image_is_permanent,
    oversized_original_is_skipped,
    processing_panic_is_permanent,
    dropped_run_keeps_the_slot_until_the_decode_ends,
    variants_payload_outcomes,
);
