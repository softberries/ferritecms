#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferrite_media::{Backend, MediaConfig, MediaError, MediaStore, PublicUrlMode, S3Config};

/// Own test binary with a single test: it mutates the process environment.
#[test]
fn s3_builder_errors_are_config_errors() {
    // With no explicit credentials the builder reads the environment; an unparsable value
    // makes `AmazonS3Builder::build` fail.
    // SAFETY: this binary contains exactly one test and starts no threads of its own.
    unsafe { std::env::set_var("AWS_CONDITIONAL_PUT", "definitely-not-a-mode") };
    let cfg = MediaConfig {
        backend: Backend::S3(S3Config {
            bucket: "b".into(),
            region: "us-east-1".into(),
            endpoint: Some("http://127.0.0.1:1".into()),
            access_key_id: None,
            secret_access_key: None,
            allow_http: true,
            virtual_hosted_style: false,
        }),
        prefix: String::new(),
        public: PublicUrlMode::ServeViaApp {
            path_prefix: "/_media".into(),
        },
    };
    let err = MediaStore::open(cfg).err().expect("must fail");
    assert!(matches!(err, MediaError::Config(_)), "{err:?}");
}
