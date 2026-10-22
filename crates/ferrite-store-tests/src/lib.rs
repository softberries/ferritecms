//! Backend-neutral contract tests for FerriteCMS stores.
//!
//! Every test is a `pub async fn(db: &TestDb)`; `tests/contract.rs` instantiates each one
//! for SQLite and Postgres with [`backend_tests!`].

// Test-support crate: failing fast with a message is the desired behaviour.
#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod harness;
pub mod migrations;

pub use harness::{PgFixture, TestDb, migration_dirs, migration_files, pg_store, sqlite_store};
