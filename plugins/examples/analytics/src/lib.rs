//! Privacy-friendly analytics example plugin: the batched `request.served` action and a
//! stats page at `/_p/analytics/`.
//!
//! An event carries a path, a status, a time, the referrer's host and a coarse user-agent
//! class, and nothing else: no cookie, no IP address (not even a hash), no query string.
//! So there is nothing to identify a visitor with, and this plugin cannot count unique
//! visitors. It counts page views per UTC day, in one JSON blob per day under the KV key
//! `day:YYYY-MM-DD` (see [`store`]).
//!
//! **"Today" is the wall clock's UTC day** (`wasi:clocks/wall-clock` is linked for every
//! plugin). The retention cutoff and the end of the stats window count back from it. An
//! event is filed under its own `ts` day, but one dated more than a day ahead of today is
//! ignored: a node whose clock runs fast must not be able to push the history out.
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
//! **Budgets.** A call may make 100 KV operations. A batch reads and writes each touched
//! day once (at most [`MAX_DAYS`] of them, the newest), then lists the day keys, oldest
//! first, and deletes the ones past retention with what is left of the 100 operations; the
//! next batch continues. A batch also decodes and re-encodes at most [`BATCH_DECODE_BYTES`]
//! of stored blobs: the newest touched day always, an older one only if it fits. Fuel, not
//! the 1 MiB a call may write, is what limits a batch, and a day that does not fit is
//! skipped and logged (its events are lost) instead of trapping the call.
//!
//! **Fuel.** The limit is counted in guest instructions, and decoding JSON is the cost. The
//! counts live in a std `HashMap` ([`store::Counts`]; its random seed comes from
//! `wasi:random`, which is linked), because a `BTreeMap` was many times slower. The stats
//! page decodes the newest day with data in full and then more days up to [`FULL_DECODE_DAYS`]
//! in all within [`FULL_DECODE_BYTES`]; older days give their totals alone.

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
/// KV operations one call may make.
const MAX_OPS: u32 = 100;
/// Stored bytes one batch decodes and re-encodes. The newest touched day always counts;
/// an older one that would take the total past this is skipped (its events are lost). A
/// blob the plugin writes is at most about 130 KB (paths cut to 200 bytes, hosts to 100,
/// core sends them percent-encoded), and decoding costs fuel in proportion.
const BATCH_DECODE_BYTES: usize = 150_000;
/// Most days one batch writes. A batch holds a few seconds of one node's events, so it spans
/// two days at midnight; three leave room for a clock that steps. Each day is a read, a check
/// that a missing blob is missing, and a write, leaving most of the 100 operations for the
/// prune.
const MAX_DAYS: usize = 3;
/// What `list-keys` returns at most.
const LIST_LIMIT: u32 = 100;

const DEFAULT_RETENTION: u32 = 90;
const MAX_RETENTION: u32 = 366;
const DEFAULT_EXCLUDE: &str = "/admin,/_p";

/// Days the stats page can show, and the one it shows without `?days=`.
const PERIODS: [u32; 4] = [1, 7, 30, 90];
const MAX_PERIOD: u32 = 90;
const DEFAULT_PERIOD: u32 = 30;
/// Most days, and stored bytes in all, the stats page decodes in full per request; the
/// newest day with data is always one of them (see `stats`).
const FULL_DECODE_DAYS: usize = 4;
const FULL_DECODE_BYTES: usize = 150_000;

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
    events: Vec<Event>,
}

/// Why a KV call was not made.
enum Refused {
    /// The call is out of KV operations.
    Budget,
    /// The host refused the write.
    Kv(String),
}

/// The `kv` handle with the per-call budget counted, so the plugin stops before the host
/// starts answering "budget exceeded".
struct Meter {
    kv: Kv,
    ops: u32,
}

impl Meter {
    fn new(kv: Kv) -> Self {
        Self { kv, ops: 0 }
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
        self.take()?;
        self.kv.set(key, value).map_err(Refused::Kv)
    }

    fn delete(&mut self, key: &str) -> core::result::Result<(), Refused> {
        self.take()?;
        self.kv.delete(key);
        Ok(())
    }

    fn list(&mut self, prefix: &str, limit: u32) -> core::result::Result<Vec<String>, Refused> {
        self.take()?;
        Ok(self.kv.list_keys(prefix, limit))
    }
}

fn day_key(day: Day) -> String {
    format!("{DAY_PREFIX}{}", day.key())
}

fn warn(msg: &str) {
    ferrite_sdk::log(Level::Warn, &format!("analytics: {msg}"));
}

/// How much of the batch's decode budget is spent.
#[derive(Default)]
struct Decoded {
    days: usize,
    bytes: usize,
}

/// Add `events` to the stored blob of `day`, read and written once. A day that is left
/// alone (a newer blob, a read that looks failed, no room in the decode budget) is logged
/// and its events are lost.
fn count_day(
    meter: &mut Meter,
    day: Day,
    events: &[Event],
    self_host: Option<&str>,
    decoded: &mut Decoded,
) -> core::result::Result<(), Refused> {
    let key = day_key(day);
    let stored = meter.get(&key)?;
    let mut totals = match stored.as_deref() {
        None => {
            // The host answers `None` both for a missing key and for a read that failed.
            // Writing a fresh blob over a day that exists would lose that day's counts.
            if !meter.list(&key, 1)?.is_empty() {
                warn(&format!("{key} exists but could not be read, left alone"));
                return Ok(());
            }
            Totals::default()
        }
        Some(bytes) => {
            if decoded.days > 0 && decoded.bytes + bytes.len() > BATCH_DECODE_BYTES {
                warn(&format!(
                    "{key} not counted: its blob ({} bytes) does not fit in this batch's decode budget",
                    bytes.len()
                ));
                return Ok(());
            }
            decoded.days += 1;
            decoded.bytes += bytes.len();
            match Stored::decode(bytes) {
                Stored::Totals(t) => t,
                Stored::Newer => {
                    warn(&format!("{key} was written by a newer version, left alone"));
                    return Ok(());
                }
                Stored::Corrupt => {
                    warn(&format!(
                        "{key} is not a blob of this plugin, starting it again"
                    ));
                    Totals::default()
                }
            }
        }
    };
    for event in events {
        totals.record(event, self_host);
    }
    let blob = serde_json::to_vec(&totals).map_err(|e| Refused::Kv(e.to_string()))?;
    meter.set(&key, &blob)
}

