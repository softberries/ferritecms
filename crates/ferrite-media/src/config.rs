use std::path::PathBuf;

use ferrite_domain::Secret;

#[derive(Debug, Clone)]
pub enum Backend {
    Local { root: PathBuf },
    S3(S3Config),
}

#[derive(Debug, Clone)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<Secret>,
    pub allow_http: bool,
    pub virtual_hosted_style: bool,
    /// How the store places a staged upload at its final key, in object_store's encoding:
    /// `header:<name>:<value>`, `header-with-status:<name>:<value>:<status>` or `multipart`
    /// (see [`crate::validate_copy_if_not_exists`]). `None` leaves the builder's setting
    /// (the `AWS_COPY_IF_NOT_EXISTS` variable on the environment path) and, without one,
    /// placement takes the head-then-copy fallback.
    pub copy_if_not_exists: Option<String>,
    pub conditional_put: ConditionalPut,
}

/// Whether the S3 backend creates objects with `If-None-Match` (conditional put).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConditionalPut {
    /// `If-None-Match: *` on create; the builder's default.
    #[default]
    Etag,
    /// For stores that reject the header: a plain put after the existence check.
    Disabled,
}

#[derive(Debug, Clone)]
pub enum PublicUrlMode {
    /// Served by the app under `path_prefix` (for example `/_media`).
    ServeViaApp { path_prefix: String },
    /// Served directly from a CDN or bucket URL.
    Direct { base_url: url::Url },
}

#[derive(Debug, Clone)]
pub struct MediaConfig {
    pub backend: Backend,
    /// Empty, or ends with `/` and never starts with `/`.
    pub prefix: String,
    pub public: PublicUrlMode,
}
