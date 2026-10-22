//! The dependency index and the invalidation epoch log.
//!
//! All state lives behind one `std::sync::Mutex`, which is never held across an
//! `.await` and never while calling into moka (moka may run the eviction listener,
//! which takes this lock, from inside its own calls).
//!
//! **Staleness guard.** Two windows could leave a stale page cached:
//!
//! 1. An invalidation arrives while the page is being rendered (the render read the old
//!    data). Every render takes a [`RenderGuard`] that records the epoch at its start;
//!    every invalidation bumps the epoch and, while renders are in flight, is appended
//!    to a log. [`State::register`] refuses a page if any logged invalidation newer than
//!    its start epoch names one of its deps (or `all`). The log is pruned to the oldest
//!    in-flight start, so it is empty whenever nothing renders.
//! 2. An invalidation arrives after registration but before moka stores the value.
//!    Each entry carries a `valid` flag that invalidation clears through the index;
//!    lookups treat a cleared entry as a miss, so a late insert is never served.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::page::{CacheKey, CachedPage};

/// The flush-everything dependency key.
pub const ALL_KEY: &str = "all";

/// What moka stores: the page plus the identity and validity of its registration.
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) id: u64,
    pub(crate) valid: Arc<AtomicBool>,
    pub(crate) page: Arc<CachedPage>,
}

