# 05 — Plugin Host

The plugin host is the project's core differentiator and its riskiest part. M0 exists to
prove this document's assumptions against the pinned wasmtime version (50.x at time of
writing).

## 1. Requirements

| # | Requirement |
|---|---|
| P1 | A plugin is a WebAssembly **component** + `ferrite-plugin.toml` manifest |
| P2 | The host links an interface only if the manifest declares the capability; a component importing an undeclared interface is **rejected at install** |
| P3 | Every call is bounded: memory, fuel, wall-clock deadline |
| P4 | No shared memory, no state across calls except via the KV interface |
| P5 | A failing plugin (trap, timeout, bad output) never breaks a page; the hook is skipped |
| P6 | Repeated failures auto-disable the plugin cluster-wide |
| P7 | Fine-grained scopes inside an interface (e.g. `content.read = ["post"]`) are enforced by the host at call time |
| P8 | Filter warm call overhead < 1 ms (M0 exit criterion) |
| P9 | WIT in `wit/` is versioned; the host can serve more than one world version |

## 2. Lifecycle

```
upload .wasm + manifest ──► validate manifest ──► inspect component imports/exports
     ──► compare against declared capabilities ──► show permissions to admin
     ──► admin approves ──► store artifact + plugins row (enabled) + event
     ──► every node: fetch artifact → compile (cached) → InstancePre per world → registry swap
```

- **Install-time check (P2)**: walk the component type's imports. Allowed:
  - `ferrite:plugin/*` interfaces that the manifest's capabilities map to (§4),
  - the WASI subset in §5.
  Anything else → reject with the exact import name. Exports must include the
  `ferrite:plugin/guest` interface of a supported world version.
- **Update**: if the new version declares capabilities beyond what was granted, it is
  stored but **not activated** until an admin approves the diff. The old version keeps
  running.
- **Disable/enable/uninstall** via admin/CLI; uninstall keeps KV data for 30 days unless
  `--purge`.

## 3. Manifest (`ferrite-plugin.toml`)

```toml
[plugin]
name        = "seo-pro"              # [a-z0-9-]{2,48}, unique per site
version     = "1.4.0"                # semver
publisher   = "example.dev"
description = "Meta tags, Open Graph, sitemap pings"
world       = "ferrite:plugin/plugin@0.1.0"
license     = "MIT"
homepage    = "https://example.dev/seo-pro"

[capabilities]
content.read       = ["post", "page"]       # content types readable; ["*"] = all public types
content.meta.read  = ["seo.*"]
content.meta.write = ["seo.*"]              # meta key globs; "plugin.<name>.*" always allowed
options.read       = ["site.title", "site.url"]
kv                 = true
http               = ["api.indexnow.org"]   # hostnames, HTTPS only; no wildcards in v1
render.head        = true                   # may contribute structured head tags
render.text        = true                   # may rewrite plain-text values (excerpt)
render.html        = "off"                  # "off" | "sanitized" | "raw" — rewrite body/footer HTML (§7)

routes             = true                   # may serve endpoints under /_p/<name>/ (§4b)
mail               = "admins"               # may e-mail site admins only (no arbitrary recipients)

[hooks]
"render.head" = 10                          # filter, priority (lower runs first)
"post.published" = 50                       # action
"request.served" = 100                      # action, batched (§4a)

[routes]
"/submit" = { methods = ["POST"], rate_limit = "10/min" }   # → /_p/seo-pro/submit

[settings]                                  # declarative; admin renders the form (§4c)
indexnow_key = { type = "secret", label = "IndexNow API key" }
default_og_image = { type = "media", label = "Default OG image" }
add_json_ld = { type = "bool", default = true, label = "Emit JSON-LD" }

[limits]                                    # may only lower the site defaults, or request
memory = "16MB"                             # higher values that the admin must approve
fuel   = 20_000_000
```

Validation: unknown keys are errors; hooks must exist in the declared world version and
require their matching capability (e.g. `render.head` hook needs `render.head = true`).

## 4. WIT world (draft for `ferrite:plugin@0.1.0`)

Final text lives in `wit/ferrite-plugin@0.1.0/` and is written in M0. Shape:

