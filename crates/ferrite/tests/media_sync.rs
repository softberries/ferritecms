//! `ferrite media sync` (M4-1 Task 7): local and S3 stores through the CLI, an in-process
//! S3 server (s3s-fs) standing in for the bucket, and `config check` on the S3 options.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use bytes::Bytes;
use ferrite_domain::{ArtifactHash, Secret};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode, S3Config};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const BUCKET: &str = "ferrite-test";
const ACCESS_KEY: &str = "test-access";
const SECRET_KEY: &str = "test-secret";

/// An s3s-fs server on a free port; dropping it stops the server.
struct S3Server {
    dir: tempfile::TempDir,
    handle: JoinHandle<()>,
    endpoint: String,
}

impl Drop for S3Server {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl S3Server {
    async fn start() -> S3Server {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(BUCKET)).unwrap();
        let fs = s3s_fs::FileSystem::new(dir.path()).unwrap();
        let mut builder = S3ServiceBuilder::new(fs);
        builder.set_auth(SimpleAuth::from_single(ACCESS_KEY, SECRET_KEY));
        let service = builder.build();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let service = service.clone();
                tokio::spawn(async move {
                    let http = ConnBuilder::new(TokioExecutor::new());
                    let _ = http.serve_connection(TokioIo::new(socket), service).await;
                });
            }
        });
        S3Server {
            dir,
            handle,
            endpoint: format!("http://{addr}"),
        }
    }

    /// A store over the bucket at `prefix`, with the test credentials.
    fn store(&self, prefix: &str) -> MediaStore {
        MediaStore::open(MediaConfig {
            backend: Backend::S3(S3Config {
                bucket: BUCKET.into(),
                region: "us-east-1".into(),
                endpoint: Some(self.endpoint.clone()),
                access_key_id: Some(ACCESS_KEY.into()),
                secret_access_key: Some(Secret::new(SECRET_KEY)),
                allow_http: true,
                virtual_hosted_style: false,
                copy_if_not_exists: None,
                conditional_put: None,
            }),
            prefix: prefix.into(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .unwrap()
    }

    /// The environment that points `s3://` URLs at this server (P9).
    fn env(&self) -> [(&'static str, String); 5] {
        [
            ("AWS_ACCESS_KEY_ID", ACCESS_KEY.into()),
            ("AWS_SECRET_ACCESS_KEY", SECRET_KEY.into()),
            ("AWS_REGION", "us-east-1".into()),
            ("AWS_ENDPOINT_URL", self.endpoint.clone()),
            ("AWS_ALLOW_HTTP", "true".into()),
        ]
    }
}

fn local_store(root: &Path) -> MediaStore {
    MediaStore::open(MediaConfig {
        backend: Backend::Local {
            root: root.to_owned(),
        },
        prefix: String::new(),
        public: PublicUrlMode::ServeViaApp {
            path_prefix: "/_media".into(),
        },
    })
    .unwrap()
}

/// Runs `ferrite` with `args` and `env` off the async runtime's threads.
async fn run(args: Vec<String>, env: Vec<(&'static str, String)>) -> Output {
    tokio::task::spawn_blocking(move || {
        let mut cmd: Command = cargo_bin_cmd!("ferrite");
        cmd.env_clear().args(args).envs(env);
        cmd.output().unwrap()
    })
    .await
    .unwrap()
}

fn text(out: &[u8]) -> String {
    String::from_utf8(out.to_vec()).unwrap()
}

fn path_str(p: &Path) -> String {
    p.to_str().unwrap().to_owned()
}

/// Three artifacts: two small and one that takes the streaming path.
async fn seed(store: &MediaStore) -> Vec<(ArtifactHash, u64)> {
    let mut out = Vec::new();
    for data in [
        Bytes::from_static(b"first artifact"),
        Bytes::from_static(b"second artifact, a little longer"),
        Bytes::from(vec![5u8; 9 * 1024 * 1024]),
    ] {
        let r = store.put_artifact(data).await.unwrap();
        out.push((r.hash, r.bytes));
    }
    out
}

fn total(artifacts: &[(ArtifactHash, u64)]) -> u64 {
    artifacts.iter().map(|(_, n)| n).sum()
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sync_local_to_s3_copies_and_skips() {
    let server = S3Server::start().await;
    let src = tempfile::tempdir().unwrap();
    let store = local_store(src.path());
    let artifacts = seed(&store).await;
    // Staging leftovers are never copied.
    std::fs::create_dir_all(src.path().join("tmp")).unwrap();
    std::fs::write(src.path().join("tmp/leftover"), b"x").unwrap();

    let args = |from: &Path| {
        vec![
            "media".to_owned(),
            "sync".to_owned(),
            "--from".to_owned(),
            format!("local:{}", from.display()),
            "--to".to_owned(),
            format!("s3://{BUCKET}/mirror"),
        ]
    };
    let first = run(args(src.path()), server.env().to_vec()).await;
    assert_eq!(first.status.code(), Some(0), "{}", text(&first.stderr));
    assert_eq!(
        text(&first.stdout).trim(),
        format!(
            "copied 3 ({} bytes), skipped 0, failed 0",
            total(&artifacts)
        )
    );
    let dest = server.store("mirror/");
    for (hash, bytes) in &artifacts {
        assert_eq!(dest.head(hash).await.unwrap(), Some(*bytes));
    }
    assert!(
        !server
            .dir
            .path()
            .join(BUCKET)
            .join("mirror/tmp/leftover")
            .exists()
    );

    let second = run(args(src.path()), server.env().to_vec()).await;
    assert_eq!(second.status.code(), Some(0), "{}", text(&second.stderr));
    assert_eq!(
        text(&second.stdout).trim(),
        "copied 0 (0 bytes), skipped 3, failed 0"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sync_s3_to_local_from_data() {
    let server = S3Server::start().await;
    let artifacts = seed(&server.store("site/")).await;

    let data = tempfile::tempdir().unwrap();
    let toml = format!(
        "[site]\nurl = \"http://127.0.0.1:8080\"\n[media]\nbackend = \"s3\"\n[media.s3]\nbucket = \"{BUCKET}\"\nregion = \"us-east-1\"\nendpoint = \"{}\"\naccess_key_id = \"{ACCESS_KEY}\"\nsecret_access_key = \"{SECRET_KEY}\"\nallow_http = true\nprefix = \"site/\"\n",
        server.endpoint
    );
    std::fs::write(data.path().join("ferrite.toml"), toml).unwrap();
    let dest_dir = tempfile::tempdir().unwrap();

    // The site's own S3 settings are used: the environment holds nothing.
    let out = run(
        vec![
            "media".into(),
            "sync".into(),
            "--data".into(),
            path_str(data.path()),
            "--to".into(),
            format!("local:{}", dest_dir.path().display()),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        format!(
            "copied 3 ({} bytes), skipped 0, failed 0",
            total(&artifacts)
        )
    );
    let dest = local_store(dest_dir.path());
    for (hash, bytes) in &artifacts {
        assert_eq!(dest.head(hash).await.unwrap(), Some(*bytes));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sync_partial_failure_exits_2() {
    let src = tempfile::tempdir().unwrap();
    let store = local_store(src.path());
    let good = store
        .put_artifact(Bytes::from_static(b"good artifact"))
        .await
        .unwrap();
    // An object whose bytes do not hash to its name cannot be copied faithfully.
    let hex: String = Sha256::digest(b"expected")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let bad: PathBuf = src.path().join(format!("artifacts/{}/{hex}", &hex[..2]));
    std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
    std::fs::write(&bad, b"corrupt").unwrap();
    let dest_dir = tempfile::tempdir().unwrap();

    let out = run(
        vec![
            "media".into(),
            "sync".into(),
            "--from".into(),
            format!("local:{}", src.path().display()),
            "--to".into(),
            format!("local:{}", dest_dir.path().display()),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        format!("copied 1 ({} bytes), skipped 0, failed 1", good.bytes)
    );
    assert!(text(&out.stderr).contains(&hex), "{}", text(&out.stderr));
    assert_eq!(
        local_store(dest_dir.path()).head(&good.hash).await.unwrap(),
        Some(good.bytes)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sync_rejects_bad_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let to = format!("local:{}", dir.path().display());
    // An unknown URL scheme is an error (exit 1).
    let out = run(
        vec![
            "media".into(),
            "sync".into(),
            "--from".into(),
            "ftp://x/y".into(),
            "--to".into(),
            to.clone(),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("--from"),
        "{}",
        text(&out.stderr)
    );
    // A source is required: either --from or --data.
    let out = run(
        vec!["media".into(), "sync".into(), "--to".into(), to],
        Vec::new(),
    )
    .await;
    // clap's usage error exits 2 (the same code as "some copies failed", see 09 §8).
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("Usage"), "{}", text(&out.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sync_unreachable_destination_exits_1() {
    let src = tempfile::tempdir().unwrap();
    seed(&local_store(src.path())).await;
    let env = vec![
        ("AWS_ACCESS_KEY_ID", ACCESS_KEY.to_owned()),
        ("AWS_SECRET_ACCESS_KEY", SECRET_KEY.to_owned()),
        ("AWS_REGION", "us-east-1".to_owned()),
        ("AWS_ENDPOINT_URL", "http://127.0.0.1:1".to_owned()),
        ("AWS_ALLOW_HTTP", "yes".to_owned()),
    ];
    let out = run(
        vec![
            "media".into(),
            "sync".into(),
            "--from".into(),
            format!("local:{}", src.path().display()),
            "--to".into(),
            format!("s3://{BUCKET}/x"),
        ],
        env,
    )
    .await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("--to"), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sync_local_site_from_data() {
    let artifacts_dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join("ferrite.toml"),
        "[site]\nurl = \"http://127.0.0.1:8080\"\n",
    )
    .unwrap();
    // The default local backend is `media` under the data folder.
    let artifacts = seed(&local_store(&data.path().join("media"))).await;

    let out = run(
        vec![
            "media".into(),
            "sync".into(),
            "--data".into(),
            path_str(data.path()),
            "--to".into(),
            format!("local:{}", artifacts_dir.path().display()),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        format!(
            "copied 3 ({} bytes), skipped 0, failed 0",
            total(&artifacts)
        )
    );
    let dest = local_store(artifacts_dir.path());
    for (hash, bytes) in &artifacts {
        assert_eq!(dest.head(hash).await.unwrap(), Some(*bytes));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sync_s3_to_s3_between_prefixes() {
    let server = S3Server::start().await;
    let artifacts = seed(&server.store("a/")).await;
    let out = run(
        vec![
            "media".into(),
            "sync".into(),
            "--from".into(),
            format!("s3://{BUCKET}/a"),
            "--to".into(),
            format!("s3://{BUCKET}/b/c"),
        ],
        server.env().to_vec(),
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        format!(
            "copied 3 ({} bytes), skipped 0, failed 0",
            total(&artifacts)
        )
    );
    let dest = server.store("b/c/");
    for (hash, bytes) in &artifacts {
        assert_eq!(dest.head(hash).await.unwrap(), Some(*bytes));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sync_ignores_keys_that_are_not_artifacts() {
    let src = tempfile::tempdir().unwrap();
    let store = local_store(src.path());
    store
        .put_artifact(Bytes::from_static(b"real"))
        .await
        .unwrap();
    std::fs::create_dir_all(src.path().join("artifacts/zz")).unwrap();
    std::fs::write(src.path().join("artifacts/zz/stray"), b"x").unwrap();
    let dest = tempfile::tempdir().unwrap();
    let out = run(
        vec![
            "media".into(),
            "sync".into(),
            "--from".into(),
            format!("local:{}", src.path().display()),
            "--to".into(),
            format!("local:{}", dest.path().display()),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        "copied 1 (4 bytes), skipped 0, failed 0"
    );
    assert!(
        text(&out.stderr).contains("ignored 1"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn config_check_warns_multipart() {
    let data = tempfile::tempdir().unwrap();
    let write = |extra: &str| {
        let toml = format!(
            "[site]\nurl = \"http://127.0.0.1:8080\"\n[media]\nbackend = \"s3\"\n[media.s3]\nbucket = \"b\"\nregion = \"r\"\n{extra}"
        );
        std::fs::write(data.path().join("ferrite.toml"), toml).unwrap();
    };
    let check = || {
        let mut cmd: Command = cargo_bin_cmd!("ferrite");
        cmd.env_clear()
            .args(["config", "check", "--data"])
            .arg(data.path());
        cmd.output().unwrap()
    };

    write("copy_if_not_exists = \"multipart\"\n");
    let out = check();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        text(&out.stdout).starts_with("ok:"),
        "{}",
        text(&out.stdout)
    );
    let err = text(&out.stderr);
    assert!(err.contains("warning"), "{err}");
    assert!(err.contains("media.s3.copy_if_not_exists"), "{err}");

    write("copy_if_not_exists = \"header:x-amz-copy-source-if-none-match:*\"\n");
    let out = check();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stderr), "");

    write("copy_if_not_exists = \"nonsense\"\n");
    let out = check();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("media.s3.copy_if_not_exists"),
        "{}",
        text(&out.stderr)
    );
}
