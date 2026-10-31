//! Privacy-friendly analytics example plugin: the batched `request.served` action and a
//! stats page at `/_p/analytics/`.
//!
//! An event carries a path, a status, a time, the referrer's host and a coarse user-agent
//! class, and nothing else: no cookie, no IP address (not even a hash), no query string.
//! So there is nothing to identify a visitor with, and this plugin cannot count unique
//! visitors. It counts page views per UTC day, in one JSON blob per day (see [`store`]):
//!
//! * a day of the last `detail_days` (default 30, at most 60) keeps its path and referrer
//!   lists, under `day:YYYY-MM-DD`;
//! * an older day is *compacted* to its counts alone, under `totals:YYYY-MM-DD`;
//! * a day older than `retention_days` (default 90, at most 366) is deleted.
//!
//! **Storage (ruling X7).** A plugin may keep 10 MiB in `kv`, and a full day blob can reach
//! about 137 KB. The two tiers keep the worst case under that (64 full
//! blobs and 366 compacted ones; `the_worst_case_footprint_fits_the_quota` checks it), and
//! every batch prunes and compacts *before* it counts, so a store that is full still frees
//! space: a lowered `retention_days` or `detail_days` takes effect at the next batch even
//! when its counting write is refused.
//!
//! **"Today" is the wall clock's UTC day** (`wasi:clocks/wall-clock` is linked for every
//! plugin). The retention cutoff, the detail window and the end of the stats window count
//! back from it. An event is filed under its own `ts` day, but one dated more than a day
//! ahead of today is ignored: a node whose clock runs fast must not be able to push the
//! history out.
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
//! **Budgets.** A call may make 100 KV operations. A batch first lists the full days,
//! oldest first, deletes those past retention and compacts at most [`MAX_COMPACTIONS`] of
//! those past the detail window, then lists the compacted days and deletes those past
//! retention, always keeping the operations its counting needs. Then it reads and writes
//! each touched day once (at most [`MAX_DAYS`] of them, the newest). The next batch goes on
//! where this one stopped. A batch also decodes and re-encodes at most
//! [`BATCH_DECODE_BYTES`] of stored blobs: the newest touched day always, an older one only
//! if it fits; compaction reads only the counts at the head of a blob. Fuel, not the 1 MiB a
//! call may write, is what limits a batch, and a day that does not fit is skipped and
//! logged (its events are lost) instead of trapping the call.
//!
//! **Fuel.** The limit is counted in guest instructions, and decoding JSON is the cost. The
//! counts live in a std `HashMap` ([`store::Counts`]; its random seed comes from
//! `wasi:random`, which is linked), because a `BTreeMap` was many times slower. The stats
//! page decodes the newest day with data in full and then more days up to
//! [`FULL_DECODE_DAYS`] in all within [`FULL_DECODE_BYTES`]; older days give their totals
//! alone.

mod date;
mod page;
mod store;

use std::collections::{BTreeMap, BTreeSet};

use ferrite_sdk::{
    ActionEvent, Ctx, Kv, Level, PluginError, Result, RouteRequest, RouteResponse, options, route,
    settings, viewers,
};
use serde::Deserialize;

use date::Day;
use page::{Detail, Slot};
use store::{Event, Stored, Totals, host_of};

