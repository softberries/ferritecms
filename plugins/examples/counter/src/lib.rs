//! Example `request.served` subscriber (M2a-2 P21): adds the number of events in each
//! batch to the decimal count under KV key `count`.
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