```wit
package ferrite:plugin@0.1.0;

interface types {
  type post-id = string;                       // uuid text

  record post {
    id: post-id, kind: string, slug: string, title: string, excerpt: string,
    status: string, published: option<string>, // RFC 3339
    url: string, author: string, terms: list<term-ref>,
  }
  record term-ref { taxonomy: string, slug: string, name: string }

  record meta-tag { name: option<string>, property: option<string>, content: string }
  record link-tag { rel: string, href: string, hreflang: option<string>, kind: option<string> }
  record script-tag { src: option<string>, json-ld: option<string> } // inline JS not allowed
  record head { title: option<string>, metas: list<meta-tag>, links: list<link-tag>, scripts: list<script-tag> }

  record request-info { path: string, locale: string, is-logged-in: bool, post: option<post-id> }

  variant filter-value { head(head), html(string), text(string) }
  /// Action payloads are JSON documented per hook, so new action hooks and payload
  /// fields are additive (minor version) instead of breaking the world.
  record action-event { subject: option<string>, payload: string }

  record route-request {
    method: string, path: string,                       // path relative to /_p/<name>
    query: list<tuple<string,string>>,
    headers: list<tuple<string,string>>,                // allowlisted: content-type, accept, accept-language, user-agent, referer
    body: list<u8>,                                     // ≤ 1 MB
    client-ip-hash: string,                             // salted hash, never the raw IP
    user: option<string>,                               // user id if logged in (no cookies or tokens passed)
  }
  record route-response {
    status: u16, headers: list<tuple<string,string>>,   // Set-Cookie, CSP and auth headers stripped/overridden by host
    body: list<u8>,
  }

  variant plugin-error { invalid-input(string), unavailable(string), other(string) }
}

/// Exported by every plugin.
interface guest {
  use types.{filter-value, action-event, request-info, plugin-error};
  apply-filter: func(hook: string, value: filter-value, req: request-info) -> result<filter-value, plugin-error>;
  on-action: func(hook: string, event: action-event) -> result<_, plugin-error>;
  /// Only called if the manifest declares `routes = true` and a matching [routes] entry.
  handle-request: func(req: route-request) -> result<route-response, plugin-error>;
}

/// Always linked.
interface log {
  enum level { trace, debug, info, warn, error }
  log: func(level: level, message: string);
}

/// capability: content.read
interface content {
  use types.{post, post-id};
  get-post: func(id: post-id) -> option<post>;
  get-post-by-slug: func(kind: string, slug: string) -> option<post>;
  list-posts: func(kind: string, limit: u32, offset: u32) -> list<post>;
}

/// capability: content.meta.read / content.meta.write
interface meta {
  use types.{post-id};
  get: func(post: post-id, key: string) -> option<string>;     // JSON text
  set: func(post: post-id, key: string, value: string) -> result<_, string>;
}

/// capability: options.read
interface options { get: func(key: string) -> option<string>; }

/// capability: kv
interface kv {
  get: func(key: string) -> option<list<u8>>;
  set: func(key: string, value: list<u8>) -> result<_, string>;
  delete: func(key: string);
  list-keys: func(prefix: string, limit: u32) -> list<string>;
}

/// capability: http
interface http {
  record request { method: string, url: string, headers: list<tuple<string,string>>, body: option<list<u8>> }
  record response { status: u16, headers: list<tuple<string,string>>, body: list<u8> }
  fetch: func(req: request) -> result<response, string>;
}

/// Always linked: the plugin's own settings only (from its [settings] schema).
interface settings { get: func(key: string) -> option<string>; }   // JSON text

/// capability: mail = "admins"
interface mail { notify-admins: func(subject: string, text-body: string) -> result<_, string>; }

world plugin {
  import log;
  import content;
  import meta;
  import options;
  import kv;
  import http;
  import settings;
  import mail;
  export guest;
}
```

**D13 — generic dispatch.** A world's exports are fixed, so instead of one export per
hook the plugin exports `apply-filter`/`on-action` and the host passes the hook name.
The manifest's `[hooks]` table tells the host which hooks to call; the host checks the
returned `filter-value` variant matches the hook's declared type (e.g. `render.head` →
`head`), otherwise it's a failure.

