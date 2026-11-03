//! The external link phase of a health run (design 16 §6.3): the `link_checks` cache,
//! robots.txt per host, and the pace (at most [`Pacing::concurrency`] checks at once,
//! one at a time per host with [`Pacing::host_gap`] between requests, at most
//! [`Pacing::budget`] checks and [`Pacing::phase`] for the phase; what is left waits for
//! the next run, the oldest results first).

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use ferrite_domain::{LinkCheck, LinkStatus, Timestamp};
use ferrite_store::{MAX_LINK_BATCH, Store};
use tokio::{sync::Semaphore, task::JoinSet};
use url::Url;

use super::{LinkOutcome, LinkProbe, RobotsFetch, robots};
use crate::{CoreError, tx::finish};

/// An `ok` result is reused this long.
pub const OK_REUSE: Duration = Duration::from_secs(7 * 24 * 3600);
/// A failing URL is broken after two failures at least this far apart.
pub const FAILING_SPREAD: Duration = Duration::from_secs(20 * 3600);
/// Cache rows not seen for this long are deleted.
pub const UNSEEN_PRUNE: Duration = Duration::from_secs(30 * 24 * 3600);

/// The pace of the phase (design 16 §6.3; tests shorten it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacing {
    pub concurrency: usize,
    pub host_gap: Duration,
    pub budget: usize,
    pub phase: Duration,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            concurrency: 4,
            host_gap: Duration::from_secs(2),
            budget: 1_000,
            phase: Duration::from_secs(30 * 60),
        }
    }
}

/// What the phase found.
#[derive(Debug, Default)]
pub(super) struct ExternalResult {
    /// URLs actually requested.
    pub checked: usize,
    /// The cache row of every URL seen (after this run).
    pub rows: HashMap<String, LinkCheck>,
}

fn micros(d: Duration) -> i64 {
    i64::try_from(d.as_micros()).unwrap_or(i64::MAX)
}

/// The cache row after `outcome` (design 16 §6.3).
pub(super) fn next(
    prev: Option<&LinkCheck>,
    url: &Url,
    outcome: LinkOutcome,
    now: Timestamp,
) -> LinkCheck {
    let host = url.host_str().unwrap_or("").to_owned();
    let mut row = prev.cloned().unwrap_or(LinkCheck {
        url: url.to_string(),
        host: host.clone(),
        status: LinkStatus::Skipped,
        http_status: None,
        detail: None,
        failures: 0,
        first_failed_at: None,
        checked_at: now,
        last_seen_at: now,
    });
    row.host = host;
    row.last_seen_at = now;
    let healthy = |row: &mut LinkCheck, status, code: Option<u16>, detail: Option<&str>| {
        row.status = status;
        row.http_status = code.map(i32::from);
        row.detail = detail.map(str::to_owned);
        row.failures = 0;
        row.first_failed_at = None;
        row.checked_at = now;
    };
    match outcome {
        LinkOutcome::Ok(c) | LinkOutcome::Restricted(c) => {
            healthy(&mut row, LinkStatus::Ok, Some(c), None)
        }
        LinkOutcome::NotFound(c) => {
            healthy(&mut row, LinkStatus::Broken, Some(c), Some("not found"))
        }
        LinkOutcome::Gone => healthy(&mut row, LinkStatus::Broken, Some(410), Some("gone")),
        LinkOutcome::NoSuchHost => healthy(&mut row, LinkStatus::Broken, None, Some("nxdomain")),
        LinkOutcome::Refused(why) | LinkOutcome::Unchecked(why) => {
            healthy(&mut row, LinkStatus::Skipped, None, Some(why));
        }
        LinkOutcome::RateLimited => {
            // Retried next run: the previous verdict stands.
            if prev.is_none() {
                row.status = LinkStatus::Skipped;
                row.detail = Some("rate limited".into());
            }
        }
        LinkOutcome::Failed(class) => {
            let first = row.first_failed_at.unwrap_or(now);
            row.failures = row.failures.saturating_add(1);
            row.first_failed_at = Some(first);
            row.http_status = None;
            row.detail = Some(class);
            row.checked_at = now;
            let spread = now.as_micros() - first.as_micros();
            row.status = if row.failures >= 2 && spread >= micros(FAILING_SPREAD) {
                LinkStatus::Broken
            } else {
                LinkStatus::Failing
            };
        }
    }
    row
}

