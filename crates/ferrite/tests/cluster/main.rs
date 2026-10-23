#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Cluster integration tests (M1.5 exit criterion, 04): 2–3 in-process nodes
//! (`ferrite::start`) on one Postgres schema, each with its own store and data folder and
//! one shared media folder. A save on one node reaches the others within 1 s, a crashed
//! node's job is finished elsewhere after its lease, jobs run exactly once, nodes of two
//! versions share a schema, and concurrent first starts migrate once. Postgres only:
//! every test prints `SKIP` without `FERRITE_TEST_PG_URL` (panics when `CI=true`).
//!
//! Tests run one at a time (the harness serialises them) so the latency bound does not
//! depend on how many clusters share the machine; observed latencies are printed on
//! stderr (`--nocapture`).

mod harness;
mod invalidation;
mod jobs;
mod reload;
mod rolling;

#[path = "../../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;
