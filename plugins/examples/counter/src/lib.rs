//! Example `request.served` subscriber (M2a-2 P21): adds the number of events in each
//! batch to the decimal count under KV key `count`.
//!
//! **The count is approximate (R19)** — fine for analytics, not for anything exact:
//!
//! * **At-least-once delivery.** A batch is a `plugin.action` job; if the call fails
//!   after the `kv.set`, or the job's lease lapses, the job is retried and the same
//!   events are counted again.
//! * **Lossy.** Events buffered on a node are lost when it crashes, when its buffer is
//!   full (10 000 events), when a flush cannot enqueue, or when no subscriber is loaded
//!   on that node at flush time.
//! * **Read-modify-write race.** `kv.get` then `kv.set` is not atomic: with more than
//!   one runner worker (`cluster.workers`) or several nodes, two batches handled at once
//!   can both read the same count and one addition is lost.
//!
//! An atomic increment import (`kv.incr`) would fix the race; it needs a new WIT world
//! version and is deferred to a future one (the 0.1 `kv` interface has only
//! get/set/delete/list-keys).
//!
//! The batch is `{"events":[{…,"status":200,…},…]}`. Every event has exactly one
//! `status` key, and a quote inside a string value is escaped (`\"`), so the text
//! `"status":` occurs once per event whatever the key order: counting it counts the
//! events without a JSON parser in the component.
use ferrite_sdk::{ActionEvent, Ctx, PluginError, Result};

const KEY: &str = "count";

fn served(ev: ActionEvent, ctx: &Ctx) -> Result<()> {
    let batch = ev.payload.matches("\"status\":").count() as u64;
    let kv = ctx.kv();
    let count = kv
        .get(KEY)
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    kv.set(KEY, (count + batch).to_string().as_bytes())
        .map_err(PluginError::Other)
}

ferrite_sdk::export_plugin! {
    filters: {},
    actions: { "request.served" => served },
}
