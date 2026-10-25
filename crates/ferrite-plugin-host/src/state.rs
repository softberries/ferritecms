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
use crate::services::{HostServices, HttpRequest};
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
const RENDER_WRITE: &str = "meta.set is not allowed during render filters";
const RENDER_MAIL: &str = "mail is not allowed during render filters";
/// `meta.set` on an item that is missing, hidden from the plugin or of a type its
/// `content.read` does not grant (P23): one answer for all three.
const NO_SUCH_POST: &str = "no such post";
/// Longest `mail.notify-admins` subject, in characters (P18).
pub const MAX_MAIL_SUBJECT_CHARS: usize = 200;
/// Largest `mail.notify-admins` body, in bytes (P18).
pub const MAX_MAIL_BODY_BYTES: usize = 64 * 1024;

/// Host-import budget for one call (design 05 §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Budget {
    /// `content`, `meta`, `options` and `settings` calls (all storage reads/writes): one
    /// unit each, two for `meta.get` ([`META_GET_COST`]).
    content: u32,
    kv_ops: u32,
    /// Bytes (key + value) `kv.set` may still write.
    kv_bytes: usize,
    /// Bytes (post id + key + value) `meta.set` may still write.
    meta_bytes: usize,
    http: u32,
    /// `mail.notify-admins` calls (P18: one per call).
    mail: u32,
}

impl Budget {
    fn per_call() -> Self {
        Self {
            content: 100,
            kv_ops: 100,
            kv_bytes: 1024 * 1024,
            meta_bytes: 1024 * 1024,
            http: 3,
            mail: 1,
        }
    }
}

/// Take one unit from `counter`; `false` when it is already exhausted.
fn take(counter: &mut u32) -> bool {
    take_n(counter, 1)
}

/// `content` units one `meta.get` costs (M2a-2a R22 M6): up to three store reads
/// (`post_kind`, the visibility check, the meta read), so twice a plain read.
const META_GET_COST: u32 = 2;

/// Take `n` units from `counter`, all or none; `false` when fewer than `n` are left.
fn take_n(counter: &mut u32, n: u32) -> bool {
    match counter.checked_sub(n) {
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

/// `plugin.<name>.` when `key` lies in the namespace of a plugin other than `own`.
fn foreign_namespace<'k>(key: &'k str, own: &str) -> Option<&'k str> {
    let rest = key.strip_prefix("plugin.")?;
    let (name, _) = rest.split_once('.')?;
    (name != own).then(|| &key[.."plugin.".len() + name.len() + 1])
}