**Imports are optional at link time.** The world lists every import; a component only
imports what it uses (wit-bindgen/`cargo component` drop unused imports — **verify in
M0**). The host's `Linker` for a plugin is built with only the granted interfaces, so an
undeclared import fails instantiation even if the install-time check were bypassed
(defence in depth).

### Hook catalogue (v0.1)

| Hook | Type | Value | Capability | Default deadline |
|---|---|---|---|---|
| `render.head` | filter | `head` | `render.head` | render (50 ms) |
| `content.body` | filter | `html` | `render.html` ≠ off | render |
| `render.footer` | filter | `html` | `render.html` ≠ off | render |
| `content.excerpt` | filter | `text` | `render.text` | render |
| `post.saved` | action | `{"post": id}` | — | action (5 s) |
| `post.published` | action | `{"post": id}` | — | action |
| `user.created` | action | `{"user": id}` | — | action |
| `request.served` | action (batched) | `{"events":[{path, status, ts, referrer_host, ua_class, cache}]}` | — | action |

### 4a. `request.served` (analytics)

Fired for every public page response **including cache hits**. Events are buffered in
memory per node and flushed every 10 s (or 1 000 events) as **one** `plugin.action` job
per subscribed plugin — never one job per request. Payload contains no IP, cookie or
full user agent: only path, status, timestamp, referrer host, a coarse UA class
(`browser`/`bot`/`other`) and cache hit/miss. Lost on node crash (acceptable for
analytics; documented).

### 4b. Plugin routes

- Mounted at `/_p/<plugin>/<path>`; only paths and methods listed in `[routes]`.
- Limits: route deadline 2 s, body ≤ 1 MB, per-route rate limit (DB-backed, so it holds
  across nodes), default `10/min` per client-IP hash.
- **CSRF / origin**: non-GET requests require `Origin`/`Referer` matching `site.url`
  unless the route declares `cross_origin = true`.
