//! Per-call store state: WASI context, resource limits, import budgets, and the host
//! implementations of the `ferrite:plugin` imports (design 05 §6).

use std::sync::Arc;

use wasmtime::component::ResourceTable;
use wasmtime::{ResourceLimiter, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::bindings::v0_1::ferrite::plugin::{
    content, http, kv, log, mail, meta, options, settings, types as wit,
};
use crate::manifest::Capabilities;
use crate::output::{GuestOutput, sanitize_log_message};
use crate::registry::SettingDefaults;
use crate::services::HostServices;
use crate::trace::{CallTrace, TracingWallClock};

/// Per-store instance/table/memory caps: the measured minimum for components built with
/// `cargo build --target wasm32-wasip2` (wit-component output). One less instance or
/// table fails instantiation with "instance count too high" / "table count too high".
/// Also used to size the pooling allocator (`engine.rs`), hence `u32`.
pub(crate) const MAX_INSTANCES: u32 = 3;
pub(crate) const MAX_TABLES: u32 = 2;
pub(crate) const MAX_MEMORIES: u32 = 1;
/// Largest size any one table may reach. Without it, `table.grow` on an unbounded table
/// allocates host memory outside the linear-memory cap. hello needs between 81 and 90
/// elements (measured), so 10_000 leaves >100x headroom.
pub(crate) const MAX_TABLE_ELEMENTS: usize = 10_000;

/// Most live host resources (WASI streams, pollables, …) one call may hold: caps both the
/// `ResourceTable` and wasmtime's store-wide component handle count. Each entry is host
/// memory outside the linear-memory cap; wasmtime's defaults are 1_000_000 and 100_000.
/// hello holds fewer than 10.
pub(crate) const MAX_RESOURCES: usize = 1_000;
/// Largest `wasi:random` `get-random-bytes` / `get-insecure-random-bytes` request; the
/// host allocates the whole buffer before copying it into the guest. Larger requests
/// fail the call. (wasmtime-wasi's default is 64 MiB.)
const MAX_RANDOM_BYTES: u64 = 64 * 1024;

/// Largest value one `meta.set` may write.
const MAX_META_VALUE: usize = 64 * 1024;

/// Upper bound on `limit` for list imports, whatever the guest asks for.
const MAX_LIST: u32 = 100;
const BUDGET_EXCEEDED: &str = "budget exceeded";
const UNAVAILABLE: &str = "unavailable in this build";
const RENDER_WRITE: &str = "meta.set is not allowed during render filters";

/// Host-import budget for one call (design 05 §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Budget {
    /// `content`, `meta`, `options` and `settings` calls (all storage reads/writes).
    content: u32,
    kv_ops: u32,
    /// Bytes (key + value) `kv.set` may still write.
    kv_bytes: usize,
    /// Bytes (post id + key + value) `meta.set` may still write.
    meta_bytes: usize,
    http: u32,
}

impl Budget {
    fn per_call() -> Self {
        Self {
            content: 100,
            kv_ops: 100,
            kv_bytes: 1024 * 1024,
            meta_bytes: 1024 * 1024,
            http: 3,
        }
    }
}

/// Take one unit from `counter`; `false` when it is already exhausted.
fn take(counter: &mut u32) -> bool {
    match counter.checked_sub(1) {
        Some(left) => {
            *counter = left;
            true
        }
        None => false,
    }
}

/// Manifest key patterns (validated by `manifest::valid_pattern`): `"*"` matches every
/// key, `"<prefix>.*"` matches `<prefix>.` followed by anything, anything else matches
/// exactly.
fn matches_any(patterns: &[String], key: &str) -> bool {
    patterns.iter().any(|p| {
        if p == "*" {
            return true;
        }
        match p.strip_suffix(".*") {
            Some(prefix) => key
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.')),
            None => p == key,
        }
    })
}

/// `StoreLimits` plus a record of whether a memory or table grow was refused. A guest
/// that cannot allocate aborts with a plain trap, so this flag is the only way to tell
/// a memory-limit failure from other traps.
pub(crate) struct CallLimits {
    inner: StoreLimits,
    grow_denied: bool,
}

impl CallLimits {
    pub(crate) fn grow_denied(&self) -> bool {
        self.grow_denied
    }

    fn note(&mut self, allowed: wasmtime::Result<bool>) -> wasmtime::Result<bool> {
        if !matches!(allowed, Ok(true)) {
            self.grow_denied = true;
        }
        allowed
    }
}

