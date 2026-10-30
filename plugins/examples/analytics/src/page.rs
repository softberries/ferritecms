//! The stats page: server-rendered HTML with an inline SVG bar chart and no script (the
//! route's CSP is `sandbox`, without `allow-scripts`). Every stored string goes through
//! [`escape`].

use std::fmt::Write;

use ferrite_sdk::html::escape;

use crate::date::Day;
use crate::store::{Counts, OTHER, Totals};

/// Longest path or host shown, in characters.
const SHOWN_CHARS: usize = 200;
/// Rows in each top list.
const TOP: usize = 20;
const CHART_WIDTH: f64 = 720.0;
const CHART_HEIGHT: f64 = 140.0;

const STYLE: &str = "\
:root{color-scheme:light dark;--bg:#fff;--fg:#1c2128;--muted:#59636e;--line:#d1d9e0;--soft:#f6f8fa;--accent:#2f6fdb}\
@media(prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#e6edf3;--muted:#9198a1;--line:#3d444d;--soft:#161b22;--accent:#5b9bf5}}\
*{box-sizing:border-box}\
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.5 system-ui,-apple-system,Segoe UI,sans-serif}\
main{max-width:56rem;margin:0 auto;padding:1.5rem 1rem 3rem}\
h1{font-size:1.5rem;margin:0}h2{font-size:1.05rem;margin:2rem 0 .5rem}\
header{display:flex;flex-wrap:wrap;gap:.75rem 1.5rem;align-items:baseline;justify-content:space-between}\
nav{display:flex;gap:.25rem}\
nav a{padding:.2rem .65rem;border:1px solid var(--line);border-radius:.4rem;color:var(--fg);text-decoration:none}\
nav a[aria-current]{background:var(--accent);border-color:var(--accent);color:#fff}\
.muted{color:var(--muted);margin:.25rem 0 0}\
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(9rem,1fr));gap:.75rem;margin:1.25rem 0}\
.tile{border:1px solid var(--line);border-radius:.6rem;padding:.75rem 1rem;background:var(--soft)}\
.tile .n{display:block;font-size:1.75rem;font-weight:650;font-variant-numeric:tabular-nums}\
.tile .l{color:var(--muted);font-size:.875rem}\
svg{display:block;width:100%;height:auto;margin:.5rem 0}\
.bar{fill:var(--accent)}.axis{stroke:var(--line);stroke-width:1}\
.t{fill:var(--muted);font-size:11px}\
table{width:100%;border-collapse:collapse}\
th,td{padding:.35rem .5rem;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}\
th{color:var(--muted);font-weight:500;font-size:.875rem}\
td.n,th.n{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}\
td.k{overflow-wrap:anywhere}code{font:.9em ui-monospace,SFMono-Regular,Menlo,monospace}\
@media(max-width:40rem){.tiles{grid-template-columns:repeat(2,1fr)}}";

/// A document with `body` inside `<main>`.
fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style>{STYLE}</style></head><body><main>{body}</main></body></html>",
        escape(title)
    )
}

/// The page for a visitor who is not signed in.
pub fn sign_in_required() -> String {
    document(
        "Analytics",
        "<h1>Analytics</h1><p>Sign in to the admin first, then open this page again.</p>\
         <p><a href=\"/admin/\">Go to the admin</a></p>",
    )
}

/// The page for a signed-in user who is not in the `viewers` setting.
pub fn not_a_viewer(user: &str) -> String {
    document(
        "Analytics",
        &format!(
            "<h1>Analytics</h1><p>You are signed in, but this page is only for the users \
             listed under Viewers in the analytics plugin's settings.</p>\
             <p>Your user id is <code>{}</code>. To allow yourself, open the admin, go to \
             Plugins, then analytics, and add this id to Viewers (several ids are separated \
             by commas or new lines).</p>",
            escape(user)
        ),
    )
}