/// Whether a cached row is fresh enough to skip a request.
fn reusable(row: &LinkCheck, now: Timestamp) -> bool {
    row.status == LinkStatus::Ok && now.as_micros() - row.checked_at.as_micros() < micros(OK_REUSE)
}

/// Runs the phase over `urls` (distinct, absolute `http(s)`), writing the cache rows.
pub(super) async fn run(
    store: &Arc<dyn Store>,
    probe: Arc<dyn LinkProbe>,
    urls: Vec<String>,
    pacing: Pacing,
    now: Timestamp,
) -> Result<ExternalResult, CoreError> {
    let mut cached: HashMap<String, LinkCheck> = HashMap::new();
    for chunk in urls.chunks(MAX_LINK_BATCH) {
        for row in store.read().quality().link_get(chunk).await? {
            cached.insert(row.url.clone(), row);
        }
    }
    let mut due: Vec<(Option<Timestamp>, Url)> = Vec::new();
    let mut rows: HashMap<String, LinkCheck> = HashMap::new();
    for u in &urls {
        let Ok(parsed) = Url::parse(u) else { continue };
        match cached.get(u) {
            Some(row) if reusable(row, now) => {
                let mut row = row.clone();
                row.last_seen_at = now;
                rows.insert(u.clone(), row);
            }
            Some(row) => {
                let mut kept = row.clone();
                kept.last_seen_at = now;
                rows.insert(u.clone(), kept);
                due.push((Some(row.checked_at), parsed));
            }
            None => due.push((None, parsed)),
        }
    }
    // Oldest results first (never checked before everything).
    due.sort_by_key(|(at, _)| at.map_or(i64::MIN, |t| t.as_micros()));
    due.truncate(pacing.budget);
    let mut by_host: BTreeMap<String, Vec<Url>> = BTreeMap::new();
    for (_, u) in due {
        by_host
            .entry(u.host_str().unwrap_or("").to_owned())
            .or_default()
            .push(u);
    }
    let deadline = Instant::now() + pacing.phase;
    let slots = Arc::new(Semaphore::new(pacing.concurrency.max(1)));
    let checked = Arc::new(AtomicUsize::new(0));
    let mut tasks = JoinSet::new();
    for (host, list) in by_host {
        let probe = probe.clone();
        let slots = slots.clone();
        let checked = checked.clone();
        let gap = pacing.host_gap;
        tasks.spawn(
            async move { host_worker(host, list, probe, slots, checked, gap, deadline).await },
        );
    }
    let mut outcomes: Vec<(Url, LinkOutcome)> = Vec::new();
    while let Some(done) = tasks.join_next().await {
        match done {
            Ok(list) => outcomes.extend(list),
            Err(e) => tracing::warn!(error = %e, "health: an external link worker failed"),
        }
    }
    for (url, outcome) in outcomes {
        let key = url.to_string();
        let row = next(cached.get(&key), &url, outcome, now);
        rows.insert(key, row);
    }
    let all: Vec<&LinkCheck> = rows.values().collect();
    for chunk in all.chunks(200) {
        let tx = store.begin().await?;
        let done = async {
            for row in chunk {
                tx.quality().link_put(row).await?;
            }
            Ok(())
        }
        .await;
        finish(tx, done).await?;
    }
    let cutoff = Timestamp::from_micros(now.as_micros() - micros(UNSEEN_PRUNE))
        .map_err(|_| CoreError::Internal("link prune time"))?;
    store.read().quality().link_prune(cutoff).await.ok();
    Ok(ExternalResult {
        checked: checked.load(Ordering::Relaxed),
        rows,
    })
}