/// Full days (with their lists).
const DAY_PREFIX: &str = "day:";
/// Compacted days (their counts alone).
const TOTALS_PREFIX: &str = "totals:";
/// KV operations one call may make.
const MAX_OPS: u32 = 100;
/// Stored bytes one batch decodes and re-encodes. The newest touched day always counts;
/// an older one that would take the total past this is skipped (its events are lost). A
/// blob the plugin writes is at most about 137 KB (paths cut to 200 bytes, hosts to 100,
/// core sends them percent-encoded), and decoding costs fuel in proportion.
const BATCH_DECODE_BYTES: usize = 150_000;
/// Most days one batch writes. A batch holds a few seconds of one node's events, so it spans
/// two days at midnight; three leave room for a clock that steps. Each day is a read, a check
/// that a missing blob is missing, and a write.
const MAX_DAYS: usize = 3;
/// KV operations counting one day takes at most (see [`MAX_DAYS`]).
const COUNT_OPS: u32 = 3;
/// Most days one batch compacts. Each reads only the counts at the head of its blob, so
/// the cost is the operations; one a day is the steady state, and a backlog (a lowered
/// `detail_days`) clears over a few batches.
const MAX_COMPACTIONS: usize = 8;
/// KV operations one compaction takes at most: read the day, read its compacted blob and
/// check that a missing one is missing, write it, then delete the day; when that write is
/// refused, delete the day first and write again.
const COMPACT_OPS: u32 = 6;
/// What `list-keys` returns at most.
const LIST_LIMIT: u32 = 100;

const DEFAULT_RETENTION: u32 = 90;
const MAX_RETENTION: u32 = 366;
const DEFAULT_DETAIL: u32 = 30;
const MAX_DETAIL: u32 = 60;
const DEFAULT_EXCLUDE: &str = "/admin,/_p";

// The detail window lies within retention, and a batch's two listings, one compaction and
// its counting fit in one call's operations together.
const _: () = assert!(MAX_DETAIL <= MAX_RETENTION && DEFAULT_DETAIL <= DEFAULT_RETENTION);
const _: () = assert!(2 + COMPACT_OPS + COUNT_OPS * MAX_DAYS as u32 <= MAX_OPS);

/// Days the stats page can show, and the one it shows without `?days=`.
const PERIODS: [u32; 4] = [1, 7, 30, 90];
const MAX_PERIOD: u32 = 90;
const DEFAULT_PERIOD: u32 = 30;
/// Most days, and stored bytes in all, the stats page decodes in full per request; the
/// newest day with data is always one of them (see `stats`).
const FULL_DECODE_DAYS: usize = 4;
const FULL_DECODE_BYTES: usize = 150_000;

// ---- settings -------------------------------------------------------------------------

/// A number setting rounded and clamped to `1..=max`; `default` when unset or not a
/// number.
fn days_setting(key: &str, default: u32, max: u32) -> u32 {
    match settings::number(key) {
        // The cast saturates; the clamp does the rest.
        Some(n) => (n.round().clamp(1.0, f64::from(max))) as u32,
        None => default,
    }
}

/// `retention_days`: 1 to 366, 90 when unset or not a number.
fn retention_days() -> u32 {
    days_setting("retention_days", DEFAULT_RETENTION, MAX_RETENTION)
}

/// `detail_days`: 1 to 60, 30 when unset or not a number; never more than the retention.
fn detail_days(retention: u32) -> u32 {
    days_setting("detail_days", DEFAULT_DETAIL, MAX_DETAIL).min(retention)
}

