//! Per-call store state: WASI context, resource limits, import budgets, and the host
//! implementations of the `ferrite:plugin` imports (design 05 §6).

use std::sync::Arc;

use wasmtime::component::ResourceTable;
use wasmtime::{StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::bindings::v0_1::ferrite::plugin::{
    content, http, kv, log, mail, meta, options, settings, types as wit,
};
use crate::manifest::Capabilities;
use crate::services::HostServices;

/// Per-store instance/table/memory caps: the measured minimum for components built with
/// `cargo build --target wasm32-wasip2` (wit-component output). One less instance or
/// table fails instantiation with "instance count too high" / "table count too high".
const MAX_INSTANCES: usize = 3;
const MAX_TABLES: usize = 2;
const MAX_MEMORIES: usize = 1;

const BUDGET_EXCEEDED: &str = "budget exceeded";
const UNAVAILABLE: &str = "unavailable in this build";

/// Host-import budget for one call (design 05 §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Budget {
    /// `content`, `meta`, `options` and `settings` calls (all storage reads/writes).
    content: u32,
    kv_ops: u32,
    /// Bytes (key + value) `kv.set` may still write.
    kv_bytes: usize,
    http: u32,
}

impl Budget {
    fn per_call() -> Self {
        Self {
            content: 100,
            kv_ops: 100,
            kv_bytes: 1024 * 1024,
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

/// Manifest-style patterns: `"*"` matches everything, `"prefix.*"` matches keys starting
/// with `prefix.`, anything else matches exactly.
fn matches_any(patterns: &[String], key: &str) -> bool {
    patterns.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => p == key,
    })
}

/// State of one store, i.e. one plugin call.
pub(crate) struct HostState {
    plugin: Arc<str>,
    grants: Arc<Capabilities>,
    services: Arc<dyn HostServices>,
    pub(crate) limits: StoreLimits,
    budget: Budget,
    wasi: WasiCtx,
    table: ResourceTable,
}

impl HostState {
    pub(crate) fn new(
        plugin: Arc<str>,
        grants: Arc<Capabilities>,
        services: Arc<dyn HostServices>,
        memory_bytes: usize,
    ) -> Self {
        let limits = StoreLimitsBuilder::new()
            .memory_size(memory_bytes)
            .instances(MAX_INSTANCES)
            .tables(MAX_TABLES)
            .memories(MAX_MEMORIES)
            .build();
        Self {
            plugin,
            grants,
            services,
            limits,
            budget: Budget::per_call(),
            // No preopens, no env, no args; stdin closed, stdout/stderr discarded
            // (wasmtime-wasi's builder defaults).
            wasi: WasiCtx::builder().build(),
            table: ResourceTable::new(),
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

impl wit::Host for HostState {}

impl log::Host for HostState {
    async fn log(&mut self, level: log::Level, message: String) {
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
        let post = self.services.get_post(&id).await?;
        self.content_type_granted(&post.kind).then(|| post.into())
    }

    async fn get_post_by_slug(&mut self, kind: String, slug: String) -> Option<wit::Post> {
        if !take(&mut self.budget.content) || !self.content_type_granted(&kind) {
            return None;
        }
        let post = self.services.get_post_by_slug(&kind, &slug).await?;
        // Re-check: a services implementation must not widen the requested type.
        self.content_type_granted(&post.kind).then(|| post.into())
    }

    async fn list_posts(&mut self, kind: String, limit: u32, offset: u32) -> Vec<wit::Post> {
        if !take(&mut self.budget.content) || !self.content_type_granted(&kind) {
            return Vec::new();
        }
        let posts = self.services.list_posts(&kind, limit, offset).await;
        posts
            .into_iter()
            .filter(|p| self.content_type_granted(&p.kind))
            .map(Into::into)
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
        self.services.meta_get(&post, &key).await
    }

    async fn set(&mut self, post: String, key: String, value: String) -> Result<(), String> {
        if !take(&mut self.budget.content) {
            return Err(BUDGET_EXCEEDED.into());
        }
        if !(matches_any(&self.grants.content_meta_write, &key) || self.own_meta_key(&key)) {
            return Err(format!("meta key `{key}` is not writable by this plugin"));
        }
        self.services.meta_set(&post, &key, &value).await
    }
}

impl options::Host for HostState {
    async fn get(&mut self, key: String) -> Option<String> {
        if !take(&mut self.budget.content) || !matches_any(&self.grants.options_read, &key) {
            return None;
        }
        self.services.option_get(&key).await
    }
}

impl kv::Host for HostState {
    async fn get(&mut self, key: String) -> Option<Vec<u8>> {
        if !take(&mut self.budget.kv_ops) {
            return None;
        }
        self.services.kv_get(&self.plugin, &key).await
    }

    async fn set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
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
        if take(&mut self.budget.kv_ops) {
            self.services.kv_delete(&self.plugin, &key).await;
        }
    }

    async fn list_keys(&mut self, prefix: String, limit: u32) -> Vec<String> {
        if !take(&mut self.budget.kv_ops) {
            return Vec::new();
        }
        self.services.kv_list(&self.plugin, &prefix, limit).await
    }
}

impl settings::Host for HostState {
    async fn get(&mut self, key: String) -> Option<String> {
        if !take(&mut self.budget.content) {
            return None;
        }
        self.services.setting_get(&self.plugin, &key).await
    }
}

impl http::Host for HostState {
    async fn fetch(&mut self, _req: http::Request) -> Result<http::Response, String> {
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
        HostState::new("tester".into(), Arc::new(grants), services, 1024 * 1024)
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

    #[test]
    fn key_patterns() {
        let pats = vec!["seo.*".to_string(), "site.title".to_string()];
        assert!(matches_any(&pats, "seo.title"));
        assert!(matches_any(&pats, "site.title"));
        assert!(!matches_any(&pats, "site.titles"));
        assert!(!matches_any(&pats, "seox"));
        assert!(matches_any(&["*".to_string()], "anything"));
    }

    #[test]
    fn own_meta_namespace() {
        let s = state(Capabilities::default(), Arc::new(InMemoryServices::new()));
        assert!(s.own_meta_key("plugin.tester.x"));
        assert!(!s.own_meta_key("plugin.testerx.x"));
        assert!(!s.own_meta_key("plugin.other.x"));
    }
}
