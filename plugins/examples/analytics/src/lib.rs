//! Privacy-friendly analytics example plugin: the batched `request.served` action and a
//! stats page at `/_p/analytics/`.
//!
//! An event carries a path, a status, a time, the referrer's host and a coarse user-agent
//! class, and nothing else: no cookie, no IP address (not even a hash), no query string.
//! So there is nothing to identify a visitor with, and this plugin cannot count unique
//! visitors. It counts page views per UTC day, in one JSON blob per day under the KV key
//! `day:YYYY-MM-DD` (see [`store`]). A key `latest` names the newest day stored.
//!
//! **The plugin has no clock.** "Today" is the newest event day it has seen. That is what
//! the retention cutoff and the stats window count back from, so both follow the events
//! and the tests can drive them.
//!
//! **The counts are approximate (R19).** A batch is a `plugin.action` job, delivered at
//! least once, and reading a blob then writing it back is not atomic:
//!
//! * a job retried after a failed call counts its events again;
//! * two batches handled at the same moment (several runner workers or nodes) can read the
//!   same blob, and one of them is lost;
//! * events buffered on a node are lost when it crashes or its buffer is full.
//!
//! Fine for a trend, not for billing. An atomic increment (`kv.incr`) needs a new world
//! version.
//!
//! **Budgets.** A call may make 100 KV operations and write 1 MiB. A batch reads and writes
//! each touched day once (at most [`MAX_DAYS`] of them, the newest), then lists the day
//! keys, oldest first, and deletes the ones past retention with what is left of the 100
//! operations; the next batch continues. A day whose blob would not fit in the write budget
//! is dropped and logged rather than failing the call.
//!
//! **Fuel.** The limit is counted in guest instructions, and decoding JSON is the cost. The
//! counts live in a hash map ([`store::Counts`]): a `BTreeMap` of paths, which share long
//! prefixes, was many times too slow. The stats page decodes only the newest
//! [`FULL_DECODE_BYTES`] of blobs with their maps; the rest give their totals alone.

mod date;
mod page;
mod store;

use std::collections::BTreeMap;

use ferrite_sdk::{
    ActionEvent, Ctx, Kv, Level, PluginError, Result, RouteRequest, RouteResponse, options, route,
    viewers_allow,
};
use serde::Deserialize;
use serde_json::Value;

use date::Day;
use page::Slot;
use store::{Event, Stored, Totals, host_of};

const DAY_PREFIX: &str = "day:";
const LATEST: &str = "latest";
/// KV operations one call may make.
const MAX_OPS: u32 = 100;
/// Key and value bytes one call may write.
const MAX_WRITTEN: usize = 1024 * 1024;
/// Most days one batch writes: two reads and writes each, leaving most of the 100
/// operations for the prune.
const MAX_DAYS: usize = 8;
/// What `list-keys` returns at most.
const LIST_LIMIT: u32 = 100;

const DEFAULT_RETENTION: u32 = 90;
const MAX_RETENTION: u32 = 366;
const DEFAULT_EXCLUDE: &str = "/admin,/_p";

/// Days the stats page can show, and the one it shows without `?days=`.
const PERIODS: [u32; 4] = [1, 7, 30, 90];
const MAX_PERIOD: u32 = 90;
const DEFAULT_PERIOD: u32 = 30;
/// Stored bytes the stats page decodes in full per request (see `stats`).
const FULL_DECODE_BYTES: usize = 200_000;

// ---- settings -------------------------------------------------------------------------

/// A text setting; `None` when unset.
fn setting_string(ctx: &Ctx, key: &str) -> Option<String> {
    match serde_json::from_str::<Value>(&ctx.setting(key)?).ok()? {
        Value::String(s) => Some(s),
        _ => None,
    }
}

/// `retention_days`: 1 to 366, 90 when unset or not a number.
fn retention_days(ctx: &Ctx) -> u32 {
    let n = ctx
        .setting("retention_days")
        .and_then(|json| serde_json::from_str::<Value>(&json).ok())
        .and_then(|v| v.as_f64())
        .filter(|n| n.is_finite());
    match n {
        // The cast saturates; the clamp does the rest.
        Some(n) => (n.round().clamp(1.0, f64::from(MAX_RETENTION))) as u32,
        None => DEFAULT_RETENTION,
    }
}

/// `exclude_paths` as path prefixes without a trailing slash. Entries that do not start
/// with `/` are ignored.
fn excluded_prefixes(ctx: &Ctx) -> Vec<String> {
    let setting = setting_string(ctx, "exclude_paths");
    setting
        .as_deref()
        .unwrap_or(DEFAULT_EXCLUDE)
        .split(',')
        .map(str::trim)
        .filter(|p| p.starts_with('/'))
        .map(|p| p.trim_end_matches('/').to_owned())
        .collect()
}

