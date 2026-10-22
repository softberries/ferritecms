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