- **XSS containment**: responses are served with
  `Content-Security-Policy: sandbox allow-forms` (opaque origin, so scripts — if any —
  can't read site cookies or call the API as the user), `X-Content-Type-Options: nosniff`.
  Plugins can't set cookies. JSON and redirects (`303` to same-origin paths) are the
  intended response types; HTML is allowed but sandboxed.
- The session cookie and `Authorization` header are never passed to the plugin; it
  only gets `user` (id) if the visitor is logged in.
- Themes post forms to routes with a helper: `{{ plugin_route("contact-form", "/submit") }}`.

### 4c. Plugin settings

- `[settings]` uses the same field schema as theme settings (`text`, `textarea`, `bool`,
  `number`, `select`, `color`, `media`, `secret`) — one renderer in the admin.
- Stored in `options` under `plugin.<name>.settings.<key>`; `secret` values are
  encrypted at rest with the site key and never returned by the API after being set.
- Changing settings emits `options.changed` + flushes pages that rendered that plugin's
  filters (dependency key `plugin:<name>`, recorded whenever a plugin filter runs).

Filters run in priority order (ties: plugin name), each receiving the previous output.
Actions run as `plugin.action` jobs ([04](04-cluster.md#job-queue)), never on the request
path.

## 5. WASI policy

Rust's `wasm32-wasip2` std imports some WASI interfaces even for simple code. Policy:

| WASI interface | Policy |
|---|---|
| `wasi:clocks/*` | allowed (wall clock, monotonic) |
| `wasi:random/*` | allowed |
| `wasi:cli/environment` | allowed, **empty** env and args |
| `wasi:cli/stdout`, `stderr` | allowed, captured → `tracing` with plugin name, rate/size limited |
| `wasi:cli/stdin` | allowed, empty |
| `wasi:cli/exit` | allowed (exit = trap for our purposes) |
| `wasi:io/*` | allowed (needed by the above) |
| `wasi:filesystem/*` | linked with **no preopens** (no accessible files) |
| `wasi:sockets/*` | **not linked**; component importing it is rejected |
| `wasi:http/*` | **not linked**; outbound HTTP only via `ferrite:plugin/http` |

Exact interface list and whether wasmtime-wasi lets us link the subset cleanly (vs.
linking all with a deny-all `WasiCtx`) is an M0 task.

## 6. Execution model & limits

### Engine (one per process)

- `Config`: component model on, `async_support(true)`, `consume_fuel(true)`,
  `epoch_interruption(true)`, pooling instance allocator (sized from config: max
  concurrent instances, memory per instance), Cranelift `opt_level = speed`.
- An epoch ticker thread calls `engine.increment_epoch()` every **5 ms**.
- Compiled components cached on disk keyed by
  `(artifact hash, wasmtime version, engine config hash)` via
  `Component::serialize` / `deserialize` (deserialize is `unsafe` — only for files we
  wrote; cache dir is ours).

### Per call (P3, P4)

```
store = Store::new(&engine, HostState { plugin, grants, limits, budget, req })
store.limiter(|s| &mut s.limits)                 // StoreLimits: memory_size, instances=1, tables
store.set_fuel(fuel_per_call)                    // instruction budget
store.fuel_async_yield_interval(Some(10_000))    // cooperative yield to tokio
store.epoch_deadline_trap(); store.set_epoch_deadline(deadline_ticks)
instance = instance_pre.instantiate_async(&mut store)      // pooling allocator → fast
result = tokio::time::timeout(hard_deadline, guest.call_apply_filter(&mut store, …))
```

- **Fresh instance per call** (default). Guarantees no state leaks between requests and
  matches P4. M0 benchmarks this against reusing an instance per (plugin, worker); if
  fresh instantiation misses P8, reuse with a reset is the fallback — documented as a
  weaker isolation mode.
- `hard_deadline` (tokio timeout, = deadline + 50%) is the backstop for time spent in
  host imports (DB, HTTP), which epochs don't interrupt.
- Host-import budgets per call: `content` ≤ 100 calls, `kv` ≤ 100 ops / 1 MB, `http` ≤ 3
  requests, each ≤ 5 s / 2 MB response; exceeding → the import returns an error, the
  guest decides.
- Data crossing the boundary is copied (Component Model canonical ABI). Strings/lists
  from the guest have size caps (e.g. `head` ≤ 64 KB total, `html` ≤ 4× input or 2 MB).

### HTTP capability details

- HTTPS only; host must exactly match the manifest list; redirects followed only to
  allow-listed hosts (max 3).
- Resolved IPs in private/loopback/link-local/metadata ranges are refused (SSRF / DNS
  rebinding protection) — check after resolution, connect to the checked IP.
- `User-Agent: ferrite/<ver> plugin/<name>`; no cookies, no ambient credentials.

## 7. Output validation

- `head` is structured: the host renders tags itself with proper escaping. Plugins can't
  inject arbitrary HTML into `<head>`. `script-tag.src` must be https or same-origin;
  `json-ld` is checked to be valid JSON and embedded with `</` escaped.
- `html` results (`content.body`, `render.footer`) are sanitised with `ammonia` using a
  policy that allows typical content markup but **no `<script>`, inline handlers or
  `javascript:` URLs** by default. Raw HTML requires `render.html = "raw"` (shown as a
  high-risk permission at install).

## 7a. Data visibility and caching

- **What `content.read` sees**: published, publicly visible items of the granted types —
  never drafts, pending, private, scheduled or trashed items, and never user e-mails or
  auth data. Exception: during an authenticated **preview** of a draft, `request-info.post`
  and `get-post` on *that one item* return it, so plugins render previews faithfully.
  Preview responses are never cached.
- **Dependency recording**: every `content`, `meta` and `options` import call made during
  a render filter records the matching dependency key (`post:<id>`, `option:<key>`,
  `archive:<type>` for listings) into the request's `DependencySet`, so cached pages that
  used plugin-read data are invalidated correctly.
- **Volatile output**: if a render-filter call uses `kv`, `http`, or wall-clock time, the
  host marks the page *volatile* and caps its cache TTL at `plugins.volatile_ttl`
  (default 60 s). No WIT change needed; it's inferred from host-import usage.

## 8. Failure isolation (P5, P6)

| Failure | Detection | Effect on request |
|---|---|---|
| Trap (panic, unreachable, OOB) | `Err(Trap)` | hook skipped, previous value used |
| Out of fuel | `Trap::OutOfFuel` | same |
| Epoch deadline / hard timeout | trap / `Elapsed` | same |
| Memory limit (grow fails → guest aborts) | trap | same |
| Wrong variant / oversize output | host validation | same |
| Instantiation failure (missing import) | `Err` at instantiate | plugin marked broken on this node |

Every failure: `tracing::warn!(plugin, hook, kind, duration, fuel_used)`, metric
increment, and an upsert into `plugin_failures` (1-minute buckets; written
asynchronously, batched per node every few seconds so failures don't add DB writes to
the request path). When the sum over `failure_window` ≥ `disable_after_failures`, the
node that crosses the threshold sets `plugins.enabled = false,
disabled_reason = 'auto: …'` and emits `plugin.disabled`. Admin sees the reason and can
re-enable.

## 9. Host-side API (Rust)

```rust
pub struct PluginHost { /* engine, registry: ArcSwap<Registry>, store access, http client */ }

impl PluginHost {
    pub async fn apply_filter(&self, hook: HookName, value: FilterValue, req: &RequestInfo)
        -> FilterValue;                                   // never fails; failures logged
    pub async fn dispatch_action(&self, plugin: &str, hook: HookName, ev: ActionEvent)
        -> Result<(), PluginError>;                       // used by job worker (retry on Err)
    pub fn inspect(&self, wasm: &[u8]) -> Result<ComponentReport, PluginError>; // install-time
    pub async fn reload(&self, plugins: Vec<InstalledPlugin>) -> Result<(), PluginError>;
}
```

## 10. Guest SDK (`ferrite-sdk`)

- Wraps `wit-bindgen` generated bindings; exposes `Head`, `Post`, `Ctx` types and
  macros:

```rust
use ferrite_sdk::{filter, action, Head, Ctx, Result};

#[filter("render.head")]
fn head(mut h: Head, ctx: &Ctx) -> Result<Head> {
    if let Some(post) = ctx.current_post()? {          // requires content.read
        h.meta("description", &post.excerpt);
    }
    Ok(h)
}

#[action("post.published")]
fn ping(post: PostId, ctx: &Ctx) -> Result<()> {
    ctx.http().post("https://api.indexnow.org/…", body)?;   // requires http
    Ok(())
}

ferrite_sdk::export!();   // generates the guest interface impl dispatching by hook name
```

- Priorities live in the manifest, not in code (single source of truth; the brief's
  example with `priority = 10` in the attribute is dropped).
- Build: `cargo build --target wasm32-wasip2 --release` producing a component directly
  (verify tooling in M0; fallback: `cargo component`).
- `ferrite plugin new <name>` scaffolds a plugin crate + manifest;
  `ferrite plugin check <dir>` runs the install-time validation locally.

## 11. Versioning (P9)

- WIT package versions follow semver. Additive changes (new hook names, new optional
  imports) → minor bump (`0.2.0`); since WIT records aren't extensible, adding a field
  to a record is a **breaking** change → new major world.
- The host embeds bindings for every supported world version (`bindgen!` per version
  module) and adapts domain types into each. Support window: current + previous major.

## 12. Hostile test plugins (required by CLAUDE.md)

In `plugins/examples/hostile/`, each with an integration test asserting the page still
renders and the failure is recorded:

| Plugin | Behaviour | Expected |
|---|---|---|
| `loop` | `loop {}` in filter | killed by epoch/fuel within deadline; page renders |
| `oom` | allocates until failure | trap; page renders; host memory stable |
| `panic` | `panic!()` | trap; page renders |
| `undeclared-import` | imports `http` without declaring it | install rejected; forced load fails to instantiate |
| `sockets` | uses `std::net` (imports `wasi:sockets`) | install rejected |
| `big-output` | returns 100 MB string | rejected by size cap |
| `slow-host` | calls `http.fetch` to a slow server | hard timeout; page renders |
| `wrong-variant` | returns `text` for `render.head` | ignored, recorded |
| `fs-escape` | tries to open `/etc/passwd` | no preopens → error inside guest; nothing leaks |
| `flapper` | fails every call | auto-disabled after threshold |
| `route-xss` | route returns HTML with `<script>` reading `document.cookie` | served under sandbox CSP; E2E asserts no cookie access |
| `route-flood` | many POSTs to its route | rate-limited across 2 nodes |