/// [`matches_any`] for meta keys (P23): a key in another plugin's `plugin.<other>.`
/// namespace matches only patterns naming that namespace, so `"*"` (or `plugin.*`)
/// never reaches it.
fn meta_key_matches(patterns: &[String], key: &str, own: &str) -> bool {
    match foreign_namespace(key, own) {
        None => matches_any(patterns, key),
        Some(ns) => patterns
            .iter()
            .filter(|p| p.starts_with(ns))
            .any(|p| matches_any(std::slice::from_ref(p), key)),
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

    /// Whether item `post` is of a type `content.read` grants (P23), looked up through
    /// the services whatever its status; `false` when it does not exist. (Owned handles:
    /// the future must not borrow `self`, which is not `Sync`.)
    fn meta_item_granted(&self, post: &str) -> impl Future<Output = bool> + Send + use<> {
        let services = Arc::clone(&self.services);
        let grants = Arc::clone(&self.grants);
        let post = post.to_owned();
        async move {
            services
                .post_kind(&post)
                .await
                .is_some_and(|kind| matches_any(&grants.content_read, &kind))
        }
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
    /// Keys the read or write patterns grant, or the plugin's own `plugin.<name>.*`, on
    /// items of the `content.read` types (P23); the services add visibility.
    async fn get(&mut self, post: String, key: String) -> Option<String> {
        let readable = meta_key_matches(&self.grants.content_meta_read, &key, &self.plugin)
            || meta_key_matches(&self.grants.content_meta_write, &key, &self.plugin)
            || self.own_meta_key(&key);
        if !take_n(&mut self.budget.content, META_GET_COST) || !readable {
            return None;
        }
        self.trace.post(&post);
        if !self.meta_item_granted(&post).await {
            return None;
        }
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
        if !(meta_key_matches(&self.grants.content_meta_write, &key, &self.plugin)
            || self.own_meta_key(&key))
        {
            return Err(format!("meta key `{key}` is not writable by this plugin"));
        }
        if value.len() > MAX_META_VALUE {
            return Err(format!(
                "meta value is {} bytes, limit {MAX_META_VALUE}",
                value.len()
            ));
        }
        let size = post.len() + key.len() + value.len();
        let Some(left) = self.budget.meta_bytes.checked_sub(size) else {
            return Err(BUDGET_EXCEEDED.into());
        };
        if !self.meta_item_granted(&post).await {
            return Err(NO_SUCH_POST.into());
        }
        self.budget.meta_bytes = left;
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
/// (the services key the read by plugin name) and resolved against the loaded
/// manifest's definition; with nothing usable stored, the declared default. An
/// undeclared key costs no budget.
impl settings::Host for HostState {
    async fn get(&mut self, key: String) -> Option<String> {
        let declared = Arc::clone(&self.settings);
        let declared = declared.get(&key)?;
        if !take(&mut self.budget.content) {
            return None;
        }
        match self
            .services
            .setting_get(&self.plugin, &key, &declared.spec)
            .await
        {
            Some(value) => Some(value),
            None => declared.default.clone(),
        }
    }
}

/// Methods a plugin may use for `http.fetch` (exact, ASCII uppercase).
pub const HTTP_METHODS: [&str; 6] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];
/// Most request headers one `http.fetch` may send.
const MAX_HTTP_REQUEST_HEADERS: usize = 32;
/// Largest request header (name + value bytes).
const MAX_HTTP_HEADER_BYTES: usize = 4 * 1024;
/// Largest request body.
const MAX_HTTP_REQUEST_BODY: usize = 1024 * 1024;
/// Request headers a plugin may not set (compared ignoring case): the host and the
/// connection are the egress's, cookies are not sent. Any `proxy-*` name is refused
/// too.
const REFUSED_HEADERS: [&str; 12] = [
    "host",
    "cookie",
    "connection",
    "proxy-authorization",
    "proxy-connection",
    "transfer-encoding",
    "content-length",
    "te",
    "trailer",
    "upgrade",
    "keep-alive",
    "expect",
];

/// Whether a plugin may not send request header `name` (ignoring case): the
/// connection-level and host headers, cookies and any `proxy-*` header.
pub fn http_header_refused(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    REFUSED_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-")
}

/// Lowercase with one trailing dot dropped: the manifest's canonical host form.
fn canonical_host(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    match host.strip_suffix('.') {
        Some(h) => h.to_owned(),
        None => host,
    }
}

/// M2a-2b Task 11 (P16, 05 §6 "HTTP capability details"), in this order: the per-call
/// budget, the method, the URL (parses; `https`; no userinfo; port absent or 443; its
/// host, made canonical, exactly in the manifest's `http` list), the headers and the
/// body. A refusal is an error to the guest, never a plugin failure. Then the services
/// fetch with the canonical URL (resolved-address checks and redirects are theirs).
/// Every call is volatile (05 §7a): the answer can change at any time.
impl http::Host for HostState {
    async fn fetch(&mut self, req: http::Request) -> Result<http::Response, String> {
        self.trace.volatile();
        if !take(&mut self.budget.http) {
            return Err(BUDGET_EXCEEDED.into());
        }
        let req = check_http_request(req, &self.grants.http)?;
        let res = self
            .services
            .http_fetch(&self.plugin, &self.grants.http, req)
            .await?;
        Ok(http::Response {
            status: res.status,
            headers: res.headers,
            body: res.body,
        })
    }
}

fn check_http_request(req: http::Request, hosts: &[String]) -> Result<HttpRequest, String> {
    if !HTTP_METHODS.contains(&req.method.as_str()) {
        return Err("method not allowed".into());
    }
    let mut url = url::Url::parse(&req.url).map_err(|_| "invalid URL".to_owned())?;
    if url.scheme() != "https" {
        return Err("only https URLs are allowed".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with credentials are not allowed".into());
    }
    // `url` drops the default port, so any port left is not 443.
    if url.port().is_some() {
        return Err("only port 443 is allowed".into());
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) => canonical_host(d),
        _ => return Err("host not allowed".into()),
    };
    if !hosts.contains(&host) {
        return Err("host not allowed".into());
    }
    url.set_host(Some(&host))
        .map_err(|_| "invalid URL".to_owned())?;
    if req.headers.len() > MAX_HTTP_REQUEST_HEADERS {
        return Err("too many request headers".into());
    }
    for (name, value) in &req.headers {
        if name.len() + value.len() > MAX_HTTP_HEADER_BYTES {
            return Err("request header over 4 KiB".into());
        }
        if http_header_refused(name) {
            return Err("request header not allowed".into());
        }
    }
    if req
        .body
        .as_ref()
        .is_some_and(|b| b.len() > MAX_HTTP_REQUEST_BODY)
    {
        return Err("request body over 1 MiB".into());
    }
    Ok(HttpRequest {
        method: req.method,
        url,
        headers: req.headers,
        body: req.body,
    })
}

/// P18 / F29, in this order: refused during render filters (R20); one mail per call
/// (a refused second attempt reaches no service, so it uses no hourly slot); a subject
/// of 1..=[`MAX_MAIL_SUBJECT_CHARS`] characters without control characters (refused,
/// never folded: no header injection); a body of at most [`MAX_MAIL_BODY_BYTES`]. The
/// services add the hourly limit and the recipients.
impl mail::Host for HostState {
    async fn notify_admins(&mut self, subject: String, text_body: String) -> Result<(), String> {
        if self.render_path {
            if !self.refusal_logged {
                self.refusal_logged = true;
                tracing::warn!(plugin = %self.plugin, "mail refused during a render filter");
            }
            return Err(RENDER_MAIL.into());
        }
        if !take(&mut self.budget.mail) {
            return Err("only one mail per call".into());
        }
        let chars = subject.chars().count();
        if subject.trim().is_empty() || chars > MAX_MAIL_SUBJECT_CHARS {
            return Err(format!(
                "the subject must be 1 to {MAX_MAIL_SUBJECT_CHARS} characters"
            ));
        }
        // Header-safety copy 2 of 3 (keep in step): `ferrite_core::mail::header_safe`
        // (1, applied again by every mailer) and `ferrite-config`'s `valid_mailbox` (3);
        // this crate does not depend on core.
        if subject
            .chars()
            .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
        {
            return Err("the subject must not contain control characters".into());
        }
        if text_body.len() > MAX_MAIL_BODY_BYTES {
            return Err(format!(
                "the body is {} bytes, limit {MAX_MAIL_BODY_BYTES}",
                text_body.len()
            ));
        }
        self.services
            .notify_admins(&self.plugin, &subject, &text_body)
            .await
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

    /// Grants reading `post` items, with item `p` (a post) in the services.
    fn post_reader() -> (Capabilities, Arc<InMemoryServices>) {
        let services = Arc::new(InMemoryServices::new());
        services.insert_post(post("p", "post"));
        let caps = Capabilities {
            content_read: vec!["post".into()],
            ..Capabilities::default()
        };
        (caps, services)
    }

    #[tokio::test]
    async fn meta_value_cap() {
        let (caps, services) = post_reader();
        let mut s = state(caps, services);
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

    /// R22 M6: `meta.get` takes 2 of the 100 `content` units, so the 51st in one call
    /// is refused.
    #[tokio::test]
    async fn meta_get_costs_two_units() {
        let (mut caps, services) = post_reader();
        caps.content_meta_read = vec!["seo.*".into()];
        services
            .meta_set("p", "seo.k", "\"v\"")
            .await
            .expect("seed");
        let mut s = state(caps, services);
        for i in 0..50 {
            assert_eq!(
                meta::Host::get(&mut s, "p".into(), "seo.k".into())
                    .await
                    .as_deref(),
                Some("\"v\""),
                "get {i}"
            );
        }
        assert_eq!(s.budget.content, 0);
        assert_eq!(
            meta::Host::get(&mut s, "p".into(), "seo.k".into()).await,
            None
        );
        // With one unit left, a `meta.get` is refused and does not take it.
        s.budget.content = 1;
        assert_eq!(
            meta::Host::get(&mut s, "p".into(), "seo.k".into()).await,
            None
        );
        assert_eq!(s.budget.content, 1);
    }

    #[tokio::test]
    async fn meta_bytes_budget() {
        let (caps, services) = post_reader();
        let mut s = state(caps, services.clone());
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

    fn render_state(services: Arc<dyn HostServices>, render_path: bool) -> HostState {
        HostState::new(
            "tester".into(),
            Arc::new(Capabilities::default()),
            Arc::default(),
            services,
            1024 * 1024,
            CallTrace::default(),
            render_path,
        )
    }

    #[tokio::test]
    async fn mail_rules() {
        use mail::Host as _;
        let services = Arc::new(InMemoryServices::new());
        // Render path: refused, nothing reaches the services.
        let mut s = render_state(services.clone(), true);
        assert_eq!(
            s.notify_admins("Hi".into(), "b".into()).await,
            Err(RENDER_MAIL.into())
        );
        assert!(services.mails().is_empty());

        // One per call; the second attempt never reaches the services.
        let mut s = render_state(services.clone(), false);
        assert_eq!(s.notify_admins("Hi".into(), "b".into()).await, Ok(()));
        assert!(s.notify_admins("Again".into(), "b".into()).await.is_err());
        assert_eq!(services.mails().len(), 1);
        assert_eq!(services.mails()[0].subject, "Hi");

        // Subject and body limits, checked after the per-call slot is taken.
        let refused = [
            (String::new(), String::new()),
            ("  ".into(), String::new()),
            ("é".repeat(MAX_MAIL_SUBJECT_CHARS + 1), String::new()),
            ("Hi\r\nBcc: victim@evil.test".into(), String::new()),
            ("Hi\nX".into(), String::new()),
            ("nul\0".into(), String::new()),
            ("c1\u{85}".into(), String::new()),
            ("ls\u{2028}".into(), String::new()),
            ("Hi".into(), "x".repeat(MAX_MAIL_BODY_BYTES + 1)),
        ];
        for (subject, body) in refused {
            let mut s = render_state(services.clone(), false);
            assert!(
                s.notify_admins(subject.clone(), body).await.is_err(),
                "{subject:?}"
            );
        }
        assert_eq!(services.mails().len(), 1, "no refused mail was sent");
        let mut s = render_state(services.clone(), false);
        assert_eq!(
            s.notify_admins(
                "é".repeat(MAX_MAIL_SUBJECT_CHARS),
                "x\n".repeat(MAX_MAIL_BODY_BYTES / 2)
            )
            .await,
            Ok(())
        );
        // Without a mail service (the trait default).
        struct NoMail;
        #[async_trait::async_trait]
        impl HostServices for NoMail {
            async fn get_post(&self, _: &str, _: &crate::services::KindOk) -> Option<Post> {
                None
            }
            async fn post_kind(&self, _: &str) -> Option<String> {
                None
            }
            async fn get_post_by_slug(&self, _: &str, _: &str) -> Option<Post> {
                None
            }
            async fn list_posts(&self, _: &str, _: u32, _: u32) -> Vec<Post> {
                Vec::new()
            }
            async fn meta_get(&self, _: &str, _: &str) -> Option<String> {
                None
            }
            async fn meta_set(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
            async fn option_get(&self, _: &str) -> Option<String> {
                None
            }
            async fn kv_get(&self, _: &str, _: &str) -> Option<Vec<u8>> {
                None
            }
            async fn kv_set(&self, _: &str, _: &str, _: Vec<u8>) -> Result<(), String> {
                Ok(())
            }
            async fn kv_delete(&self, _: &str, _: &str) {}
            async fn kv_list(&self, _: &str, _: &str, _: u32) -> Vec<String> {
                Vec::new()
            }
            async fn setting_get(
                &self,
                _: &str,
                _: &str,
                _: &crate::manifest::SettingSpec,
            ) -> Option<String> {
                None
            }
        }
        let mut s = render_state(Arc::new(NoMail), false);
        assert_eq!(
            s.notify_admins("Hi".into(), "b".into()).await,
            Err(crate::services::MAIL_NOT_CONFIGURED.into())
        );
    }

    /// P23: `meta.get` and `meta.set` act only on items of the `content.read` types
    /// (looked up whatever their status); a missing item is refused like a hidden one.
    #[tokio::test]
    async fn meta_is_scoped_to_read_types() {
        let services = Arc::new(InMemoryServices::new());
        services.insert_post(post("p1", "post"));
        let mut draft = post("d1", "post");
        draft.status = "draft".into();
        services.insert_post(draft);
        services.insert_post(post("g1", "page"));
        for id in ["p1", "d1", "g1"] {
            services.meta_set(id, "seo.k", "\"v\"").await.expect("seed");
        }
        let caps = Capabilities {
            content_read: vec!["post".into()],
            content_meta_write: vec!["seo.*".into()],
            ..Capabilities::default()
        };
        let mut s = state(caps, services.clone());
        let set = |id: &str| (id.to_string(), "seo.k".to_string(), "\"w\"".to_string());
        for id in ["p1", "d1"] {
            let (p, k, v) = set(id);
            assert_eq!(meta::Host::set(&mut s, p, k, v).await, Ok(()), "{id}");
            assert_eq!(
                meta::Host::get(&mut s, id.into(), "seo.k".into())
                    .await
                    .as_deref(),
                Some("\"w\""),
                "{id}"
            );
        }
        for id in ["g1", "missing"] {
            let (p, k, v) = set(id);
            assert_eq!(
                meta::Host::set(&mut s, p, k, v).await,
                Err(NO_SUCH_POST.into()),
                "{id}"
            );
            assert_eq!(
                meta::Host::get(&mut s, id.into(), "seo.k".into()).await,
                None
            );
        }
        assert_eq!(
            services.meta_get("g1", "seo.k").await.as_deref(),
            Some("\"v\"")
        );
    }

    /// P23: a `"*"` (or `plugin.*`) meta grant never reaches another plugin's
    /// `plugin.<other>.*` keys; the plugin's own namespace and an explicit grant of the
    /// other namespace do.
    #[tokio::test]
    async fn star_meta_grant_excludes_other_plugins_namespace() {
        let (mut caps, services) = post_reader();
        for key in ["plugin.other.secret", "plugin.tester.mine", "seo.title"] {
            services.meta_set("p", key, "1").await.expect("seed");
        }
        caps.content_meta_read = vec!["*".into()];
        caps.content_meta_write = vec!["*".into(), "plugin.*".into()];
        let mut s = state(caps.clone(), services.clone());
        let get =
            async |s: &mut HostState, key: &str| meta::Host::get(s, "p".into(), key.into()).await;
        assert_eq!(get(&mut s, "plugin.other.secret").await, None);
        assert_eq!(
            get(&mut s, "plugin.tester.mine").await.as_deref(),
            Some("1")
        );
        assert_eq!(get(&mut s, "seo.title").await.as_deref(), Some("1"));
        assert!(
            meta::Host::set(&mut s, "p".into(), "plugin.other.secret".into(), "2".into())
                .await
                .is_err()
        );
        assert_eq!(
            services
                .meta_get("p", "plugin.other.secret")
                .await
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            meta::Host::set(&mut s, "p".into(), "plugin.tester.mine".into(), "2".into()).await,
            Ok(())
        );
        // An explicit grant of the other plugin's namespace is honoured.
        caps.content_meta_read = vec!["plugin.other.*".into()];
        let mut s = state(caps, services);
        assert_eq!(
            get(&mut s, "plugin.other.secret").await.as_deref(),
            Some("1")
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
