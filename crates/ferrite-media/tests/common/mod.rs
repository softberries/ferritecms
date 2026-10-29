use ferrite_domain::Secret;
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode, S3Config};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const BUCKET: &str = "ferrite-test";
const ACCESS_KEY: &str = "test-access";
const SECRET_KEY: &str = "test-secret";

/// A running s3s-fs server plus the temp dir backing it. Dropping aborts the server.
pub struct Backing {
    _dir: tempfile::TempDir,
    server: Option<JoinHandle<()>>,
    pub config: MediaConfig,
    /// Directory under which object keys appear as plain files.
    objects_root: std::path::PathBuf,
}

impl Drop for Backing {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

impl Backing {
    /// Filesystem path of an object key (local root, or the s3s-fs bucket directory).
    pub fn object_path(&self, key: &str) -> std::path::PathBuf {
        self.objects_root.join(key)
    }

    /// Everything the backend keeps on disk, including in-flight multipart state.
    pub fn storage_root(&self) -> &std::path::Path {
        self._dir.path()
    }

    pub fn store(&self) -> MediaStore {
        MediaStore::open(self.config.clone()).expect("open store")
    }
}

pub fn public_mode() -> PublicUrlMode {
    PublicUrlMode::ServeViaApp {
        path_prefix: "/_media".into(),
    }
}

pub fn s3_config(endpoint: String, prefix: &str) -> MediaConfig {
    MediaConfig {
        backend: Backend::S3(S3Config {
            bucket: BUCKET.into(),
            region: "us-east-1".into(),
            endpoint: Some(endpoint),
            access_key_id: Some(ACCESS_KEY.into()),
            secret_access_key: Some(Secret::new(SECRET_KEY)),
            allow_http: true,
            virtual_hosted_style: false,
            copy_if_not_exists: None,
            conditional_put: None,
        }),
        prefix: prefix.into(),
        public: public_mode(),
    }
}

pub fn local_backend(prefix: &str) -> Backing {
    let dir = tempfile::tempdir().expect("tempdir");
    let objects_root = dir.path().join("media");
    let config = MediaConfig {
        backend: Backend::Local {
            root: objects_root.clone(),
        },
        prefix: prefix.into(),
        public: public_mode(),
    };
    Backing {
        _dir: dir,
        server: None,
        config,
        objects_root,
    }
}

pub async fn s3_backend(prefix: &str) -> Backing {
    let dir = tempfile::tempdir().expect("tempdir");
    // s3s-fs maps buckets to directories under its root.
    std::fs::create_dir_all(dir.path().join(BUCKET)).expect("bucket dir");
    let fs = s3s_fs::FileSystem::new(dir.path()).expect("s3s-fs");
    let mut builder = S3ServiceBuilder::new(fs);
    builder.set_auth(SimpleAuth::from_single(ACCESS_KEY, SECRET_KEY));
    let service = builder.build();

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
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
    let objects_root = dir.path().join(BUCKET);
    Backing {
        _dir: dir,
        objects_root,
        server: Some(server),
        config: s3_config(format!("http://{addr}"), prefix),
    }
}

/// Generates `mod <name> { local(); s3(); }` running the body against both backends.
/// The body receives `$backing: common::Backing`.
#[macro_export]
macro_rules! backend_tests {
    ($( $name:ident ( $backing:ident ) $body:block )+) => {
        $(
            mod $name {
                #[allow(unused_imports)]
                use super::*;

                #[tokio::test]
                async fn local() {
                    let $backing = $crate::common::local_backend("");
                    $body
                }

                #[tokio::test]
                async fn s3() {
                    let $backing = $crate::common::s3_backend("").await;
                    $body
                }
            }
        )+
    };
}