/// `exclude_paths` as path prefixes without a trailing slash. Entries that do not start
/// with `/` are ignored.
fn excluded_prefixes() -> Vec<String> {
    let setting = settings::text("exclude_paths");
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

// ---- KV access ------------------------------------------------------------------------

#[derive(Deserialize)]
struct Batch<'a> {
    #[serde(borrow)]
    events: Vec<Event<'a>>,
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

    /// Operations left in this call.
    fn left(&self) -> u32 {
        MAX_OPS - self.ops
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

    /// Whether `key` exists, by a listing (which does not read the value): how a `None`
    /// from [`Meter::get`] is told apart from a read that failed.
    fn exists(&mut self, key: &str) -> core::result::Result<bool, Refused> {
        Ok(self.list(key, 1)?.iter().any(|k| k == key))
    }
}

const OUT_OF_OPS: &str = "this call is out of KV operations";
/// How the host's refusal over the plugin's storage quota starts (05 §6).
const QUOTA_EXCEEDED: &str = "kv quota exceeded";

fn day_key(day: Day) -> String {
    format!("{DAY_PREFIX}{}", day.key())
}

fn totals_key(day: Day) -> String {
    format!("{TOTALS_PREFIX}{}", day.key())
}

fn warn(msg: &str) {
    ferrite_sdk::log(Level::Warn, &format!("analytics: {msg}"));
}

// ---- prune and compaction -------------------------------------------------------------

/// The days a batch keeps, counted back from today.
#[derive(Clone, Copy)]
struct Window {
    /// The oldest day kept at all.
    keep_from: Day,
    /// The oldest day that keeps its lists.
    detail_from: Day,
}

/// Prune and compact, oldest first, leaving `reserve` operations for counting. Full days
/// past retention are deleted and at most [`MAX_COMPACTIONS`] past the detail window are
/// compacted (always at least one when one is due and nothing failed), then compacted days
/// past retention are deleted. A failure is logged and ends the step it happened in; the
/// call goes on to count.
fn maintain(meter: &mut Meter, window: Window, reserve: u32) {
    // One operation stays for listing the compacted days.
    let keep = reserve + 1;
    if let Ok(keys) = meter.list(DAY_PREFIX, LIST_LIMIT) {
        let mut compacted = 0;
        for key in keys {
            let Some(day) = key.strip_prefix(DAY_PREFIX).and_then(Day::parse_key) else {
                continue;
            };
            if day >= window.detail_from {
                break;
            }
            if day < window.keep_from {
                // Deletes leave room for one compaction after them; the rest wait for the
                // next batch.
                if meter.left() > keep + COMPACT_OPS {
                    let _ = meter.delete(&key);
                }
                continue;
            }
            if compacted == MAX_COMPACTIONS || meter.left() < keep + COMPACT_OPS {
                break;
            }
            compacted += 1;
            if let Err(why) = compact(meter, day) {
                warn(&format!("{} not compacted: {why}", day.key()));
                break;
            }
        }
    }
    if let Ok(keys) = meter.list(TOTALS_PREFIX, LIST_LIMIT) {
        for key in keys {
            let Some(day) = key.strip_prefix(TOTALS_PREFIX).and_then(Day::parse_key) else {
                continue;
            };
            if day >= window.keep_from || meter.left() <= reserve || meter.delete(&key).is_err() {
                break;
            }
        }
    }
}

/// Move the counts of the full day `day` into its compacted blob (adding to one already
/// there, from events counted after it was compacted) and delete the full blob. Only the
/// counts at the head of the blob are read ([`Stored::decode_head`]). A day that cannot
/// be read is left for the next batch, one written by a newer version is left alone, and
/// one that is not a blob of this plugin is deleted. `Err` when a write failed: the full
/// blob is then kept, unless it was deleted to make room (and the day's counts are lost).
fn compact(meter: &mut Meter, day: Day) -> core::result::Result<(), String> {
    let budget = |_: Refused| OUT_OF_OPS.to_owned();
    let key = day_key(day);
    let Some(bytes) = meter.get(&key).map_err(budget)? else {
        // The day was listed, so the read failed (or it was deleted since).
        warn(&format!("{key} could not be read; compacting it later"));
        return Ok(());
    };
    let mut totals = match Stored::decode_head(&bytes) {
        Stored::Totals(t) => t,
        Stored::Newer => {
            warn(&format!("{key} was written by a newer version, left alone"));
            return Ok(());
        }
        Stored::Corrupt => {
            warn(&format!("{key} is not a blob of this plugin, deleted"));
            meter.delete(&key).map_err(budget)?;
            return Ok(());
        }
    };
    let tkey = totals_key(day);
    match meter.get(&tkey).map_err(budget)? {
        None => {
            if meter.exists(&tkey).map_err(budget)? {
                warn(&format!(
                    "{tkey} exists but could not be read; {key} left for later"
                ));
                return Ok(());
            }
        }
        Some(stored) => match Stored::decode_summary(&stored) {
            Stored::Totals(earlier) => totals.add_counts(&earlier),
            Stored::Newer => {
                warn(&format!(
                    "{tkey} was written by a newer version, {key} left alone"
                ));
                return Ok(());
            }
            Stored::Corrupt => warn(&format!("{tkey} is not a blob of this plugin, replaced")),
        },
    }
    let blob = totals.summary_blob()?;
    match meter.set(&tkey, &blob) {
        Ok(()) => {}
        Err(Refused::Budget) => return Err(OUT_OF_OPS.to_owned()),
        // Over the quota: the full blob is the big one, so free its space first and write
        // again. Any other refusal may be transient, so the full blob stays for a later
        // batch.
        Err(Refused::Kv(first)) if first.starts_with(QUOTA_EXCEEDED) => {
            warn(&format!(
                "{tkey} not written ({first}); deleting {key} first to make room"
            ));
            meter.delete(&key).map_err(budget)?;
            return match meter.set(&tkey, &blob) {
                Ok(()) => Ok(()),
                Err(Refused::Budget) => Err(OUT_OF_OPS.to_owned()),
                Err(Refused::Kv(msg)) => Err(format!("{msg}; the counts of {key} are lost")),
            };
        }
        Err(Refused::Kv(msg)) => return Err(format!("{tkey} not written ({msg}); {key} kept")),
    }
    meter.delete(&key).map_err(budget)
}

// ---- counting -------------------------------------------------------------------------

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
    events: &[Event<'_>],
    self_host: Option<&str>,
    decoded: &mut Decoded,
) -> core::result::Result<(), Refused> {
    let key = day_key(day);
    let stored = meter.get(&key)?;
    let mut totals = match stored.as_deref() {
        None => {
            // The host answers `None` both for a missing key and for a read that failed.
            // Writing a fresh blob over a day that exists would lose that day's counts.
            if meter.exists(&key)? {
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
    let excluded = excluded_prefixes();
    let self_host = options::get("site.url").as_deref().and_then(host_of);

    // "Today" is the wall clock's UTC day. Retention keeps the newest N days counted back
    // from it, the detail window the newest `detail_days` of those, and an event more than
    // a day ahead of it (a node whose clock runs fast) is ignored, so it cannot push
    // history out.
    let today = Day::today();
    let retention = retention_days();
    let window = Window {
        keep_from: today.before(retention - 1),
        detail_from: today.before(detail_days(retention) - 1),
    };
    let newest_allowed = today.after(1);

    // Group by the event's own UTC day. An event that cannot be read, lies under an
    // excluded path, or falls outside the window is skipped.
    let mut by_day: BTreeMap<Day, Vec<Event<'_>>> = BTreeMap::new();
    let mut ahead = 0usize;
    for event in batch.events {
        let Some(day) = Day::from_ts(&event.ts) else {
            continue;
        };
        if day > newest_allowed {
            ahead += 1;
        } else if day >= window.keep_from && !is_excluded(&excluded, &event.path) {
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
    if by_day.len() > MAX_DAYS {
        warn(&format!(
            "a batch spans {} days; counting only the newest {MAX_DAYS}",
            by_day.len()
        ));
    }
    let counted = by_day.len().min(MAX_DAYS);

    // Prune and compact first, so that a store at its quota frees space before the
    // counting writes, and a failed write cannot stop it.
    let mut meter = Meter::new(ctx.kv());
    maintain(&mut meter, window, COUNT_OPS * counted as u32);

    // Newest first, so that if the KV operations or the decode budget run out it is the
    // oldest day that loses. The first host error ends the loop: every further write
    // would only add to what a retry of the job counts twice.
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
    Ok(())
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

/// The days of the window (`days`, newest first) that have a key under `prefix`, by one
/// listing per month: at most 4 operations for 90 days, instead of one per day.
fn listed_days(
    meter: &mut Meter,
    prefix: &str,
    days: &[Day],
) -> core::result::Result<BTreeSet<Day>, Refused> {
    let months: BTreeSet<String> = days.iter().map(|d| d.month()).collect();
    let mut found = BTreeSet::new();
    for month in months {
        let keys = meter.list(&format!("{prefix}{month}"), LIST_LIMIT)?;
        found.extend(
            keys.iter()
                .filter_map(|k| k.strip_prefix(prefix).and_then(Day::parse_key)),
        );
    }
    Ok(found)
}

/// The page's decode budget: the newest day with data in full, then more while fewer
/// than [`FULL_DECODE_DAYS`] are decoded and their blobs add up to [`FULL_DECODE_BYTES`];
/// from the first day that does not fit, none.
struct FullBudget {
    open: bool,
    days: usize,
    bytes: usize,
}

impl FullBudget {
    fn take(&mut self, len: usize) -> bool {
        self.open = self.open
            && (self.days == 0
                || (self.days < FULL_DECODE_DAYS && self.bytes + len <= FULL_DECODE_BYTES));
        if self.open {
            self.days += 1;
            self.bytes += len;
        }
        self.open
    }
}

/// One day of the window: its full blob (lists within the budget) and its compacted one,
/// each read only when listed. A listed blob whose read answers `None` failed to read.
fn read_day(
    meter: &mut Meter,
    day: Day,
    full: bool,
    compacted: bool,
    budget: &mut FullBudget,
) -> Slot {
    let mut slot = Slot {
        day,
        totals: None,
        detail: Detail::TotalsOnly,
        unread: false,
    };
    if full {
        match meter.get(&day_key(day)) {
            Ok(Some(bytes)) => {
                let lists = budget.take(bytes.len());
                let stored = if lists {
                    Stored::decode(&bytes)
                } else {
                    Stored::decode_brief(&bytes)
                };
                if let Stored::Totals(t) = stored {
                    slot.totals = Some(t);
                    slot.detail = if lists {
                        Detail::Lists
                    } else {
                        Detail::Skipped
                    };
                }
            }
            Ok(None) | Err(_) => slot.unread = true,
        }
    }
    if compacted {
        match meter.get(&totals_key(day)) {
            Ok(Some(bytes)) => {
                if let Stored::Totals(t) = Stored::decode_summary(&bytes) {
                    match &mut slot.totals {
                        Some(totals) => totals.add_counts(&t),
                        None => slot.totals = Some(t),
                    }
                }
            }
            Ok(None) | Err(_) => slot.unread = true,
        }
    }
    slot
}

fn stats(req: RouteRequest, ctx: &Ctx) -> Result<RouteResponse> {
    let Some(user) = req.user.as_deref() else {
        return Ok(html(403, page::sign_in_required()));
    };
    let allowed = settings::text("viewers");
    if !viewers::allow(allowed.as_deref(), Some(user)) {
        return Ok(html(403, page::not_a_viewer(user)));
    }

    // Which days have a blob comes from listings, one per month and tier (8 operations at
    // most), so only stored days are read: 90 reads at most, and a listed day whose read
    // answers `None` is a failed read, which the page says, not an empty day. A blob this
    // version cannot read counts as empty.
    //
    // Decoding costs fuel in proportion to the entries, and a busy site's blob holds up to
    // 700 of them. So only the newest day with data is always decoded with its path and
    // referrer maps; after it, further days follow while fewer than `FULL_DECODE_DAYS` are
    // decoded and their blobs add up to `FULL_DECODE_BYTES`. From the first day that does not
    // fit, every older day gives its totals alone, and the page says so. This also bounds the
    // map the page sums the lists in: at most `FULL_DECODE_DAYS` days of capped maps.
    let mut meter = Meter::new(ctx.kv());
    let today = Day::today();
    let period = period(&req);
    let window: Vec<Day> = (0..period).map(|back| today.before(back)).collect();
    let (full, compacted) = match (
        listed_days(&mut meter, DAY_PREFIX, &window),
        listed_days(&mut meter, TOTALS_PREFIX, &window),
    ) {
        (Ok(full), Ok(compacted)) => (full, compacted),
        _ => return Ok(html(200, page::unreadable(period))),
    };
    let mut budget = FullBudget {
        open: true,
        days: 0,
        bytes: 0,
    };
    let mut slots: Vec<Slot> = window
        .iter()
        .map(|&day| {
            read_day(
                &mut meter,
                day,
                full.contains(&day),
                compacted.contains(&day),
                &mut budget,
            )
        })
        .collect();
    slots.reverse();
    if slots.iter().all(|s| s.totals.is_none()) {
        if slots.iter().any(|s| s.unread) {
            return Ok(html(200, page::unreadable(period)));
        }
        return Ok(html(200, page::no_data(period)));
    }
    Ok(html(200, page::stats(&slots, &PERIODS, period)))
}

ferrite_sdk::export_plugin! {
    filters: {},
    actions: { "request.served" => served },
    routes: { "/" => stats },
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::{MAX_HOST_BYTES, MAX_PATH_BYTES, MAX_PATHS, MAX_REFERRERS, OTHER};

    /// Most full day blobs the store can hold at once: the detail window, tomorrow (a node
    /// a little ahead), and the days one batch may count below the window before the next
    /// batch compacts them.
    const FULL_DAYS_AT_MOST: u32 = MAX_DETAIL + 1 + MAX_DAYS as u32;

    /// The largest day blob the plugin can write: every map full, every key as long as
    /// the cut allows (core's paths and hosts need no JSON escape), every count `u64::MAX`.
    fn largest_full_blob() -> usize {
        let mut t = Totals::default();
        let big = u64::MAX;
        (t.views, t.bots, t.other) = (big, big, big);
        for class in ["2xx", "3xx", "4xx", "5xx"] {
            t.status.insert(class.into(), big);
        }
        for i in 0..MAX_PATHS {
            let key = format!("/{i:03}{}", "p".repeat(MAX_PATH_BYTES - 4));
            assert_eq!(key.len(), MAX_PATH_BYTES);
            t.paths.insert(key, big);
        }
        t.paths.insert(OTHER.into(), big);
        for i in 0..MAX_REFERRERS {
            let key = format!("{i:03}{}", "h".repeat(MAX_HOST_BYTES - 3));
            assert_eq!(key.len(), MAX_HOST_BYTES);
            t.referrers.insert(key, big);
        }
        t.referrers.insert(OTHER.into(), big);
        serde_json::to_vec(&t).unwrap().len()
    }

    /// The largest compacted blob: every count `u64::MAX`.
    fn largest_compacted_blob() -> usize {
        let mut t = Totals::default();
        let big = u64::MAX;
        (t.views, t.bots, t.other) = (big, big, big);
        for class in ["2xx", "3xx", "4xx", "5xx"] {
            t.status.insert(class.into(), big);
        }
        t.summary_blob().unwrap().len()
    }

    /// Ruling X7: the most the store can hold stays under the plugin's 10 MiB quota with
    /// room to spare (about 9 MiB): `FULL_DAYS_AT_MOST` full blobs and one compacted blob
    /// per day of the longest retention, each as large as the plugin can write.
    #[test]
    fn the_worst_case_footprint_fits_the_quota() {
        let full = largest_full_blob();
        let compacted = largest_compacted_blob();
        assert!((130_000..140_000).contains(&full), "{full}");
        assert!(compacted < 256, "{compacted}");
        let worst = FULL_DAYS_AT_MOST as usize * full + MAX_RETENTION as usize * compacted;
        let quota = 10 * 1024 * 1024;
        assert!(worst < 9 * 1024 * 1024, "{worst} bytes");
        assert!(worst < quota);
        // The brief's figure, without the transient days, is lower still.
        assert!(MAX_DETAIL as usize * full + MAX_RETENTION as usize * compacted < worst);
    }
}