/// One host's URLs, one at a time with `gap` between requests; robots.txt first.
async fn host_worker(
    host: String,
    list: Vec<Url>,
    probe: Arc<dyn LinkProbe>,
    slots: Arc<Semaphore>,
    checked: Arc<AtomicUsize>,
    gap: Duration,
    deadline: Instant,
) -> Vec<(Url, LinkOutcome)> {
    let Some(first) = list.first() else {
        return Vec::new();
    };
    let rules = {
        let Ok(_permit) = slots.acquire().await else {
            return Vec::new();
        };
        match probe.robots(first).await {
            RobotsFetch::Body(b) => robots::parse(&b),
            RobotsFetch::AllowAll => robots::Rules::default(),
            RobotsFetch::SkipHost => {
                tracing::debug!(host, "health: robots.txt unavailable; host skipped");
                return Vec::new();
            }
        }
    };
    let mut out = Vec::new();
    for url in list {
        let mut path = url.path().to_owned();
        if let Some(q) = url.query() {
            path.push('?');
            path.push_str(q);
        }
        if !rules.allows(&path) {
            out.push((url, LinkOutcome::Unchecked("robots")));
            continue;
        }
        tokio::time::sleep(gap).await;
        if Instant::now() >= deadline {
            break;
        }
        let outcome = {
            let Ok(_permit) = slots.acquire().await else {
                break;
            };
            checked.fetch_add(1, Ordering::Relaxed);
            probe.check(&url).await
        };
        out.push((url, outcome));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000_000;

    fn at(hours: i64) -> Timestamp {
        Timestamp::from_micros(1_700_000_000_000_000 + hours * HOUR).unwrap()
    }

    #[test]
    fn failing_becomes_broken_after_two_failures_20h_apart() {
        let u = Url::parse("https://example.org/x").unwrap();
        let one = next(None, &u, LinkOutcome::Failed("timeout".into()), at(0));
        assert_eq!((one.status, one.failures), (LinkStatus::Failing, 1));
        let two = next(Some(&one), &u, LinkOutcome::Failed("timeout".into()), at(5));
        assert_eq!(two.status, LinkStatus::Failing, "too close together");
        let three = next(
            Some(&two),
            &u,
            LinkOutcome::Failed("http 503".into()),
            at(21),
        );
        assert_eq!(three.status, LinkStatus::Broken);
        assert_eq!(three.first_failed_at, Some(at(0)));
        // A success clears it.
        let ok = next(Some(&three), &u, LinkOutcome::Ok(200), at(22));
        assert_eq!(
            (ok.status, ok.failures, ok.first_failed_at),
            (LinkStatus::Ok, 0, None)
        );
    }

    #[test]
    fn classification_and_rate_limits() {
        let u = Url::parse("https://example.org/x").unwrap();
        assert_eq!(
            next(None, &u, LinkOutcome::NotFound(404), at(0)).status,
            LinkStatus::Broken
        );
        assert_eq!(
            next(None, &u, LinkOutcome::Gone, at(0)).status,
            LinkStatus::Broken
        );
        assert_eq!(
            next(None, &u, LinkOutcome::NoSuchHost, at(0)).status,
            LinkStatus::Broken
        );
        assert_eq!(
            next(None, &u, LinkOutcome::Restricted(403), at(0)).status,
            LinkStatus::Ok
        );
        assert_eq!(
            next(None, &u, LinkOutcome::Unchecked("robots"), at(0)).status,
            LinkStatus::Skipped
        );
        let ok = next(None, &u, LinkOutcome::Ok(200), at(0));
        let limited = next(Some(&ok), &u, LinkOutcome::RateLimited, at(30));
        assert_eq!(
            (limited.status, limited.checked_at),
            (LinkStatus::Ok, at(0))
        );
        assert!(reusable(&ok, at(24 * 6)));
        assert!(!reusable(&ok, at(24 * 7)));
    }
}
