//! Scheduled backups (M4-1 Task 9, P15): the `[backup]` section and the `site.backup` job.
//! On SQLite the job writes a repository snapshot inside `serve` while content is written,
//! and the snapshot restores and serves; on Postgres it uploads export archives to
//! `<to>/exports/` and keeps the newest `keep`. With the schedule off nothing is registered.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use bytes::Bytes;
use ferrite::jobs::{SITE_BACKUP_KIND, SITE_BACKUP_MAX_ATTEMPTS, site_backup};
use ferrite::{NodeOptions, OpenMode, RunnerOverrides, open_store, start};
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_domain::{Artifact, ArtifactKind, Job, JobId, JobStatus, NodeId, Timestamp};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::{LOCK_BACKUP, LOCK_GC, Store};
use ferrite_store_tests::TestDb;
use ferrite_store_tests::fixtures::{fixture_user, new_post};
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn ok(mut cmd: Command) -> Output {
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    out
}

/// `init` + `seed demo` in `<tmp>/<name>`, with `backup` appended to its `ferrite.toml`.
fn seeded(tmp: &Path, name: &str, backup: &str) -> PathBuf {
    let data = tmp.join(name);
    let mut c = ferrite();
    c.args(["init", "--url", "http://127.0.0.1:8080", "--data"])
        .arg(&data);
    ok(c);
    let mut c = ferrite();
    c.args(["seed", "demo", "--data"]).arg(&data);
    ok(c);
    let toml = data.join("ferrite.toml");
    let mut src = std::fs::read_to_string(&toml).unwrap();
    src.push_str(backup);
    std::fs::write(&toml, src).unwrap();
    data
}

fn site_config(data: &Path, extra: &[(&str, &str)]) -> ferrite_config::Config {
    let mut env = vec![(
        "FERRITE_SERVER__LISTEN".to_owned(),
        "127.0.0.1:0".to_owned(),
    )];
    env.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    ferrite_config::load(data, env).unwrap()
}

fn local_media(root: &Path) -> Arc<MediaStore> {
    Arc::new(
        MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: root.to_owned(),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .unwrap(),
    )
}

fn job() -> Job {
    let now = Timestamp::now();
    Job {
        id: JobId::new(),
        kind: SITE_BACKUP_KIND.into(),
        payload: json!({"v": 1}),
        unique_key: None,
        run_at: now,
        priority: 0,
        status: JobStatus::Running,
        attempts: 1,
        max_attempts: SITE_BACKUP_MAX_ATTEMPTS,
        locked_by: None,
        locked_until: None,
        last_error: None,
        created_at: now,
        finished_at: None,
    }
}

/// The complete snapshot names of a repository folder (those with a manifest), oldest first.
fn snapshots(repo: &Path) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(repo.join("snapshots")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = dir
        .map(|e| e.unwrap())
        .filter(|e| e.path().join("manifest.json").is_file())
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// The file names in `dir`, sorted (none when it does not exist).
fn names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    out.sort();
    out
}

async fn sqlite(data: &Path) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", data.join("ferrite.db").display()))
        .await
        .unwrap()
}

async fn done_backups(data: &Path) -> i64 {
    let mut conn = sqlite(data).await;
    let n = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind = 'site.backup' AND status = 'done'",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    let _ = conn.close().await;
    n
}

/// `GET path`: status and body.
async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap();
    (status, body.to_owned())
}