/// The page when every stored day of the period failed to read.
pub fn unreadable(period: u32) -> String {
    let (days, s) = (period, if period == 1 { "" } else { "s" });
    document(
        "Analytics",
        &format!(
            "<h1>Analytics</h1><p>The statistics for the last {days} day{s} could not be read \
             right now. Try again in a moment.</p>"
        ),
    )
}

/// The page when the period holds no data.
pub fn no_data(period: u32) -> String {
    let (days, s) = (period, if period == 1 { "" } else { "s" });
    document(
        "Analytics",
        &format!(
            "<h1>Analytics</h1><p>No data yet for the last {days} day{s}. Page views appear \
             here a few seconds after visitors load pages of the site.</p>"
        ),
    )
}

/// One day of the window. `totals` is `None` for a day nothing was read for.
pub struct Slot {
    pub day: Day,
    pub totals: Option<Totals>,
    /// What `totals` holds besides the counts.
    pub detail: Detail,
    /// A stored blob of this day was listed but could not be read (the host answers a
    /// failed read as a missing key): the figures leave it out.
    pub unread: bool,
}

/// Whether a day's `paths` and `referrers` are in its [`Slot::totals`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Decoded with its lists.
    Lists,
    /// Its blob has lists, but the page's decode budget was spent: counts only.
    Skipped,
    /// Compacted (older than `detail_days`): it has counts only.
    TotalsOnly,
}

/// The stats page for `slots` (oldest first).
pub fn stats(slots: &[Slot], periods: &[u32], period: u32) -> String {
    let mut sum = Sum::default();
    for slot in slots.iter().filter_map(|s| s.totals.as_ref()) {
        sum.add(slot);
    }
    let first = slots.first().map(|s| s.day.key()).unwrap_or_default();
    let last = slots.last().map(|s| s.day.key()).unwrap_or_default();

    let mut out = String::new();
    out.push_str("<header><div><h1>Analytics</h1>");
    let _ = write!(
        out,
        "<p class=\"muted\">UTC days {} to {}. A page view is a browser request answered \
         200 or 304.</p></div><nav aria-label=\"Period\">",
        escape(&first),
        escape(&last)
    );
    for &p in periods {
        let current = if p == period {
            " aria-current=\"page\""
        } else {
            ""
        };
        let _ = write!(out, "<a href=\"?days={p}\"{current}>{p}d</a>");
    }
    out.push_str("</nav></header>");

    out.push_str("<div class=\"tiles\">");
    tile(&mut out, "views", sum.views, "page views");
    tile(&mut out, "bots", sum.bots, "bot requests");
    tile(&mut out, "other", sum.other, "other clients");
    tile(&mut out, "days", u64::from(period), "days shown");
    out.push_str("</div>");

    out.push_str("<h2>Views per day</h2>");
    chart(&mut out, slots);

    out.push_str("<h2>Responses</h2><div class=\"tiles\">");
    for class in ["2xx", "3xx", "4xx", "5xx"] {
        let n = sum.status.get(class).copied().unwrap_or(0);
        tile(&mut out, &format!("status-{class}"), n, class);
    }
    out.push_str("</div>");

    let with_data: Vec<Detail> = slots
        .iter()
        .filter(|s| s.totals.is_some())
        .map(|s| s.detail)
        .collect();
    let count = |d: Detail| with_data.iter().filter(|&&x| x == d).count();
    let (listed, skipped, compacted) = (
        count(Detail::Lists),
        count(Detail::Skipped),
        count(Detail::TotalsOnly),
    );
    if listed < with_data.len() {
        let _ = write!(
            out,
            "<p class=\"muted\" data-note=\"partial\">The lists below cover the newest {listed} \
             of the {} days with data.",
            with_data.len()
        );
        if skipped > 0 {
            out.push_str(
                " Reading more would exceed what the plugin may compute in one request: \
                 choose a shorter period to include more days.",
            );
        }
        if compacted > 0 {
            let _ = write!(
                out,
                " {compacted} older {} only {} totals (see the <code>detail_days</code> setting).",
                if compacted == 1 {
                    "day keeps"
                } else {
                    "days keep"
                },
                if compacted == 1 { "its" } else { "their" }
            );
        }
        out.push_str("</p>");
    }
    let unread = slots.iter().filter(|s| s.unread).count();
    if unread > 0 {
        let _ = write!(
            out,
            "<p class=\"muted\" data-note=\"unread\">{unread} {} could not be read right now \
             and {} left out of these figures. Reload the page in a moment.</p>",
            if unread == 1 { "day" } else { "days" },
            if unread == 1 { "is" } else { "are" }
        );
    }
    out.push_str("<h2>Top pages</h2>");
    top(&mut out, "Path", &sum.paths, true);
    out.push_str("<h2>Top referrers</h2>");
    top(&mut out, "Host", &sum.referrers, false);
    document("Analytics", &out)
}