impl Entry {
    pub(crate) fn is_valid(&self) -> bool {
        self.valid.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
struct Registration {
    id: u64,
    deps: Vec<String>,
    valid: Arc<AtomicBool>,
}

#[derive(Debug)]
enum Logged {
    All,
    Keys(Vec<String>),
}

#[derive(Debug, Default)]
struct Inner {
    epoch: u64,
    next_id: u64,
    by_dep: HashMap<String, HashSet<CacheKey>>,
    by_key: HashMap<CacheKey, Registration>,
    /// Start epochs of in-flight renders (multiset).
    in_flight: BTreeMap<u64, usize>,
    /// Invalidations newer than the oldest in-flight render.
    log: VecDeque<(u64, Logged)>,
}

impl Inner {
    fn unindex(&mut self, key: &CacheKey) {
        let Some(reg) = self.by_key.remove(key) else {
            return;
        };
        reg.valid.store(false, Ordering::Release);
        for dep in &reg.deps {
            if let Some(set) = self.by_dep.get_mut(dep) {
                set.remove(key);
                if set.is_empty() {
                    self.by_dep.remove(dep);
                }
            }
        }
    }

    fn log(&mut self, what: Logged) {
        self.epoch += 1;
        if !self.in_flight.is_empty() {
            self.log.push_back((self.epoch, what));
        }
    }

    fn prune_log(&mut self) {
        match self.in_flight.keys().next().copied() {
            None => self.log.clear(),
            Some(oldest) => {
                while self.log.front().is_some_and(|(e, _)| *e <= oldest) {
                    self.log.pop_front();
                }
            }
        }
    }
}

/// Shared index state; cheap to clone.
#[derive(Debug, Clone, Default)]
pub(crate) struct State(Arc<Mutex<Inner>>);

impl State {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock cannot leave the maps half-updated in a way
        // that serves stale pages (flags are cleared before maps change), so recover.
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Starts a render: invalidations from now on are logged against it.
    pub(crate) fn begin_render(&self) -> RenderGuard {
        let mut g = self.lock();
        let start = g.epoch;
        *g.in_flight.entry(start).or_insert(0) += 1;
        RenderGuard {
            state: self.clone(),
            start,
        }
    }

    /// Registers `page` under `key` for a render that started at `guard`, replacing any
    /// earlier registration of `key`. `None` if an invalidation since the render
    /// started touches the page's deps (the page may be stale and must not be cached).
    pub(crate) fn register(
        &self,
        key: &CacheKey,
        page: Arc<CachedPage>,
        guard: &RenderGuard,
    ) -> Option<Entry> {
        let mut g = self.lock();
        let stale = g.log.iter().any(|(epoch, what)| {
            *epoch > guard.start
                && match what {
                    Logged::All => true,
                    Logged::Keys(keys) => keys.iter().any(|k| page.deps.contains(k)),
                }
        });
        if stale {
            return None;
        }
        g.unindex(key);
        g.next_id += 1;
        let id = g.next_id;
        let valid = Arc::new(AtomicBool::new(true));
        let mut deps = page.deps.clone();
        deps.sort();
        deps.dedup();
        for dep in &deps {
            g.by_dep.entry(dep.clone()).or_default().insert(key.clone());
        }
        g.by_key.insert(
            key.clone(),
            Registration {
                id,
                deps,
                valid: valid.clone(),
            },
        );
        Some(Entry { id, valid, page })
    }

    /// Drops every registration whose deps intersect `keys` and returns their cache
    /// keys (to be removed from moka). `all` in `keys` drops everything; the caller
    /// then also clears moka.
    pub(crate) fn invalidate(&self, keys: &[String]) -> Invalidated {
        let mut g = self.lock();
        if keys.iter().any(|k| k == ALL_KEY) {
            Self::flush_locked(&mut g);
            return Invalidated::All;
        }
        g.log(Logged::Keys(keys.to_vec()));
        let mut hit: HashSet<CacheKey> = HashSet::new();
        for dep in keys {
            if let Some(set) = g.by_dep.get(dep) {
                hit.extend(set.iter().cloned());
            }
        }
        for key in &hit {
            g.unindex(key);
        }
        Invalidated::Keys(hit.into_iter().collect())
    }

    /// Drops every registration.
    pub(crate) fn flush(&self) {
        Self::flush_locked(&mut self.lock());
    }

    fn flush_locked(g: &mut Inner) {
        g.log(Logged::All);
        for reg in g.by_key.values() {
            reg.valid.store(false, Ordering::Release);
        }
        g.by_key.clear();
        g.by_dep.clear();
    }

    /// moka removed the entry `id` stored under `key` (size, expiry, explicit or
    /// replacement): drop its registration unless a newer entry owns the key.
    pub(crate) fn evicted(&self, key: &CacheKey, id: u64) {
        let mut g = self.lock();
        if g.by_key.get(key).is_some_and(|r| r.id == id) {
            g.unindex(key);
        }
    }

    /// (registered cache keys, dep keys, cache keys referenced from the dep map), for
    /// tests checking that the index has no dangling entries.
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> (HashSet<CacheKey>, HashSet<String>, HashSet<CacheKey>) {
        let g = self.lock();
        let keys = g.by_key.keys().cloned().collect();
        let deps = g.by_dep.keys().cloned().collect();
        let referenced = g.by_dep.values().flatten().cloned().collect();
        (keys, deps, referenced)
    }

    #[cfg(test)]
    pub(crate) fn log_len(&self) -> usize {
        self.lock().log.len()
    }
}

/// Result of [`State::invalidate`].
pub(crate) enum Invalidated {
    All,
    Keys(Vec<CacheKey>),
}

/// An in-flight render; dropping it ends the render (and lets the log shrink).
pub(crate) struct RenderGuard {
    state: State,
    start: u64,
}

impl Drop for RenderGuard {
    fn drop(&mut self) {
        let mut g = self.state.lock();
        if let Some(n) = g.in_flight.get_mut(&self.start) {
            *n -= 1;
            if *n == 0 {
                g.in_flight.remove(&self.start);
            }
        }
        g.prune_log();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;

    use super::*;

    fn page(deps: &[&str]) -> Arc<CachedPage> {
        Arc::new(CachedPage {
            status: 200,
            headers: vec![],
            body: Bytes::from_static(b"x"),
            etag: "\"x\"".into(),
            deps: deps.iter().map(|d| (*d).to_owned()).collect(),
            ttl: None::<Duration>,
        })
    }

    fn key(path: &str) -> CacheKey {
        CacheKey::anon("h", path, "")
    }

    #[test]
    fn invalidation_during_render_refuses_registration() {
        let s = State::default();
        let g = s.begin_render();
        s.invalidate(&["post:1".into()]);
        assert!(s.register(&key("/a/"), page(&["post:1"]), &g).is_none());
        // Unrelated deps are fine.
        assert!(s.register(&key("/b/"), page(&["post:2"]), &g).is_some());
        // A render that starts after the invalidation is not affected.
        let g2 = s.begin_render();
        assert!(s.register(&key("/a/"), page(&["post:1"]), &g2).is_some());
    }

    #[test]
    fn flush_during_render_refuses_everything() {
        let s = State::default();
        let g = s.begin_render();
        s.invalidate(&["all".into()]);
        assert!(s.register(&key("/a/"), page(&["post:9"]), &g).is_none());
    }

    #[test]
    fn log_is_empty_without_renders() {
        let s = State::default();
        s.invalidate(&["post:1".into()]);
        assert_eq!(s.log_len(), 0);
        let g = s.begin_render();
        s.invalidate(&["post:1".into()]);
        assert_eq!(s.log_len(), 1);
        drop(g);
        assert_eq!(s.log_len(), 0);
    }

    #[test]
    fn invalidation_clears_flag_of_registered_entry() {
        let s = State::default();
        let g = s.begin_render();
        let e = s.register(&key("/a/"), page(&["post:1"]), &g).unwrap();
        drop(g);
        assert!(e.is_valid());
        s.invalidate(&["post:1".into()]);
        assert!(!e.is_valid());
        let (keys, deps, refs) = s.snapshot();
        assert!(keys.is_empty() && deps.is_empty() && refs.is_empty());
    }

    #[test]
    fn re_registration_replaces_deps_and_ignores_stale_eviction() {
        let s = State::default();
        let g = s.begin_render();
        let old = s.register(&key("/a/"), page(&["post:1"]), &g).unwrap();
        let new = s.register(&key("/a/"), page(&["post:2"]), &g).unwrap();
        assert!(!old.is_valid());
        // moka reports the old value as replaced: the new registration stays.
        s.evicted(&key("/a/"), old.id);
        let (keys, deps, _) = s.snapshot();
        assert_eq!(keys.len(), 1);
        assert_eq!(deps, HashSet::from(["post:2".to_owned()]));
        s.evicted(&key("/a/"), new.id);
        let (keys, deps, refs) = s.snapshot();
        assert!(keys.is_empty() && deps.is_empty() && refs.is_empty());
    }
}