fn served(ev: ActionEvent, ctx: &Ctx) -> Result<()> {
    // Core always sends `{"events":[…]}` with events of one shape. A payload that does not
    // parse, an event of another shape included, is logged and dropped whole: a retry could
    // not fix it, and failures count toward auto-disable. (An event whose time cannot be
    // read is only skipped.)
    let batch: Batch = match serde_json::from_str(&ev.payload) {
        Ok(batch) => batch,
        Err(e) => {
            warn(&format!("request.served payload not understood: {e}"));
            return Ok(());
        }
    };
    let excluded = excluded_prefixes(ctx);
    let self_host = options::get("site.url").as_deref().and_then(host_of);

    // "Today" is the wall clock's UTC day. Retention keeps the newest N days counted back
    // from it, and an event more than a day ahead of it (a node whose clock runs fast) is
    // ignored, so it cannot push history out.
    let today = Day::today();
    let keep_from = today.before(retention_days(ctx) - 1);
    let newest_allowed = today.after(1);

    // Group by the event's own UTC day. An event that cannot be read, lies under an
    // excluded path, or falls outside the window is skipped.
    let mut by_day: BTreeMap<Day, Vec<Event>> = BTreeMap::new();
    let mut ahead = 0usize;
    for event in batch.events {
        let Some(day) = Day::from_ts(&event.ts) else {
            continue;
        };
        if day > newest_allowed {
            ahead += 1;
        } else if day >= keep_from && !is_excluded(&excluded, &event.path) {
            by_day.entry(day).or_default().push(event);
        }
    }
    if ahead > 0 {
        // A warning: a node whose clock is more than a day behind this one's would lose
        // every event it handles, and this line is how that shows.
        warn(&format!(
            "ignored {ahead} events dated more than a day ahead of today"
        ));
    }
    if by_day.is_empty() {
        return Ok(());
    }

    if by_day.len() > MAX_DAYS {
        warn(&format!(
            "a batch spans {} days; counting only the newest {MAX_DAYS}",
            by_day.len()
        ));
    }
    // Newest first, so that if the KV operations or the decode budget run out it is the
    // oldest day that loses.
    // The first host error ends the loop: every further write would only add to what a
    // retry of the job counts twice.
    let mut meter = Meter::new(ctx.kv());
    let mut decoded = Decoded::default();
    for (day, events) in by_day.iter().rev().take(MAX_DAYS) {
        match count_day(&mut meter, *day, events, self_host.as_deref(), &mut decoded) {
            Ok(()) => {}
            Err(Refused::Budget) => warn(&format!(
                "{} not counted: this call is out of KV operations",
                day.key()
            )),
            // A failed write fails the call, so the job is retried: at-least-once delivery.
            Err(Refused::Kv(msg)) => return Err(PluginError::Other(msg)),
        }
    }
    prune(&mut meter, keep_from);
    Ok(())
}

/// Delete the day blobs older than `keep_from`, oldest first, while operations are left.
fn prune(meter: &mut Meter, keep_from: Day) {
    let Ok(keys) = meter.list(DAY_PREFIX, LIST_LIMIT) else {
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

    // One read per day of the window, ending today: 90 of the 100 operations at most. A day
    // with nothing stored (or a blob this version cannot read) counts as empty.
    //
    // Decoding costs fuel in proportion to the entries, and a busy site's blob holds up to
    // 700 of them. So only the newest day with data is always decoded with its path and
    // referrer maps; after it, further days follow while fewer than `FULL_DECODE_DAYS` are
    // decoded and their blobs add up to `FULL_DECODE_BYTES`. From the first day that does not
    // fit, every older day gives its totals alone, and the page says so. This also bounds the
    // map the page sums the lists in: at most `FULL_DECODE_DAYS` days of capped maps.
    let kv = ctx.kv();
    let today = Day::today();
    let period = period(&req);
    let mut full_open = true;
    let mut full_days = 0usize;
    let mut full_bytes = 0usize;
    let mut slots: Vec<Slot> = (0..period)
        .map(|back| {
            let day = today.before(back);
            let mut full = false;
            let totals = kv.get(&day_key(day)).and_then(|bytes| {
                full = full_open
                    && (full_days == 0
                        || (full_days < FULL_DECODE_DAYS
                            && full_bytes + bytes.len() <= FULL_DECODE_BYTES));
                full_open = full;
                if full {
                    full_days += 1;
                    full_bytes += bytes.len();
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
    if slots.iter().all(|s| s.totals.is_none()) {
        return Ok(html(200, page::no_data(period)));
    }
    Ok(html(200, page::stats(&slots, &PERIODS, period)))
}

ferrite_sdk::export_plugin! {
    filters: {},
    actions: { "request.served" => served },
    routes: { "/" => stats },
}