fn tile(out: &mut String, name: &str, n: u64, label: &str) {
    let _ = write!(
        out,
        "<div class=\"tile\"><span class=\"n\" data-stat=\"{}\">{n}</span>\
         <span class=\"l\">{}</span></div>",
        escape(name),
        escape(label)
    );
}

/// The window's counts added up.
#[derive(Default)]
struct Sum {
    views: u64,
    bots: u64,
    other: u64,
    status: Counts,
    paths: Counts,
    referrers: Counts,
}

impl Sum {
    fn add(&mut self, day: &Totals) {
        self.views += day.views;
        self.bots += day.bots;
        self.other += day.other;
        for (into, from) in [
            (&mut self.status, &day.status),
            (&mut self.paths, &day.paths),
            (&mut self.referrers, &day.referrers),
        ] {
            for (key, n) in from {
                // `get_mut` first: most keys repeat from day to day, and a clone is an
                // allocation and the fuel is counted in instructions.
                match into.get_mut(key) {
                    Some(sum) => *sum += n,
                    None => {
                        into.insert(key.clone(), *n);
                    }
                }
            }
        }
    }
}

/// An inline SVG bar chart, one bar per day, with a tooltip each.
fn chart(out: &mut String, slots: &[Slot]) {
    let views: Vec<u64> = slots
        .iter()
        .map(|s| s.totals.as_ref().map_or(0, |t| t.views))
        .collect();
    let max = views.iter().copied().max().unwrap_or(0).max(1);
    let width = CHART_WIDTH / slots.len().max(1) as f64;
    let _ = write!(
        out,
        "<svg viewBox=\"0 0 {CHART_WIDTH} {}\" role=\"img\" aria-label=\"Page views per day\">",
        CHART_HEIGHT + 22.0
    );
    for (i, (slot, &n)) in slots.iter().zip(&views).enumerate() {
        let h = CHART_HEIGHT * n as f64 / max as f64;
        let _ = write!(
            out,
            "<rect class=\"bar\" x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{h:.2}\">\
             <title>{}: {n} views</title></rect>",
            i as f64 * width + width * 0.1,
            CHART_HEIGHT - h,
            width * 0.8,
            escape(&slot.day.key()),
        );
    }
    let _ = write!(
        out,
        "<line class=\"axis\" x1=\"0\" y1=\"{CHART_HEIGHT}\" x2=\"{CHART_WIDTH}\" y2=\"{CHART_HEIGHT}\"/>\
         <text class=\"t\" x=\"0\" y=\"11\">{max} views</text>\
         <text class=\"t\" x=\"0\" y=\"{}\">{}</text>\
         <text class=\"t\" x=\"{CHART_WIDTH}\" y=\"{}\" text-anchor=\"end\">{}</text></svg>",
        CHART_HEIGHT + 16.0,
        escape(&slots.first().map(|s| s.day.key()).unwrap_or_default()),
        CHART_HEIGHT + 16.0,
        escape(&slots.last().map(|s| s.day.key()).unwrap_or_default()),
    );
}

