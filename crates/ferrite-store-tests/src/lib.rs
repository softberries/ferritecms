//! Backend-neutral contract tests for FerriteCMS stores.
//!
//! Every test is a `pub async fn(db: &TestDb)`; `tests/contract.rs` instantiates each one
//! for SQLite and Postgres with [`backend_tests!`].

// Test-support crate: failing fast with a message is the desired behaviour.
#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod batch;
pub mod constraints;
pub mod content;
pub mod counting;
pub mod event;
pub mod fixtures;
pub mod harness;
pub mod menu;
pub mod migrations;
pub mod plugin;
pub mod search;
pub mod semantics;
pub mod site;
pub mod taxonomy;
pub mod tx;
pub mod user;

pub use counting::CountingStore;
pub use harness::{PgFixture, TestDb, migration_dirs, migration_files, pg_store, sqlite_store};