/// True when `path` is a prefix or lies under one (`/admin` covers `/admin/x`, not
/// `/administrator`).
fn is_excluded(prefixes: &[String], path: &str) -> bool {
    prefixes.iter().any(|p| {
        path.strip_prefix(p.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

// ---- counting -------------------------------------------------------------------------

#[derive(Deserialize)]
struct Batch {
    events: Vec<Value>,
}

/// Why a KV call was not made.
enum Refused {
    /// The call is out of operations or write bytes.
    Budget,
    /// The host refused the write.
    Kv(String),
}

/// The `kv` handle with the per-call budget counted, so the plugin stops before the host
/// starts answering "budget exceeded".
struct Meter {
    kv: Kv,
    ops: u32,
    written: usize,
}

impl Meter {
    fn new(kv: Kv) -> Self {
        Self {
            kv,
            ops: 0,
            written: 0,
        }
    }

    fn take(&mut self) -> core::result::Result<(), Refused> {
        if self.ops >= MAX_OPS {
            return Err(Refused::Budget);
        }
        self.ops += 1;
        Ok(())
    }

    fn get(&mut self, key: &str) -> core::result::Result<Option<Vec<u8>>, Refused> {
        self.take()?;
        Ok(self.kv.get(key))
    }

    fn set(&mut self, key: &str, value: &[u8]) -> core::result::Result<(), Refused> {
        if self.ops >= MAX_OPS || self.written + key.len() + value.len() > MAX_WRITTEN {
            return Err(Refused::Budget);
        }
        self.ops += 1;
        self.written += key.len() + value.len();
        self.kv.set(key, value).map_err(Refused::Kv)
    }

    fn delete(&mut self, key: &str) -> core::result::Result<(), Refused> {
        self.take()?;
        self.kv.delete(key);
        Ok(())
    }

    fn list(&mut self, prefix: &str) -> core::result::Result<Vec<String>, Refused> {
        self.take()?;
        Ok(self.kv.list_keys(prefix, LIST_LIMIT))
    }
}

fn day_key(day: Day) -> String {
    format!("{DAY_PREFIX}{}", day.key())
}

fn warn(msg: &str) {
    ferrite_sdk::log(Level::Warn, &format!("analytics: {msg}"));
}

/// Add `events` to the stored blob of `day`, read and written once.
fn count_day(
    meter: &mut Meter,
    day: Day,
    events: &[Event],
    self_host: Option<&str>,
) -> core::result::Result<(), Refused> {
    let key = day_key(day);
    let mut totals = match meter.get(&key)?.as_deref().map(Stored::decode) {
        None => Totals::default(),
        Some(Stored::Totals(t)) => t,
        Some(Stored::Newer) => {
            warn(&format!("{key} was written by a newer version, left alone"));
            return Ok(());
        }
        Some(Stored::Corrupt) => {
            warn(&format!(
                "{key} is not a blob of this plugin, starting it again"
            ));
            Totals::default()
        }
    };
    for event in events {
        totals.record(event, self_host);
    }
    let blob = serde_json::to_vec(&totals).map_err(|e| Refused::Kv(e.to_string()))?;
    meter.set(&key, &blob)
}

fn served(ev: ActionEvent, ctx: &Ctx) -> Result<()> {
    let batch: Batch = serde_json::from_str(&ev.payload)
        .map_err(|e| PluginError::InvalidInput(format!("request.served payload: {e}")))?;
    let excluded = excluded_prefixes(ctx);
    let self_host = options::get("site.url").as_deref().and_then(host_of);

    // Group by the event's own UTC day. An event that cannot be read, or lies under an
    // excluded path, is skipped.
    let mut by_day: BTreeMap<Day, Vec<Event>> = BTreeMap::new();
    for raw in batch.events {
        let Ok(event) = serde_json::from_value::<Event>(raw) else {
            continue;
        };
        let Some(day) = Day::from_ts(&event.ts) else {
            continue;
        };
        if !is_excluded(&excluded, &event.path) {
            by_day.entry(day).or_default().push(event);
        }
    }
    if by_day.is_empty() {
        return Ok(());
    }

    let mut meter = Meter::new(ctx.kv());
    let refused = |r: Refused| match r {
        Refused::Budget => PluginError::Other("kv call budget exceeded".to_owned()),
        Refused::Kv(msg) => PluginError::Other(msg),
    };
    let stored_latest = meter
        .get(LATEST)
        .map_err(refused)?
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| Day::parse_key(&s));
    let batch_newest = by_day.keys().next_back().copied();
    // Retention keeps the newest N days, counted back from the newest day seen.
    let now = batch_newest.max(stored_latest);
    let keep_from = now.map(|now| now.before(retention_days(ctx) - 1));
    if let Some(keep_from) = keep_from {
        by_day.retain(|day, _| *day >= keep_from);
    }

    if by_day.len() > MAX_DAYS {
        warn(&format!(
            "a batch spans {} days; counting only the newest {MAX_DAYS}",
            by_day.len()
        ));
    }
    // Newest first, so that if the write budget runs out it is the oldest day that loses.
    let mut failed = None;
    for (day, events) in by_day.iter().rev().take(MAX_DAYS) {
        match count_day(&mut meter, *day, events, self_host.as_deref()) {
            Ok(()) => {}
            Err(Refused::Budget) => warn(&format!(
                "{} not counted: it does not fit in this call's budget",
                day.key()
            )),
            Err(Refused::Kv(msg)) => failed = Some(msg),
        }
    }

    if let Some(newest) = by_day.keys().next_back()
        && stored_latest.is_none_or(|latest| *newest > latest)
        && let Err(Refused::Kv(msg)) = meter.set(LATEST, newest.key().as_bytes())
    {
        failed = Some(msg);
    }

    if let Some(keep_from) = keep_from {
        prune(&mut meter, keep_from);
    }
    // A failed write fails the call, so the job is retried: at-least-once delivery.
    failed.map_or(Ok(()), |msg| Err(PluginError::Other(msg)))
}

/// Delete the day blobs older than `keep_from`, oldest first, while operations are left.
fn prune(meter: &mut Meter, keep_from: Day) {
    let Ok(keys) = meter.list(DAY_PREFIX) else {
        return;
    };
    for key in keys {
        let Some(day) = key.strip_prefix(DAY_PREFIX).and_then(Day::parse_key) else {
            continue;
        };
        if day >= keep_from || meter.delete(&key).is_err() {
            break;
        }
    }
}

// ---- the stats page -------------------------------------------------------------------

/// `?days=`: the first value as a number of days, 1 to 90; 30 when it is not a number.
fn period(req: &RouteRequest) -> u32 {
    req.query
        .iter()
        .find(|(k, _)| k == "days")
        .and_then(|(_, v)| v.trim().parse::<u32>().ok())
        .map_or(DEFAULT_PERIOD, |n| n.clamp(1, MAX_PERIOD))
}

/// The page body of an HTML response.
fn html(status: u16, body: String) -> RouteResponse {
    route::html(status, &body)
}

fn stats(req: RouteRequest, ctx: &Ctx) -> Result<RouteResponse> {
    let Some(user) = req.user.as_deref() else {
        return Ok(html(403, page::sign_in_required()));
    };
    let viewers = setting_string(ctx, "viewers");
    if !viewers_allow(viewers.as_deref(), Some(user)) {
        return Ok(html(403, page::not_a_viewer(user)));
    }

    let kv = ctx.kv();
    let Some(latest) = kv
        .get(LATEST)
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| Day::parse_key(&s))
    else {
        return Ok(html(200, page::no_data()));
    };
    // The newest day, then one read per day of the window: 91 of the 100 operations at
    // most. A day with nothing stored (or a blob this version cannot read) counts as empty.
    //
    // Decoding costs fuel in proportion to the bytes, and a busy site's blob is tens of
    // kilobytes, so only the newest blobs up to `FULL_DECODE_BYTES` are decoded with their
    // path and referrer maps; the older ones give their totals alone, and the page says so.
    let period = period(&req);
    let mut full_left = FULL_DECODE_BYTES;
    let mut slots: Vec<Slot> = (0..period)
        .map(|back| {
            let day = latest.before(back);
            let mut full = false;
            let totals = kv.get(&day_key(day)).and_then(|bytes| {
                full = bytes.len() <= full_left;
                if full {
                    full_left -= bytes.len();
                }
                let stored = if full {
                    Stored::decode(&bytes)
                } else {
                    Stored::decode_brief(&bytes)
                };
                match stored {
                    Stored::Totals(t) => Some(t),
                    _ => None,
                }
            });
            Slot { day, totals, full }
        })
        .collect();
    slots.reverse();
    Ok(html(200, page::stats(&slots, &PERIODS, period)))
}

ferrite_sdk::export_plugin! {
    filters: {},
    actions: { "request.served" => served },
    routes: { "/" => stats },
}