/// The job kinds a running node lists in its registry row.
async fn job_kinds(store: &dyn Store, node: NodeId) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let meta: Value = store
            .read()
            .nodes()
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|n| n.id == node)
            .map(|n| n.meta)
            .unwrap_or(Value::Null);
        if let Some(kinds) = meta["job_kinds"].as_array() {
            return kinds
                .iter()
                .map(|k| k.as_str().unwrap().to_owned())
                .collect();
        }
        assert!(Instant::now() < deadline, "no job_kinds in {meta}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The scheduled job backs a running SQLite site up into its repository while content is
/// written; `keep` prunes; the newest snapshot restores into a new folder, which serves
/// and holds the content.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduled_backup_while_serving_restores() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let data = tokio::task::spawn_blocking(move || {
        seeded(
            &root,
            "site",
            "\n[backup]\nschedule = \"daily\"\nto = \"../repo\"\nkeep = 2\n",
        )
    })
    .await
    .unwrap();
    let repo = tmp.path().join("repo");
    let config = site_config(&data, &[]);
    let store = open_store(&config, OpenMode::MustExist).await.unwrap();
    let author = fixture_user(store.as_ref()).await;
    let marker = new_post(&author, "before-the-backups");
    let tx = store.begin().await.unwrap();
    tx.content().insert(&marker).await.unwrap();
    tx.commit().await.unwrap();

    let opts = NodeOptions {
        periodics: Some(vec![Periodic {
            kind: SITE_BACKUP_KIND,
            every: Duration::from_secs(2),
            payload: json!({"v": 1}),
            max_attempts: SITE_BACKUP_MAX_ATTEMPTS,
        }]),
        scheduler_tick: Some(Duration::from_millis(100)),
        runner: RunnerOverrides {
            idle_poll: Some(Duration::from_millis(100)),
            ..RunnerOverrides::default()
        },
        ..NodeOptions::default()
    };
    let node = start(&config, store.clone(), opts).await.unwrap();
    assert!(
        job_kinds(store.as_ref(), node.node_id)
            .await
            .contains(&SITE_BACKUP_KIND.to_owned())
    );

    // A writer adds posts the whole time the backups run.
    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let writer = {
        let (store, stop, written, author) =
            (store.clone(), stop.clone(), written.clone(), author.clone());
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let n = written.load(Ordering::Relaxed);
                let tx = store.begin().await.unwrap();
                tx.content()
                    .insert(&new_post(&author, &format!("during-{n}")))
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
                written.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while done_backups(&data).await < 3 {
        assert!(Instant::now() < deadline, "three backups did not finish");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop.store(true, Ordering::Relaxed);
    writer.await.unwrap();
    let written = written.load(Ordering::Relaxed);
    node.shutdown().await.unwrap();
    store.close().await;

    // keep = 2: the oldest snapshot was pruned. The local snapshot files are gone.
    let kept = snapshots(&repo);
    assert_eq!(kept.len(), 2, "{kept:?}");
    assert!(repo.join("ferrite-backup-repo.json").is_file());
    assert!(
        names(&data.join("backups"))
            .iter()
            .all(|n| !n.starts_with("backup-")),
        "{:?}",
        names(&data.join("backups"))
    );
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(repo.join("snapshots").join(&kept[1]).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["kind"], "backup");
    assert!(
        manifest["artifacts"]["count"].as_u64().unwrap() > 0,
        "{manifest}"
    );

    // Restore the newest snapshot into a new folder.
    let target = tmp.path().join("restored");
    let (r, t) = (repo.clone(), target.clone());
    let out = tokio::task::spawn_blocking(move || {
        let mut c = ferrite();
        c.arg("restore").arg(&r).arg("--data").arg(&t);
        c.output().unwrap()
    })
    .await
    .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    // R14: the restored ferrite.toml still schedules backups into the same repository.
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("warning: the restored ferrite.toml schedules backups"),
        "{err}"
    );
    assert!(err.contains("FERRITE_BACKUP__SCHEDULE=off"), "{err}");
    assert!(err.contains(repo.to_str().unwrap()), "{err}");
    // With the schedule off in the environment there is no warning.
    let (r, t) = (repo.clone(), tmp.path().join("restored-quiet"));
    let out = tokio::task::spawn_blocking(move || {
        let mut c = ferrite();
        c.env("FERRITE_BACKUP__SCHEDULE", "off");
        c.arg("restore").arg(&r).arg("--data").arg(&t);
        c.output().unwrap()
    })
    .await
    .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("schedules backups"),
        "{}",
        text(&out)
    );

    // It holds the content written before and during the backups.
    let (has_marker, during): (i64, i64) = {
        let mut conn = sqlite(&target).await;
        let row = sqlx::query_as(
            "SELECT (SELECT count(*) FROM content_items WHERE slug = 'before-the-backups'), \
                    (SELECT count(*) FROM content_items WHERE slug LIKE 'during-%')",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        let _ = conn.close().await;
        row
    };
    assert_eq!(has_marker, 1);
    assert!(
        during > 0 && during as u64 <= written,
        "{during} of {written}"
    );

    // And serves (without backing itself up into the same repository).
    let config = site_config(&target, &[("FERRITE_BACKUP__SCHEDULE", "off")]);
    let store = open_store(&config, OpenMode::MustExist).await.unwrap();
    let node = start(&config, store.clone(), NodeOptions::default())
        .await
        .unwrap();
    let (status, body) = get(node.addr, "/").await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Welcome to FerriteCMS"), "{body}");
    node.shutdown().await.unwrap();
    store.close().await;
}