impl ResourceLimiter for CallLimits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let allowed = self.inner.memory_growing(current, desired, maximum);
        self.note(allowed)
    }

    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.inner.memory_grow_failed(error)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let allowed = self.inner.table_growing(current, desired, maximum);
        self.note(allowed)
    }

    fn table_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.inner.table_grow_failed(error)
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

/// State of one store, i.e. one plugin call.
pub(crate) struct HostState {
    plugin: Arc<str>,
    grants: Arc<Capabilities>,
    /// The manifest's `[settings]` keys and defaults: `settings.get` answers these only.
    settings: Arc<SettingDefaults>,
    services: Arc<dyn HostServices>,
    pub(crate) limits: CallLimits,
    budget: Budget,
    pub(crate) wasi: WasiCtx,
    /// Holds the capture pipes (`wasi` has Arc clones) and logs their contents when
    /// dropped; `HostState` is dropped at the end of every call.
    output: GuestOutput,
    table: ResourceTable,
    /// Dependency keys and volatility of this call (design 05 §7a, `trace.rs`).
    trace: CallTrace,
    /// A render-class filter call (ruling R20): storage writes are refused, since a
    /// page view must not change data (and cached pages would hide the effect).
    render_path: bool,
    /// Whether this call already logged a refused render-path write.
    refusal_logged: bool,
}

impl HostState {
    pub(crate) fn new(
        plugin: Arc<str>,
        grants: Arc<Capabilities>,
        settings: Arc<SettingDefaults>,
        services: Arc<dyn HostServices>,
        memory_bytes: usize,
        trace: CallTrace,
        render_path: bool,
    ) -> Self {
        let limits = StoreLimitsBuilder::new()
            .memory_size(memory_bytes)
            // u32 -> usize is lossless on every target wasmtime supports.
            .instances(MAX_INSTANCES as usize)
            .tables(MAX_TABLES as usize)
            .memories(MAX_MEMORIES as usize)
            .table_elements(MAX_TABLE_ELEMENTS)
            .build();
        let limits = CallLimits {
            inner: limits,
            grow_denied: false,
        };
        let output = GuestOutput::new(plugin.clone());
        let mut wasi = WasiCtx::builder();
        // No preopens, no env, no args; stdin closed. stdout/stderr are captured with a
        // cap and logged when the call ends.
        wasi.stdout(output.stdout.clone())
            .stderr(output.stderr.clone())
            .max_random_size(MAX_RANDOM_BYTES)
            .wall_clock(TracingWallClock::new(trace.clone()));
        let mut table = ResourceTable::new();
        table.set_max_capacity(MAX_RESOURCES);
        Self {
            plugin,
            grants,
            settings,
            services,
            limits,
            budget: Budget::per_call(),
            wasi: wasi.build(),
            output,
            table,
            trace,
            render_path,
            refusal_logged: false,
        }
    }

    fn content_type_granted(&self, kind: &str) -> bool {
        matches_any(&self.grants.content_read, kind)
    }

