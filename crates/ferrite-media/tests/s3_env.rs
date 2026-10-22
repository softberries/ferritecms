#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use ferrite_media::{Backend, MediaError, MediaStore};

/// Own test binary with a single test: it mutates the process environment.
#[test]
fn s3_builder_errors_are_config_errors() {
    // With no explicit credentials the builder reads the environment; an unparsable value
    // makes `AmazonS3Builder::build` fail.
    // SAFETY: this binary contains exactly one test and starts no threads of its own.
    unsafe { std::env::set_var("AWS_CONDITIONAL_PUT", "definitely-not-a-mode") };
    let mut cfg = common::s3_config("http://127.0.0.1:1".into(), "");
    if let Backend::S3(s3) = &mut cfg.backend {
        s3.access_key_id = None;
        s3.secret_access_key = None;
    }
    let err = MediaStore::open(cfg).err().expect("must fail");
    assert!(matches!(err, MediaError::Config(_)), "{err:?}");
}