/// `serve` with its own periodics (no test override) schedules `site.backup` when
/// `[backup]` is on: the current period's job row appears, keyed by the day's start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_schedules_site_backup_with_default_periodics() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let data = tokio::task::spawn_blocking(move || {
        seeded(
            &root,
            "site",
            "\n[backup]\nschedule = \"daily\"\nto = \"../repo\"\n",
        )
    })
    .await
    .unwrap();
    let config = site_config(&data, &[]);
    let store = open_store(&config, OpenMode::MustExist).await.unwrap();
    let node = start(&config, store.clone(), NodeOptions::default())
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let key = loop {
        let key: Option<String> = {
            let mut conn = sqlite(&data).await;
            let key = sqlx::query_scalar(
                "SELECT unique_key FROM jobs WHERE kind = 'site.backup' LIMIT 1",
            )
            .fetch_optional(&mut conn)
            .await
            .unwrap();
            let _ = conn.close().await;
            key
        };
        if let Some(key) = key {
            break key;
        }
        assert!(
            Instant::now() < deadline,
            "no site.backup job was scheduled"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    node.shutdown().await.unwrap();
    store.close().await;
    let start_secs: i64 = key
        .strip_prefix("site.backup:")
        .unwrap_or_else(|| panic!("{key}"))
        .parse()
        .unwrap();
    assert_eq!(start_secs % 86_400, 0, "{key}: aligned to 00:00 UTC");
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    assert!(now - start_secs < 86_400 + 60, "{key}");
}

/// R14: pruning a repository spares unlisted blobs younger than a day, so a snapshot still
/// being written into the same repository (another site, another process) keeps the blobs
/// it copied, or found already there, before its manifest exists. Older unlisted blobs go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_spares_blobs_of_a_snapshot_in_flight() {
    use ferrite::backup::repo::Repo;
    use ferrite::backup::target::RepoLocation;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let data = tokio::task::spawn_blocking(move || seeded(&root, "site", ""))
        .await
        .unwrap();
    let repo_dir = tmp.path().join("repo");
    // A complete snapshot of site A.
    let (d, r) = (data.clone(), repo_dir.clone());
    tokio::task::spawn_blocking(move || {
        let mut c = ferrite();
        c.args(["backup", "--data"]).arg(&d).arg("--to").arg(&r);
        ok(c);
    })
    .await
    .unwrap();
    let repo = Repo::open(&RepoLocation::Dir(repo_dir.clone()), false)
        .await
        .unwrap();
    let blob_file = |h: &str| repo_dir.join("artifacts").join(&h[..2]).join(h);
    let age = |path: &Path| {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(2 * 86_400))
            .unwrap();
    };

    // Another site's media: one blob it reuses (already in the repository, old), one it
    // copies now, one abandoned long ago.
    let other = local_media(&tmp.path().join("other-media"));
    let mut hashes = Vec::new();
    for data in ["reused", "in flight", "abandoned"] {
        hashes.push(
            other
                .put_artifact(Bytes::from(data.to_owned()))
                .await
                .unwrap()
                .hash,
        );
    }
    let (reused, in_flight, abandoned) = (&hashes[0], &hashes[1], &hashes[2]);
    repo.copy_blobs(&other, &[reused.clone(), abandoned.clone()])
        .await
        .unwrap();
    age(&blob_file(reused.as_str()));
    age(&blob_file(abandoned.as_str()));
    // The in-flight run: copies `in flight`, finds `reused` present; no manifest yet.
    let copy = repo
        .copy_blobs(&other, &[reused.clone(), in_flight.clone()])
        .await
        .unwrap();
    assert_eq!((copy.written, copy.present), (1, 1));

    // Site A's prune runs before the other run writes its manifest.
    let report = repo.prune(1).await.unwrap();
    assert!(
        blob_file(in_flight.as_str()).is_file(),
        "copied blob spared"
    );
    assert!(blob_file(reused.as_str()).is_file(), "reused blob spared");
    assert!(
        !blob_file(abandoned.as_str()).exists(),
        "old unlisted blob deleted"
    );
    assert_eq!(report.artifacts, 1);
}

