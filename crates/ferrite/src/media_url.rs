//! `local:<path>` / `s3://bucket[/prefix]` store arguments (`ferrite media sync`, later the
//! other media tools).

use std::path::PathBuf;

use anyhow::{Context, bail};
use ferrite_config::InitMedia;
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode, S3Config};

/// Opens the store a URL names.
///
/// - `local:<path>`: files under `<path>`; the folder is created when `create` is set,
///   otherwise it must exist.
/// - `s3://bucket[/prefix]`: credentials, endpoint and options come from the environment
///   (`AmazonS3Builder::from_env`: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`,
///   `AWS_ENDPOINT_URL`, `AWS_ALLOW_HTTP`, `AWS_COPY_IF_NOT_EXISTS`, …), never from
///   `ferrite.toml`.
///
/// The store is not probed.
pub(crate) fn parse_media_url(url: &str, create: bool) -> anyhow::Result<MediaStore> {
    let backend_and_prefix = if let Some(path) = url.strip_prefix("local:") {
        if path.is_empty() {
            bail!("expected local:<path>");
        }
        let root = PathBuf::from(path);
        if !create && !root.is_dir() {
            bail!("{} is not a folder", root.display());
        }
        (Backend::Local { root }, String::new())
    } else if url.starts_with("s3://") {
        let InitMedia::S3 { bucket, prefix } =
            InitMedia::parse_s3_url(url).map_err(anyhow::Error::msg)?
        else {
            unreachable!("parse_s3_url returns the S3 variant");
        };
        (Backend::S3(s3_from_env(bucket)), prefix)
    } else {
        bail!("expected local:<path> or s3://bucket[/prefix], got {url:?}");
    };
    let (backend, prefix) = backend_and_prefix;
    MediaStore::open(MediaConfig {
        backend,
        prefix,
        public: PublicUrlMode::ServeViaApp {
            path_prefix: "/_media".to_owned(),
        },
    })
    .context("cannot open the media store")
}

/// The settings the builder would not take from the environment itself: `build_s3` always
/// sets region and addressing style, so those are read here. `AWS_ALLOW_HTTP` is read here
/// too, although `build_s3` now keeps the environment's client options (it only turns
/// plain HTTP on, never off). The credentials, the endpoint and the placement options stay
/// unset and so come from the environment.
fn s3_from_env(bucket: String) -> S3Config {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let flag = |name: &str| {
        var(name).is_some_and(|v| {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes" | "y"
            )
        })
    };
    S3Config {
        bucket,
        region: var("AWS_REGION")
            .or_else(|| var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|| "us-east-1".to_owned()),
        endpoint: None,
        access_key_id: None,
        secret_access_key: None,
        allow_http: flag("AWS_ALLOW_HTTP"),
        virtual_hosted_style: flag("AWS_VIRTUAL_HOSTED_STYLE_REQUEST"),
        copy_if_not_exists: None,
        conditional_put: None,
    }
}