/// The `TOP` biggest keys of `counts`, then the overflow bucket. With `decode`, a key is
/// shown percent-decoded where that gives readable text ([`readable_path`]).
fn top(out: &mut String, heading: &str, counts: &Counts, decode: bool) {
    let mut rows: Vec<(&String, u64)> = counts
        .iter()
        .filter(|(k, _)| k.as_str() != OTHER)
        .map(|(k, &n)| (k, n))
        .collect();
    let order = |a: &(&String, u64), b: &(&String, u64)| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0));
    if rows.len() > TOP {
        // Only the top rows need an order: select them first (linear), then sort those.
        rows.select_nth_unstable_by(TOP - 1, order);
        rows.truncate(TOP);
    }
    rows.sort_by(order);
    let rest = counts.get(OTHER).copied().filter(|&n| n > 0);
    if rows.is_empty() && rest.is_none() {
        out.push_str("<p class=\"muted\">Nothing yet.</p>");
        return;
    }
    let _ = write!(
        out,
        "<table><thead><tr><th>{}</th><th class=\"n\">Views</th></tr></thead><tbody>",
        escape(heading)
    );
    for (key, n) in rows.into_iter().take(TOP) {
        let shown = if decode {
            readable_path(key)
        } else {
            key.clone()
        };
        let _ = write!(
            out,
            "<tr><td class=\"k\"><code>{}</code></td><td class=\"n\">{n}</td></tr>",
            escape(&clip(&shown))
        );
    }
    if let Some(n) = rest {
        let _ = write!(
            out,
            "<tr><td class=\"k\">{}</td><td class=\"n\">{n}</td></tr>",
            escape(OTHER)
        );
    }
    out.push_str("</tbody></table>");
}

/// A stored path for display: core sends paths percent-encoded (`/caf%C3%A9/`), so it is
/// shown decoded (`/café/`) when the decoded bytes are UTF-8 without control characters,
/// and as stored otherwise. Display only: the stored key, `exclude_paths` and the counts
/// stay encoded, and the result is escaped like any other text.
pub fn readable_path(path: &str) -> String {
    if !path.contains('%') {
        return path.to_owned();
    }
    let hex = |c: u8| (c as char).to_digit(16);
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    match String::from_utf8(out) {
        Ok(text) if !text.chars().any(char::is_control) => text,
        _ => path.to_owned(),
    }
}

/// `s` cut to [`SHOWN_CHARS`] characters, with an ellipsis when it was longer.
fn clip(s: &str) -> String {
    match s.char_indices().nth(SHOWN_CHARS) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::readable_path;

    #[test]
    fn encoded_paths_are_shown_decoded_when_readable() {
        assert_eq!(readable_path("/caf%C3%A9/"), "/café/");
        assert_eq!(readable_path("/%D0%B0%D0%B1/"), "/аб/");
        assert_eq!(readable_path("/a%20b/%3Cx%3E"), "/a b/<x>");
        assert_eq!(readable_path("/plain/"), "/plain/");
        // Lowercase hex decodes too.
        assert_eq!(readable_path("/caf%c3%a9"), "/café");
    }

    #[test]
    fn unreadable_paths_are_shown_as_stored() {
        // Not UTF-8 once decoded: a lone lead byte, a cut sequence.
        assert_eq!(readable_path("/%FF/"), "/%FF/");
        assert_eq!(readable_path("/caf%C3"), "/caf%C3");
        // Control characters stay encoded.
        assert_eq!(readable_path("/a%00b"), "/a%00b");
        assert_eq!(readable_path("/a%0Ab"), "/a%0Ab");
        // A `%` that starts no escape is kept as it is.
        assert_eq!(readable_path("/100%/x%4"), "/100%/x%4");
    }
}
