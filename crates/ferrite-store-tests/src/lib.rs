//! Backend-neutral contract tests for FerriteCMS stores.
//!
//! Every test is a `pub async fn(db: &TestDb)`; `tests/contract.rs` instantiates each one
//! for SQLite and Postgres with [`backend_tests!`].

// Test-support crate: failing fast with a message is the desired behaviour.
#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod auth;
pub mod autosave;
pub mod batch;
pub mod comment;
pub mod comment_counts;
pub mod comment_threads;
pub mod constraints;
pub mod content;
pub mod counting;
pub mod event;
pub mod fixtures;
pub mod harness;
pub mod idempotency;
pub mod job;
pub mod job_list;
pub mod lock;
pub mod menu;
pub mod migrations;
pub mod node;
pub mod notify;
pub mod plugin;
pub mod plugin_failure;
pub mod search;
pub mod semantics;
pub mod site;
pub mod taxonomy;
pub mod tx;
pub mod user;
pub mod user_filters;

pub use counting::CountingStore;
pub use harness::{PgFixture, TestDb, migration_dirs, migration_files, pg_store, sqlite_store};
