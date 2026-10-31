//! Fixed rate-limit windows in tests (F10). The limits count per window of the database
//! clock (`start = now - now % w`), so a test whose hits must share one window first
//! waits for enough of it ([`align_to_window`]). A loaded host can still be slower than
//! that margin: [`in_one_window`] runs the scenario again in a fresh window when it failed
//! after the window rolled over, and re-raises a failure inside one window as it is.

use std::{future::Future, panic::AssertUnwindSafe, time::Duration};

use ferrite_store::Store;
use futures::FutureExt;

/// How many times [`in_one_window`] runs a scenario in all.
pub const WINDOW_ATTEMPTS: u32 = 3;

fn micros(d: Duration) -> i64 {
    i64::try_from(d.as_micros()).unwrap()
}

async fn db_micros(store: &dyn Store) -> i64 {
    store.read().nodes().db_now().await.unwrap().as_micros()
}

/// Sleeps until 50 ms past the next `window` boundary of the database clock when fewer
/// than `need` remain in the current window, so a test's hits share one window (F10).
pub async fn align_to_window(store: &dyn Store, window: Duration, need: Duration) {
    let w = micros(window);
    let left = w - db_micros(store).await.rem_euclid(w);
    if left < micros(need) {
        tokio::time::sleep(Duration::from_micros(left.unsigned_abs()) + Duration::from_millis(50))
            .await;
    }
}

/// Runs `scenario` so that its rate-limit hits share one `window` of the database clock.
///
/// It first waits as [`align_to_window`] does for `need`. When the scenario fails (panics)
/// and the window it started in had ended by the time it failed, the failure says nothing
/// about the limits (the host was slower than `need`): it waits for the next window and
/// runs the scenario again, at most [`WINDOW_ATTEMPTS`] times in all, and re-raises the
/// last failure. A failure within one window is a real one and is re-raised at once; a
/// pass is a pass. `scenario` gets the attempt (0, 1, …) so it can use fresh identities
/// where an earlier attempt left state behind (accounts, comments, buckets of a longer
/// window).
pub async fn in_one_window<F, Fut>(
    store: &dyn Store,
    window: Duration,
    need: Duration,
    mut scenario: F,
) where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = ()>,
{
    let w = micros(window);
    align_to_window(store, window, need).await;
    for attempt in 0..WINDOW_ATTEMPTS {
        let started = db_micros(store).await.div_euclid(w);
        let Err(panic) = AssertUnwindSafe(scenario(attempt)).catch_unwind().await else {
            return;
        };
        let ended = db_micros(store).await.div_euclid(w);
        if ended == started || attempt + 1 == WINDOW_ATTEMPTS {
            std::panic::resume_unwind(panic);
        }
        eprintln!(
            "in_one_window: attempt {} failed after its {window:?} window rolled over; \
             running it again in the next window",
            attempt + 1
        );
        align_to_window(store, window, window).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    use super::*;
    use crate::TestDb;

    const W: Duration = Duration::from_millis(400);

    #[tokio::test]
    async fn a_failure_after_a_rollover_runs_again_in_a_fresh_window() {
        let db = TestDb::sqlite().await;
        let runs = AtomicU32::new(0);
        let seen = std::sync::Mutex::new(Vec::new());
        let (runs_ref, seen_ref) = (&runs, &seen);
        in_one_window(db.store(), W, W / 2, |attempt| async move {
            runs_ref.fetch_add(1, Ordering::SeqCst);
            seen_ref.lock().unwrap().push(attempt);
            if attempt == 0 {
                // Slower than the window: the next one has begun when this fails.
                tokio::time::sleep(W + Duration::from_millis(50)).await;
                panic!("hits spread over two windows");
            }
        })
        .await;
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert_eq!(*seen.lock().unwrap(), [0, 1]);
    }

    #[tokio::test]
    async fn a_failure_within_one_window_is_reraised_at_once() {
        let db = Arc::new(TestDb::sqlite().await);
        let runs = Arc::new(AtomicU32::new(0));
        let (d, r) = (db.clone(), runs.clone());
        let failed = tokio::spawn(async move {
            let r = &r;
            in_one_window(d.store(), Duration::from_secs(3600), W, |_| async move {
                r.fetch_add(1, Ordering::SeqCst);
                panic!("a real failure");
            })
            .await;
        })
        .await
        .unwrap_err();
        assert!(failed.is_panic());
        let msg = failed.into_panic();
        assert_eq!(msg.downcast_ref::<&str>(), Some(&"a real failure"));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "not run again");
    }

    #[tokio::test]
    async fn every_attempt_rolling_over_gives_up_after_the_last() {
        let db = Arc::new(TestDb::sqlite().await);
        let runs = Arc::new(AtomicU32::new(0));
        let (d, r) = (db.clone(), runs.clone());
        let failed = tokio::spawn(async move {
            let r = &r;
            in_one_window(d.store(), W, W / 2, |_| async move {
                r.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(W + Duration::from_millis(50)).await;
                panic!("always too slow");
            })
            .await;
        })
        .await
        .unwrap_err();
        assert!(failed.is_panic());
        assert_eq!(runs.load(Ordering::SeqCst), WINDOW_ATTEMPTS);
    }

    #[tokio::test]
    async fn a_pass_runs_once() {
        let db = TestDb::sqlite().await;
        let runs = AtomicU32::new(0);
        let runs_ref = &runs;
        in_one_window(db.store(), W, W / 2, |_| async move {
            runs_ref.fetch_add(1, Ordering::SeqCst);
        })
        .await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
}