    fn own_meta_key(&self, key: &str) -> bool {
        key.strip_prefix("plugin.")
            .and_then(|rest| rest.strip_prefix(&*self.plugin))
            .is_some_and(|rest| rest.starts_with('.'))
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl wasmtime_wasi_io::IoView for HostState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl wit::Host for HostState {}

impl log::Host for HostState {
    async fn log(&mut self, level: log::Level, message: String) {
        if !self.output.admit_log() {
            return;
        }
        let message = sanitize_log_message(&message);
        let plugin = &*self.plugin;
        match level {
            log::Level::Trace => tracing::trace!(plugin, "{message}"),
            log::Level::Debug => tracing::debug!(plugin, "{message}"),
            log::Level::Info => tracing::info!(plugin, "{message}"),
            log::Level::Warn => tracing::warn!(plugin, "{message}"),
            log::Level::Error => tracing::error!(plugin, "{message}"),
        }
    }
}

impl content::Host for HostState {
    async fn get_post(&mut self, id: String) -> Option<wit::Post> {
        if !take(&mut self.budget.content) {
            return None;
        }
        // Recorded found or not: publishing the item later must refresh the page.
        self.trace.post(&id);
        let grants = Arc::clone(&self.grants);
        let kind_ok = move |kind: &str| matches_any(&grants.content_read, kind);
        let post = self.services.get_post(&id, &kind_ok).await?;
        // Re-check: a services implementation must not widen the granted types.
        self.content_type_granted(&post.kind).then(|| {
            self.trace.post(&post.id);
            post.into()
        })
    }

    async fn get_post_by_slug(&mut self, kind: String, slug: String) -> Option<wit::Post> {
        if !take(&mut self.budget.content) || !self.content_type_granted(&kind) {
            return None;
        }
        // A missing slug may appear when an item of `kind` is created or published.
        self.trace.archive(&kind);
        let post = self.services.get_post_by_slug(&kind, &slug).await?;
        // Re-check: a services implementation must not widen the requested type.
        self.content_type_granted(&post.kind).then(|| {
            self.trace.post(&post.id);
            post.into()
        })
    }

    async fn list_posts(&mut self, kind: String, limit: u32, offset: u32) -> Vec<wit::Post> {
        if !take(&mut self.budget.content) || !self.content_type_granted(&kind) {
            return Vec::new();
        }
        self.trace.archive(&kind);
        let posts = self
            .services
            .list_posts(&kind, limit.min(MAX_LIST), offset)
            .await;
        posts
            .into_iter()
            .filter(|p| self.content_type_granted(&p.kind))
            .map(|p| {
                self.trace.post(&p.id);
                p.into()
            })
            .collect()
    }
}

impl meta::Host for HostState {
    async fn get(&mut self, post: String, key: String) -> Option<String> {
        let readable = matches_any(&self.grants.content_meta_read, &key)
            || matches_any(&self.grants.content_meta_write, &key)
            || self.own_meta_key(&key);
        if !take(&mut self.budget.content) || !readable {
            return None;
        }
        self.trace.post(&post);
        self.services.meta_get(&post, &key).await
    }

    async fn set(&mut self, post: String, key: String, value: String) -> Result<(), String> {
        if self.render_path {
            if !self.refusal_logged {
                self.refusal_logged = true;
                tracing::warn!(plugin = %self.plugin, key, "meta.set refused during a render filter");
            }
            return Err(RENDER_WRITE.into());
        }
        if !take(&mut self.budget.content) {
            return Err(BUDGET_EXCEEDED.into());
        }
        if !(matches_any(&self.grants.content_meta_write, &key) || self.own_meta_key(&key)) {
            return Err(format!("meta key `{key}` is not writable by this plugin"));
        }
        if value.len() > MAX_META_VALUE {
            return Err(format!(
                "meta value is {} bytes, limit {MAX_META_VALUE}",
                value.len()
            ));
        }
        let size = post.len() + key.len() + value.len();
        match self.budget.meta_bytes.checked_sub(size) {
            Some(left) => self.budget.meta_bytes = left,
            None => return Err(BUDGET_EXCEEDED.into()),
        }
        self.services.meta_set(&post, &key, &value).await
    }
}

impl options::Host for HostState {
    async fn get(&mut self, key: String) -> Option<String> {
        if !take(&mut self.budget.content) || !matches_any(&self.grants.options_read, &key) {
            return None;
        }
        self.trace.option(&key);
        self.services.option_get(&key).await
    }
}

impl kv::Host for HostState {
    async fn get(&mut self, key: String) -> Option<Vec<u8>> {
        self.trace.volatile();
        if !take(&mut self.budget.kv_ops) {
            return None;
        }
        self.services.kv_get(&self.plugin, &key).await
    }

    async fn set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
        self.trace.volatile();
        if !take(&mut self.budget.kv_ops) {
            return Err(BUDGET_EXCEEDED.into());
        }
        let size = key.len() + value.len();
        match self.budget.kv_bytes.checked_sub(size) {
            Some(left) => self.budget.kv_bytes = left,
            None => return Err(BUDGET_EXCEEDED.into()),
        }
        self.services.kv_set(&self.plugin, &key, value).await
    }

    async fn delete(&mut self, key: String) {
        self.trace.volatile();
        if take(&mut self.budget.kv_ops) {
            self.services.kv_delete(&self.plugin, &key).await;
        }
    }

    async fn list_keys(&mut self, prefix: String, limit: u32) -> Vec<String> {
        self.trace.volatile();
        if !take(&mut self.budget.kv_ops) {
            return Vec::new();
        }
        self.services
            .kv_list(&self.plugin, &prefix, limit.min(MAX_LIST))
            .await
    }
}

/// Only keys the manifest declares (P13), read in the calling plugin's own namespace
/// (the services key the read by plugin name); with nothing stored, the declared
/// default. An undeclared key costs no budget.
impl settings::Host for HostState {
    async fn get(&mut self, key: String) -> Option<String> {
        let default = self.settings.get(&key)?.clone();
        if !take(&mut self.budget.content) {
            return None;
        }
        match self.services.setting_get(&self.plugin, &key).await {
            Some(value) => Some(value),
            None => default,
        }
    }
}

impl http::Host for HostState {
    async fn fetch(&mut self, _req: http::Request) -> Result<http::Response, String> {
        self.trace.volatile();
        if !take(&mut self.budget.http) {
            return Err(BUDGET_EXCEEDED.into());
        }
        // Real implementation (allow-list, SSRF checks) lands in M1.
        Err(UNAVAILABLE.into())
    }
}

impl mail::Host for HostState {
    async fn notify_admins(&mut self, _subject: String, _text_body: String) -> Result<(), String> {
        // Real implementation lands in M2.
        Err(UNAVAILABLE.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::InMemoryServices;
    use crate::types::Post;
    use kv::Host as _;

    fn state(grants: Capabilities, services: Arc<dyn HostServices>) -> HostState {
        HostState::new(
            "tester".into(),
            Arc::new(grants),
            Arc::default(),
            services,
            1024 * 1024,
            CallTrace::default(),
            false,
        )
    }

    #[tokio::test]
    async fn kv_budget() {
        let services = Arc::new(InMemoryServices::new());
        services
            .kv_set("tester", "k", b"v".to_vec())
            .await
            .expect("seed");
        let caps = Capabilities {
            kv: true,
            ..Capabilities::default()
        };
        let mut s = state(caps, services);
        for i in 0..100 {
            assert_eq!(s.get("k".into()).await, Some(b"v".to_vec()), "op {i}");
        }
        assert_eq!(s.get("k".into()).await, None);
        assert_eq!(s.budget.kv_ops, 0, "budget exhausted");
        assert_eq!(
            s.set("k".into(), b"x".to_vec()).await,
            Err(BUDGET_EXCEEDED.into())
        );
    }

    #[tokio::test]
    async fn kv_bytes_budget() {
        let caps = Capabilities {
            kv: true,
            ..Capabilities::default()
        };
        let mut s = state(caps, Arc::new(InMemoryServices::new()));
        assert_eq!(s.set("a".into(), vec![0; 1024 * 1024 - 1]).await, Ok(()));
        assert_eq!(
            s.set("b".into(), vec![0; 1]).await,
            Err(BUDGET_EXCEEDED.into())
        );
    }

    #[tokio::test]
    async fn meta_value_cap() {
        let mut s = state(Capabilities::default(), Arc::new(InMemoryServices::new()));
        let key = || "plugin.tester.k".to_string();
        assert_eq!(
            meta::Host::set(&mut s, "p".into(), key(), "v".repeat(64 * 1024)).await,
            Ok(())
        );
        assert!(
            meta::Host::set(&mut s, "p".into(), key(), "v".repeat(64 * 1024 + 1))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn meta_bytes_budget() {
        let services = Arc::new(InMemoryServices::new());
        let mut s = state(Capabilities::default(), services.clone());
        let value = "v".repeat(64 * 1024);
        // 15 x (64 KiB + post + key) fits in 1 MiB; a 16th full value does not.
        for i in 0..15 {
            let key = format!("plugin.tester.k{i:02}");
            assert_eq!(
                meta::Host::set(&mut s, "p".into(), key, value.clone()).await,
                Ok(()),
                "{i}"
            );
        }
        assert_eq!(
            meta::Host::set(&mut s, "p".into(), "plugin.tester.k15".into(), value).await,
            Err(BUDGET_EXCEEDED.into())
        );
        assert_eq!(services.meta_get("p", "plugin.tester.k15").await, None);
        // The refused write did not use up the rest of the budget.
        assert_eq!(
            meta::Host::set(&mut s, "p".into(), "plugin.tester.small".into(), "x".into()).await,
            Ok(())
        );
    }

    fn post(id: &str, kind: &str) -> Post {
        Post {
            id: id.into(),
            kind: kind.into(),
            slug: id.into(),
            title: String::new(),
            excerpt: String::new(),
            status: "publish".into(),
            published: None,
            url: String::new(),
            author: String::new(),
            terms: Vec::new(),
        }
    }

    #[tokio::test]
    async fn content_read_only_sees_granted_types() {
        use content::Host as _;
        let services = Arc::new(InMemoryServices::new());
        services.insert_post(post("p1", "post"));
        services.insert_post(post("g1", "page"));
        let caps = Capabilities {
            content_read: vec!["post".into()],
            ..Capabilities::default()
        };
        let mut s = state(caps, services.clone());
        assert!(s.get_post("p1".into()).await.is_some());
        assert!(s.get_post("g1".into()).await.is_none());
        assert!(
            s.get_post_by_slug("page".into(), "g1".into())
                .await
                .is_none()
        );
        assert!(s.list_posts("page".into(), 10, 0).await.is_empty());
        assert_eq!(s.list_posts("post".into(), 10, 0).await.len(), 1);

        let all = Capabilities {
            content_read: vec!["*".into()],
            ..Capabilities::default()
        };
        let mut s = state(all, services);
        assert!(s.get_post("g1".into()).await.is_some());
    }

    /// A component exporting `grow(n)`, which runs `table.grow` by `n` on an unbounded
    /// funcref table and returns the old size, or -1 if the grow was refused.
    const TABLE_GROWER: &str = r#"
        (component
          (core module $m
            (table $t 1 funcref)
            (func (export "grow") (param i32) (result i32)
              (table.grow $t (ref.null func) (local.get 0))))
          (core instance $i (instantiate $m))
          (func (export "grow") (param "n" u32) (result s32)
            (canon lift (core func $i "grow"))))
    "#;

    #[tokio::test]
    async fn table_growth_is_bounded() {
        use crate::config::HostConfig;
        use crate::engine::build_engine;
        use wasmtime::Store;
        use wasmtime::component::{Component, Linker};

        let engine = build_engine(&HostConfig::default()).expect("engine");
        let component = Component::new(&engine, TABLE_GROWER).expect("wat component");
        let linker = Linker::<HostState>::new(&engine);
        let mut store = Store::new(
            &engine,
            state(Capabilities::default(), Arc::new(InMemoryServices::new())),
        );
        store.limiter(|s| &mut s.limits);
        store.set_fuel(1_000_000).expect("fuel");
        store.set_epoch_deadline(1_000); // no ticker runs in this test
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .expect("instantiate");
        let grow = instance
            .get_typed_func::<(u32,), (i32,)>(&mut store, "grow")
            .expect("grow export");
        let mut call = async |n: u32| grow.call_async(&mut store, (n,)).await.expect("no trap").0;
        // Refused without allocating: 100 M elements would be ~800 MB of host memory.
        assert_eq!(call(100_000_000).await, -1);
        assert_eq!(call(MAX_TABLE_ELEMENTS as u32).await, -1, "1 + max > max");
        // The store is still usable and small growth within the cap succeeds.
        assert_eq!(call(10).await, 1);
    }

    #[test]
    fn resource_table_is_capped() {
        let mut s = state(Capabilities::default(), Arc::new(InMemoryServices::new()));
        let table = wasmtime_wasi_io::IoView::table(&mut s);
        for i in 0..MAX_RESOURCES {
            assert!(table.push(i).is_ok(), "entry {i}");
        }
        assert!(table.push(0usize).is_err(), "entry past the cap refused");
    }

    #[test]
    fn random_requests_are_capped() {
        use wasmtime_wasi::p2::bindings::random::{insecure, random};
        let mut s = state(Capabilities::default(), Arc::new(InMemoryServices::new()));
        let ctx = s.wasi.random();
        let max = MAX_RANDOM_BYTES;
        assert_eq!(
            random::Host::get_random_bytes(ctx, max)
                .map(|b| b.len())
                .ok(),
            Some(64 * 1024)
        );
        assert!(random::Host::get_random_bytes(ctx, max + 1).is_err());
        assert!(random::Host::get_random_bytes(ctx, u64::MAX).is_err());
        assert!(insecure::Host::get_insecure_random_bytes(ctx, max + 1).is_err());
    }

    #[test]
    fn key_patterns() {
        let pats = vec!["seo.*".to_string(), "site.title".to_string()];
        assert!(matches_any(&pats, "seo.title"));
        assert!(matches_any(&pats, "seo.a.b"));
        assert!(matches_any(&pats, "site.title"));
        assert!(!matches_any(&pats, "site.titles"));
        assert!(!matches_any(&pats, "seox"));
        assert!(!matches_any(&pats, "seo"));
        assert!(!matches_any(&pats, "xseo.title"));
        assert!(matches_any(&["*".to_string()], "anything"));
        // Invalid forms (rejected by manifest validation) never act as globs.
        let bad = vec!["seo*".to_string(), "*.title".to_string()];
        assert!(!matches_any(&bad, "seox"));
        assert!(!matches_any(&bad, "site.title"));
    }

    #[tokio::test]
    async fn list_limits_are_clamped() {
        use content::Host as _;
        let services = Arc::new(InMemoryServices::new());
        for i in 0..150 {
            services.insert_post(post(&format!("p{i:03}"), "post"));
            services
                .kv_set("tester", &format!("k{i:03}"), Vec::new())
                .await
                .expect("seed");
        }
        let caps = Capabilities {
            content_read: vec!["post".into()],
            kv: true,
            ..Capabilities::default()
        };
        let mut s = state(caps, services);
        assert_eq!(s.list_posts("post".into(), u32::MAX, 0).await.len(), 100);
        assert_eq!(s.list_posts("post".into(), 7, 0).await.len(), 7);
        assert_eq!(
            kv::Host::list_keys(&mut s, "k".into(), u32::MAX)
                .await
                .len(),
            100
        );
    }

    /// P13: `settings.get` answers only keys the manifest declares, with the declared
    /// default (JSON text) when nothing is stored, else the stored value; it reads the
    /// calling plugin's namespace only.
    #[tokio::test]
    async fn settings_get_defaults_and_undeclared() {
        let manifest = crate::Manifest::parse(
            "[plugin]\nname = \"tester\"\nversion = \"0.1.0\"\npublisher = \"x.dev\"\n\
             world = \"ferrite:plugin/plugin@0.1.0\"\n[settings]\n\
             title = { type = \"text\", label = \"T\", default = \"Hi \\\"you\\\"\" }\n\
             limit = { type = \"number\", label = \"L\", default = 2.5 }\n\
             on = { type = \"bool\", label = \"O\", default = true }\n\
             api_key = { type = \"secret\", label = \"K\" }\n",
        )
        .expect("manifest");
        let defaults = crate::registry::setting_defaults(&manifest);
        let services = Arc::new(InMemoryServices::new());
        // Stored for undeclared keys and for other plugins: never answered.
        services.set_setting("tester", "undeclared", "\"x\"");
        services.set_setting("other", "title", "\"theirs\"");
        services.set_setting("other", "api_key", "\"their-secret\"");
        let mut s = HostState::new(
            "tester".into(),
            Arc::new(Capabilities::default()),
            Arc::new(defaults),
            services.clone(),
            1024 * 1024,
            CallTrace::default(),
            false,
        );
        assert_eq!(
            settings::Host::get(&mut s, "title".into()).await.as_deref(),
            Some("\"Hi \\\"you\\\"\"")
        );
        assert_eq!(
            settings::Host::get(&mut s, "limit".into()).await.as_deref(),
            Some("2.5")
        );
        assert_eq!(
            settings::Host::get(&mut s, "on".into()).await.as_deref(),
            Some("true")
        );
        assert_eq!(
            settings::Host::get(&mut s, "api_key".into()).await,
            None,
            "no default, nothing stored"
        );
        assert_eq!(settings::Host::get(&mut s, "undeclared".into()).await, None);
        assert_eq!(settings::Host::get(&mut s, "".into()).await, None);
        // Stored values win over the default.
        services.set_setting("tester", "title", "\"Stored\"");
        services.set_setting("tester", "api_key", "\"mine\"");
        assert_eq!(
            settings::Host::get(&mut s, "title".into()).await.as_deref(),
            Some("\"Stored\"")
        );
        assert_eq!(
            settings::Host::get(&mut s, "api_key".into())
                .await
                .as_deref(),
            Some("\"mine\"")
        );
        // Undeclared keys cost no budget; declared ones do.
        let left = s.budget.content;
        assert_eq!(settings::Host::get(&mut s, "undeclared".into()).await, None);
        assert_eq!(s.budget.content, left);
        while s.budget.content > 0 {
            settings::Host::get(&mut s, "title".into()).await;
        }
        assert_eq!(
            settings::Host::get(&mut s, "title".into()).await,
            None,
            "budget exhausted"
        );
    }

    #[test]
    fn own_meta_namespace() {
        let s = state(Capabilities::default(), Arc::new(InMemoryServices::new()));
        assert!(s.own_meta_key("plugin.tester.x"));
        assert!(!s.own_meta_key("plugin.testerx.x"));
        assert!(!s.own_meta_key("plugin.other.x"));
    }
}
