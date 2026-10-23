#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The real `ferrite` binary end to end, as a child process on a random port, on both
//! profiles: solo (SQLite + local media) and cluster (Postgres + in-process S3; skipped
//! without `FERRITE_TEST_PG_URL`, panics when `CI=true`). `site` covers `seed demo` +
//! `serve` (public site, two processes, startup refusals, shutdown); `api` drives the
//! `/api/v1` API over raw HTTP (`api_end_to_end_on_real_binary`). One harness for both.

mod api;
mod harness;
mod site;