/// With `schedule = "off"` (the default) there is no `site.backup` handler or periodic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_off_registers_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let data = tokio::task::spawn_blocking(move || seeded(&root, "site", ""))
        .await
        .unwrap();
    let config = site_config(&data, &[]);
    assert_eq!(config.backup.schedule, ferrite_config::BackupSchedule::Off);
    let store = open_store(&config, OpenMode::MustExist).await.unwrap();
    let media = local_media(&data.join("media"));
    assert!(
        site_backup(&config, store.clone(), media.clone())
            .unwrap()
            .is_none()
    );
    // Daily and weekly give the periodic; the handler's limits are fixed.
    for (schedule, every) in [("daily", 86_400), ("weekly", 604_800)] {
        let on = site_config(
            &data,
            &[
                ("FERRITE_BACKUP__SCHEDULE", schedule),
                ("FERRITE_BACKUP__TO", "../repo"),
            ],
        );
        let (handler, periodic) = site_backup(&on, store.clone(), media.clone())
            .unwrap()
            .expect("scheduled");
        assert_eq!(periodic.kind, SITE_BACKUP_KIND);
        assert_eq!(periodic.every, Duration::from_secs(every));
        assert_eq!(periodic.max_attempts, 3);
        assert_eq!(handler.kind(), "site.backup");
        assert_eq!(handler.timeout(), Duration::from_secs(21_600));
    }
    let node = start(&config, store.clone(), NodeOptions::default())
        .await
        .unwrap();
    let kinds = job_kinds(store.as_ref(), node.node_id).await;
    assert!(kinds.contains(&"artifacts.gc".to_owned()), "{kinds:?}");
    assert!(!kinds.contains(&SITE_BACKUP_KIND.to_owned()), "{kinds:?}");
    node.shutdown().await.unwrap();
    let mut conn = sqlite(&data).await;
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'site.backup'")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    let _ = conn.close().await;
    assert_eq!(jobs, 0);
    store.close().await;
    assert!(!tmp.path().join("repo").exists());
}

/// The handler's outcomes on SQLite: another backup holding the lock is `Busy`; a target
/// that is not a repository is `Retry`; a blob missing from the media store still
/// completes (R10); a repository inside the data folder is refused at setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn site_backup_job_outcomes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let data = tokio::task::spawn_blocking(move || {
        seeded(
            &root,
            "site",
            "\n[backup]\nschedule = \"daily\"\nto = \"../repo\"\n",
        )
    })
    .await
    .unwrap();
    let repo = tmp.path().join("repo");
    let config = site_config(&data, &[]);
    let store = open_store(&config, OpenMode::MustExist).await.unwrap();
    let media = local_media(&data.join("media"));
    let (handler, _) = site_backup(&config, store.clone(), media.clone())
        .unwrap()
        .unwrap();
    let handler = handler.with_gc_wait(Duration::from_millis(300));

    let held = store.try_lock(LOCK_BACKUP).await.unwrap().expect("free");
    let r = handler.run(&job()).await;
    assert!(matches!(r, Err(JobError::Busy(_))), "{r:?}");
    held.release().await.unwrap();
    assert!(!repo.exists(), "nothing written while busy");

    // R17: a GC pass holding `artifacts.gc` past the wait makes the run Busy (it never
    // copies blobs while a GC may delete them), and nothing is written.
    let gc = store.try_lock(LOCK_GC).await.unwrap().expect("free");
    let r = handler.run(&job()).await;
    assert!(
        matches!(&r, Err(JobError::Busy(why)) if why.contains("artifacts.gc")),
        "{r:?}"
    );
    gc.release().await.unwrap();
    assert!(!repo.exists(), "nothing written while the GC runs");

    // A folder that is not a repository: retried, never written into.
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("notes.txt"), "mine").unwrap();
    let r = handler.run(&job()).await;
    assert!(matches!(r, Err(JobError::Retry(_))), "{r:?}");
    assert_eq!(names(&repo), ["notes.txt"]);
    std::fs::remove_file(repo.join("notes.txt")).unwrap();

    // A blob missing from the media store: the backup completes and lists it.
    let hash = media
        .put_artifact(Bytes::from_static(b"soon gone"))
        .await
        .unwrap()
        .hash;
    store
        .read()
        .artifacts()
        .insert_if_absent(&Artifact {
            hash: hash.clone(),
            kind: ArtifactKind::Media,
            bytes: 9,
            mime: "application/octet-stream".into(),
            created_at: Timestamp::now(),
            orphaned_at: None,
        })
        .await
        .unwrap();
    media.delete(&hash).await.unwrap();
    handler.run(&job()).await.unwrap();
    let snaps = snapshots(&repo);
    assert_eq!(snaps.len(), 1);
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(repo.join("snapshots").join(&snaps[0]).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["missing"], json!([hash.to_string()]), "{manifest}");

    // A repository inside the data folder (here through a symlink, which the config check
    // cannot see) is refused when the job is set up.
    let link = tmp.path().join("link");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&data, &link).unwrap();
        let via_link = site_config(
            &data,
            &[("FERRITE_BACKUP__TO", link.join("repo").to_str().unwrap())],
        );
        let err = site_backup(&via_link, store.clone(), media.clone())
            .err()
            .expect("refused");
        assert!(
            format!("{err:#}").contains("inside the data folder"),
            "{err:#}"
        );
    }
    store.close().await;
}

/// On Postgres the job uploads export archives to `<to>/exports/` (a marked repository),
/// deletes the local temporary file and keeps the newest `keep` (only its own names).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduled_export_on_postgres_writes_and_prunes() {
    let Some(db) = TestDb::postgres().await else {
        return;
    };
    let (url, schema) = db.pg_location().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("pg");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("ferrite.toml"),
        format!(
            "[site]\nurl = \"http://127.0.0.1:8080/\"\n[database]\nurl = \"{url}\"\n\
             schema = \"{schema}\"\n[backup]\nschedule = \"daily\"\nto = \"../repo\"\nkeep = 2\n"
        ),
    )
    .unwrap();
    let config = site_config(&data, &[]);
    let store = db.shared_store();
    let media = local_media(&data.join("media"));
    let blob = media
        .put_artifact(Bytes::from_static(b"exported blob"))
        .await
        .unwrap();
    store
        .read()
        .artifacts()
        .insert_if_absent(&Artifact {
            hash: blob.hash.clone(),
            kind: ArtifactKind::Media,
            bytes: 13,
            mime: "application/octet-stream".into(),
            created_at: Timestamp::now(),
            orphaned_at: None,
        })
        .await
        .unwrap();
    let (handler, periodic) = site_backup(&config, store.clone(), media.clone())
        .unwrap()
        .unwrap();
    assert_eq!(periodic.every, Duration::from_secs(86_400));

    // A file of the operator's in exports/ is never pruned.
    for _ in 0..3 {
        handler.run(&job()).await.unwrap();
    }
    std::fs::write(repo.join("exports").join("notes.txt"), "mine").unwrap();
    handler.run(&job()).await.unwrap();

    assert!(repo.join("ferrite-backup-repo.json").is_file());
    let exports = names(&repo.join("exports"));
    assert_eq!(exports.len(), 3, "{exports:?}");
    assert!(exports.contains(&"notes.txt".to_owned()));
    let archives: Vec<&String> = exports
        .iter()
        .filter(|n| n.ends_with(".ferrite.tar.zst"))
        .collect();
    assert_eq!(archives.len(), 2, "{exports:?}");
    // Nothing left locally.
    assert!(
        names(&data.join("backups"))
            .iter()
            .all(|n| !n.starts_with("export-")),
        "{:?}",
        names(&data.join("backups"))
    );
    let newest = repo.join("exports").join(archives[1]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&newest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let bytes = std::fs::read(&newest).unwrap();
    let decoder = zstd::stream::read::Decoder::new(&bytes[..]).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let mut paths = Vec::new();
    let mut manifest = Value::Null;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = String::from_utf8(entry.path_bytes().into_owned()).unwrap();
        if path == "manifest.json" {
            manifest = serde_json::from_reader(&mut entry).unwrap();
        }
        paths.push(path);
    }
    assert_eq!(manifest["kind"], "export", "{manifest}");
    assert_eq!(manifest["source_backend"], "postgres");
    assert_eq!(manifest["artifacts"]["count"], 1);
    let h = blob.hash.as_str();
    assert!(
        paths.contains(&format!("artifacts/{}/{h}", &h[..2])),
        "{paths:?}"
    );
    db.teardown().await;
}
