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

- **Status (M1.5)**: `inspect` + manifest validation exist. The host API is split for hot
  reload: `PluginHost::prepare(manifest, wasm)` inspects, compiles and links (slow; async
  callers run it under `spawn_blocking`) and `PluginHost::commit(prepared, keep)` swaps the
  registry once to exactly the prepared plugins plus the loaded ones named in `keep`;
  `install` is both, keeping everything. Calls in flight keep the registry snapshot (and
  instances) they loaded; later calls see the new one. `PluginHost::disable(name, reason)`
  stops a loaded plugin at once; `loaded()` lists the registry.
  `ferrite_http::PluginReloader::reload` brings a node's host to the `plugins` rows in one
  commit: rows whose artifact, manifest and grant are unchanged (and that the host has
  not disabled) are kept without recompiling (P19: reuse is in memory by artifact; no
  on-disk component cache); new or changed rows, and rows enabled again after a host-side
  disable, are fetched (SHA-256 checked), grant-checked (R21) and prepared; a version that
  fails keeps the previous one running (reported `Failed`); disabled and deleted rows are
  dropped. Startup's `PluginReloader::reload` is the first reload; a row it could not load is retried like a
  failed reload (R17). In `ferrite serve` one
  `PluginReloader` per process runs again on every `plugins.changed` and
  `plugin.disabled` event (the latter also disables the plugin in this host at once) and
  after lost event history, on the node's reload task (`crates/ferrite/src/reload.rs`,
  P18), never on the event loop; the page cache is flushed after each event-driven reload
  (R13), and a failed reload is retried after 5 s, doubling to 5 min, until it succeeds or
  a newer event arrives (R14). Operators install with
  `ferrite::extensions::install_plugin_file` (prepares on a scratch host first, uploads,
  then `ExtensionService` writes artifact row + plugin row + `plugins.changed` in one
  transaction; P21). Admin approval/permission display, the update-needs-approval flow
  and uninstall/KV retention are M2.
- **Install-time check (P2)**: walk the component type's imports. Allowed:
  - `ferrite:plugin/*` interfaces that the manifest's capabilities map to (§4),
  - the WASI subset in §5.
  Anything else → reject with the exact import name. Exports must include the
  `ferrite:plugin/guest` interface of a supported world version.
- **Update** (not in M0): if the new version declares capabilities beyond what was granted, it is
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
  use types.{filter-value, action-event, request-info, route-request, route-response, plugin-error};
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
interface options { get: func(key: string) -> option<string>; }   // JSON text

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
imports what it uses (**verified in M0**: `cargo build --target wasm32-wasip2` with
wit-bindgen drops unused imports; the `hello` plugin imports exactly `log`). The host's `Linker` for a plugin is built with only the granted interfaces, so an
undeclared import fails instantiation even if the install-time check were bypassed
(defence in depth).

### Hook catalogue (v0.1)

| Hook | Type | Value | Capability | Default deadline |
|---|---|---|---|---|
| `render.head` | filter | `head` | `render.head` | render (200 ms, ruling R18) |
| `content.body` | filter | `html` | `render.html` ≠ off | render |
| `render.footer` | filter | `html` | `render.html` ≠ off | render |
| `content.excerpt` | filter | `text` | `render.text` | render |
| `post.saved` | action | `{"post": id}` | — | action (5 s) |
| `post.published` | action | `{"post": id}` | — | action |
| `user.created` | action | `{"user": id}` | — | action |
| `request.served` | action (batched) | `{"events":[{path, status, ts, referrer_host, ua_class, cache}]}` | — | action |

### 4a. `request.served` (analytics)

*M0 status: not implemented (M2). M0 only has the `request.served` entry in the hook catalogue.*

Fired for every public page response **including cache hits**. Events are buffered in
memory per node and flushed every 10 s (or 1 000 events) as **one** `plugin.action` job
per subscribed plugin — never one job per request. Payload contains no IP, cookie or
full user agent: only path, status, timestamp, referrer host, a coarse UA class
(`browser`/`bot`/`other`) and cache hit/miss. Lost on node crash (acceptable for
analytics; documented).

### 4b. Plugin routes

*M0 status: the host can call a plugin's `handle-request` (`PluginHost::handle_route`, 2 s route deadline) but mounting under `/_p/`, rate limits, CSRF/origin checks, CSP and header stripping, and the theme helper are M2; no example plugin exports a route.*

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

*Status (M1c-2): the manifest `[settings]` schema is parsed; the `settings` import reads `plugin.<name>.settings.<key>` from `options` (`StoreHostServices::setting_get`) and records an `option:` dependency key, so a settings change already invalidates through `options.changed`. Settings writes, secret encryption and the admin form are M2.*

- `[settings]` uses the same field schema as theme settings (`text`, `textarea`, `bool`,
  `number`, `select`, `color`, `media`, `secret`) — one renderer in the admin.
- Stored in `options` under `plugin.<name>.settings.<key>`; `secret` values are
  encrypted at rest with the site key and never returned by the API after being set.
- Changing settings emits `options.changed` + flushes pages that rendered that plugin's
  filters (as built: through the `option:plugin.<name>.settings.<key>` keys recorded on read; the finer `plugin:<name>` key is not recorded, and plugin install/enable changes flush through `plugins`, see §7a).

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

**Settled in M0.** wasmtime-wasi 50 lets the host link the subset per interface (no
fallback to a deny-all `WasiCtx` was needed). `wasmtime_wasi::p2::add_to_linker_async` is
*not* used because it also links `wasi:sockets` (wasmtime-wasi-50.0.0 `src/p2/mod.rs:314-365`). Linked interfaces:
`wasi:io` (via `wasmtime_wasi_io::add_to_linker_async`), `wasi:clocks/{wall-clock,monotonic-clock}`,
`wasi:random/{random,insecure,insecure-seed}`,
`wasi:cli/{exit,environment,stdin,stdout,stderr,terminal-input,terminal-output,terminal-stdin,terminal-stdout,terminal-stderr}`,
`wasi:filesystem/{types,preopens}` (no preopens configured). `wasi:sockets/*` and
`wasi:http/*` are not linked, so such a component also fails to instantiate even if
inspection were bypassed.

stdout/stderr are captured into a buffer capped at **64 KiB per stream per call** (the
rest is discarded and counted), then logged through `tracing` with control and bidi
characters escaped (no log injection). Guest `log` calls are capped at **4 KiB per message
(`LOG_MESSAGE_CAP`) and 100 calls per hook call (`LOG_CALLS_MAX`)** in `src/output.rs`, also escaped.
Guest-derived failure text (a `plugin-error` message, wasmtime error chains that can carry
guest name-section frame names, install-time import/world names) is cut to **1 KiB
(`GUEST_TEXT_CAP`)** and escaped the same way before it is logged, recorded or returned
in a `PluginError`.

## 6. Execution model & limits

### Engine (one per process)

- `Config`: component model on, `consume_fuel(true)`, `epoch_interruption(true)`,
  optional pooling instance allocator (`HostConfig.pooling`, default **off**, see
  Measured below). `Config::async_support` is not needed in wasmtime 50.
- An epoch ticker thread calls `engine.increment_epoch()` every **5 ms**.
- **Status: not implemented yet (unchanged in M1c-2)** — compiled components are compiled in `install` each time (at `serve` startup). Planned: cached on disk keyed by
  `(artifact hash, wasmtime version, engine config hash)` via
  `Component::serialize` / `deserialize` (deserialize is `unsafe` — only for files we
  wrote; cache dir is ours).

### Per call (P3, P4)

```
store = Store::new(&engine, HostState { plugin, grants, limits, budget, req })
store.limiter(|s| &mut s.limits)                 // StoreLimits: memory_size, instances=3, tables=2, memories=1, table_elements=10_000
store.set_fuel(fuel_per_call)                    // instruction budget
store.fuel_async_yield_interval(Some(100_000))   // cooperative yield to tokio (was 10_000, see below)
store.epoch_deadline_trap(); store.set_epoch_deadline(deadline_ticks)
instance = instance_pre.instantiate_async(&mut store)      // ~20 µs measured
result = tokio::time::timeout(hard_deadline, guest.call_apply_filter(&mut store, …))
```

- **Fresh instance per call** (the only mode). Guarantees no state leaks between requests
  and matches P4. M0 measured it (below): P8 is met by a wide margin, so the
  reuse-per-worker fallback is **not built**.

**Measured (M0, `cargo bench -p ferrite-plugin-host --bench filter_call`, release,
Apple M3 Max dev laptop, wasmtime 50.0.0, criterion medians, hello `render.head` filter
including `apply_filter` registry lookup, instantiate, call, output validation; machine
was busy, load average 27-60, so treat as upper-ish bounds):**

| Benchmark | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| `hello_render_head_fresh` (default allocator) | 32.5 µs | 26.8 µs | 26.9 µs |
| `hello_render_head_fresh_pooling` | 25.6 µs | 23.7 µs | 24.2 µs |
| `instantiate_only` (excludes WASI setup: unused imports defined as traps, no `HostState`) | 24.2 µs | 20.0 µs | 20.3 µs |

P8 (< 1 ms) is met by ~35x. Pooling saves only ~3 µs (~10%) and reserves virtual address
space for a fixed slot count (calls beyond `POOL_SLOTS = 256` concurrent instances fail
to instantiate), so **pooling defaults to off**; `HostConfig.pooling = true` enables it
(slots sized to the StoreLimits above, `max_memory_size` = the memory cap). Revisit
under real concurrent load in M1.
- `hard_deadline` (tokio timeout, = deadline + 50%) is the backstop for time spent in
  host imports (DB, HTTP), which epochs don't interrupt.
- Host-import budgets per call: `content` ≤ 100 calls, `kv` ≤ 100 ops / 1 MB, `meta.set`
  ≤ 1 MiB written (post id + key + value) with each value ≤ 64 KiB, `http` ≤ 3
  requests, each ≤ 5 s / 2 MB response; exceeding → the import returns an error, the
  guest decides. In M0, `meta`, `options` and `settings` calls also count against the
  `content` budget (conservative; spec named only content/kv/http). Guest-supplied list
  limits (`list-posts`, `kv.list-keys`) are clamped to 100.
- **StoreLimits (measured in M0)**: `instances = 3`, `tables = 2`, `memories = 1`,
  `table_elements = 10_000`. A `wasm32-wasip2` component built by Rust instantiates
  several core instances (main module plus WASI adapter shims), so `instances = 1`
  fails; **3 is the minimum that worked** for the example plugins. `table_elements`
  bounds a guest `table.grow` (otherwise host OOM).
- **WASI host allocations**: the per-store `ResourceTable` and wasmtime's store-wide
  component handle count are both capped at **1_000** (`MAX_RESOURCES`; wasmtime's
  defaults are 1_000_000 and 100_000), and `wasi:random` byte requests at **64 KiB**
  (`max_random_size`; default 64 MiB). Past either cap the call fails (counted as `Trap`).
- **Fuel yield interval**: `100_000`, not the 10 000 first drafted here. At 10 000 a
  one-off measurement during Task 6 (no bench covers it) showed the yield bookkeeping slowing a busy-loop guest by about 3.5x, which broke the
  fuel-exhaustion test (< 200 ms). Wall-clock stays bounded by the epoch deadline
  regardless; the cost is coarser cooperative yielding under many CPU-bound plugins.
- **Manifest limits (ruling R3)**: effective limit = min(manifest `[limits]`, host
  default). A manifest requesting more than the default fails `install`; the admin
  approval flow for higher values is post-M0 (M1 checklist).
- Data crossing the boundary is copied (Component Model canonical ABI). Strings/lists
  from the guest have size caps (e.g. `head` ≤ 64 KB total, `html` ≤ 4× input or 2 MB).

### HTTP capability details

*M0 status: the `http` and `mail` host imports are stubs returning `unavailable`; the rules below are the M1 design. Only the per-call request budget (3) is enforced.*

- HTTPS only; host must exactly match the manifest list; redirects followed only to
  allow-listed hosts (max 3).
- Resolved IPs in private/loopback/link-local/metadata ranges are refused (SSRF / DNS
  rebinding protection) — check after resolution, connect to the checked IP.
- `User-Agent: ferrite/<ver> plugin/<name>`; no cookies, no ambient credentials.

## 7. Output validation

As built (M1c-2; M0 ruling R4 closed). The host validates every filter result
(`crates/ferrite-plugin-host/src/validate.rs`); a result that fails is dropped, the previous
value is passed on and the call counts as a failure (§8).

- **Variant and size.** The result must be the variant the hook works on. `head` ≤ 64 KiB
  of serialised strings plus 64 B per meta/link/script, and at most 256 metas, 64 links,
  16 scripts; `html`/`text` ≤ max(4 × input, 64 KiB), never above 2 MiB.
- **`html` sanitising (ruling R22).** `html` results (`content.body`; `render.footer` is in
  the catalogue but not wired into rendering yet) are sanitised with `ammonia` unless the
  plugin declares `render.html = "raw"`: ammonia's defaults (no `<script>`, `<style>`,
  `<iframe>`, `<form>`, `<svg>`, `<math>`, no `on*` handlers, no `javascript:`/`data:` or other
  non-allow-listed URL schemes), `rel="noopener noreferrer"` on every link, plus `a[target]`
  and `code[class]` restricted to `language-*` classes. Each non-raw plugin's whole output is
  re-sanitised after it runs, so markup a `raw` plugin added earlier in the chain survives
  only if no non-raw plugin runs after it. Raw HTML is meant to be shown as a high-risk
  permission at install (M2 admin).
- **`head` is structured.** The host never accepts HTML for `<head>`: the HTTP layer renders
  the tags itself (`crates/ferrite-http/src/head.rs`). Every attribute value and the title are
  escaped (`& < > " '`). A meta needs a `name` or a `property`. A link needs an `http(s)://` or
  root-relative (`/…`, not `//…`) `href`, and `https://` or root-relative when its `rel` loads a
  resource (stylesheet, preload, modulepreload, prefetch, icon, apple-touch-icon, manifest).
  `script-tag.src` must be `https://…`, on the site's own origin, or root-relative, with no
  backslashes, whitespace or control characters. `script-tag.json-ld` must parse as JSON; it
  is re-serialised compactly with every `<`, `>` and `&` written as `\u00XX`, so neither
  `</script>` nor `<!--` can occur inside the element. Anything else is dropped and logged.
- **Core head first.** Plugins receive the core SEO head (title, description, canonical, Open
  Graph; [06 §5](06-themes-rendering.md#5-rendering-pipeline-as-built)) as the input of the
  `render.head` chain, so a plugin can extend or replace tags but its output still goes
  through the rules above.

## 7a. Data visibility and caching

As built (M1c-2), in `crates/ferrite-http/src/plugins/` over the store-backed
`StoreHostServices` (plan ruling P3).

- **What `content.read` sees**: published items with `published_at <= now`, not media, read
  through `SiteReader` (`get-post`, `get-post-by-slug` with a page path such as
  `"parent/child"` for hierarchical types, `list-posts`), restricted by the host to the types
  the manifest lists (M0 ruling R6). Never drafts, pending, private, scheduled or trashed
  items, and never user e-mails or auth data: `Post.author` is the author's display name and
  `Post.url` is absolute. `meta.get` works only on items `content` could return.
  `options.get` never returns `plugin.*` keys, whatever the manifest grants (a plugin reads its
  own settings through `settings.get`, option `plugin.<name>.settings.<key>`).
- **Preview scoping.** Per-request state travels in a tokio task-local set around each filter
  chain (`run_filter`). During an authenticated preview, `get-post` and `meta.get` on *that one
  item* return it, so plugins render previews faithfully. The preview item can only come from
  `SiteReader::preview` (its `can_read` check, R17), so a public request cannot put a draft in
  scope. `/_preview/<id>` (06 §3) serves previews; their responses are never cached and
  `request-info.is-logged-in` is `true`. On other pages `is-logged-in` is `true` for requests
  with a valid session cookie (P21).
- **No writes on the render path (ruling R20).** `meta.set` during a render-class filter
  (`render.head`, `content.body`, `content.excerpt`, `render.footer`) returns
  `"meta.set is not allowed during render filters"`, is logged and does not use budget.
  Writes are allowed in actions and routes; `meta.set` there emits `invalidate` with
  `post:<id>` in the same transaction.
- **Dependency recording.** The host records inside the import implementations, so a plugin
  cannot under-report (`crates/ferrite-plugin-host/src/trace.rs`): `get-post` records
  `post:<id>` of the requested id, found or not; `get-post-by-slug` `archive:<kind>` plus the
  returned post; `list-posts` `archive:<kind>` plus each returned post; `meta.get` `post:<id>`;
  `options.get` `option:<key>`. The HTTP layer adds the keys `SiteReader` reports beyond those
  (`user:<id>`, `term:<id>`, ancestors). Only calls that pass their capability and budget
  checks are recorded. The page's keys are merged with the plugin trace
  (`PluginHost::apply_filter_traced`), and every page also records `plugins`. (The finer
  `plugin:<name>` key of §4c is not recorded; plugin changes flush through `plugins`.)
- **Volatile output (ruling R19).** A filter call that uses `kv` or `http`, reads the wall
  clock (`wasi:clocks/wall-clock.now`; the monotonic clock is not volatile), **or fails** marks
  the page volatile, and it is cached for at most 60 s (`SHORT_TTL` in
  `crates/ferrite-http/src/app/state.rs`; the planned `plugins.volatile_ttl` setting does not
  exist yet). No WIT change needed.
- **Render deadline (ruling R18).** Render-class hooks get a 200 ms epoch deadline by default
  (`plugins.render_deadline_ms`), with the tokio hard timeout at 1.5 × that. The epoch
  deadline includes time a call spends awaiting host imports, which on Postgres in debug builds
  measured about 80 ms for a plugin making a dozen reads.
- **Loading (plan ruling P3, ruling R21).** At startup `serve` installs every enabled
  `plugins` row from its artifact in the media store: the bytes must match the row's hash, the
  `manifest` column (the `ferrite-plugin.toml` table as JSON) is converted back to TOML and
  validated with `Manifest::parse`, and its `plugin.name` must equal the row's name. A
  non-null `granted` column must be a superset of the manifest's capabilities or the plugin is
  skipped (so `granted = {}` skips every plugin that asks for any capability); a null
  `granted` means the manifest is honoured as written (M1c-2 rows are written by operators and
  tests; the M2 install flow always sets it). A row that fails any step is logged and skipped;
  it never stops the site. Hot reload on `plugins.changed` is M1.5; plugin install CLI/API is
  M2.

## 8. Failure isolation (P5, P6)

| Failure | Detection | Effect on request |
|---|---|---|
| Trap (panic, unreachable, OOB) | `Err(Trap)` | hook skipped, previous value used |
| Out of fuel | `Trap::OutOfFuel` | same |
| Epoch deadline / hard timeout | trap / `Elapsed` | same |
| Memory limit (grow fails → guest aborts) | trap | same |
| Wrong variant / oversize output | host validation | same |
| Instantiation failure (missing import) | `Err` at instantiate | plugin marked broken on this node |

**Status (M1.5, P20)**: failures are classified and counted in memory per host
(`PluginHost::failures`, `is_enabled`, `disabled_reason`; threshold and window from
`HostConfig`); a plugin crossing the threshold on one host is disabled there at once,
until it is prepared and committed again. Every failure is also reported, synchronously
and without blocking, to the host's `FailureSink`. The node's sink,
`ferrite_http::SharedFailures`, buffers `(plugin, hook) → count`; its flusher (every 5 s,
plus a final flush on shutdown) adds the counts to 1-minute `plugin_failures` buckets
(bucket = database clock floored to the minute) in one transaction. Each buffered count
carries the plugin row's `updated_at` that the node's `PluginReloader` last committed
(`with_failures`). Counts recorded against an older row version, or for a disabled or
deleted row, are dropped, so a plugin enabled again never gets its pre-enable failures
back. For each plugin written (and any whose previous check errored, retried even with
no new failures), the flusher reads the row's `updated_at` as an optimistic token
(ruling R12) and sums the buckets starting at or after the start of the bucket holding
`now - plugins.failure_window` (floored, so a window shorter than a minute still sees
the current bucket). At `≥ plugins.disable_after_failures` it runs
`PluginRepo::auto_disable(name, reason, token)`:

```sql
UPDATE plugins
SET enabled = false, disabled_reason = 'auto: <n> failures in <window>', …
WHERE name = $1 AND enabled AND updated_at = $token
```

A re-enable racing the check makes it a no-op. Only when that statement made the
transition does the flusher append `plugin.disabled` (keys `[name, reason]`) in the same
transaction, so exactly one node emits it however many cross the threshold. That node
also disables the plugin locally at once; the others do on the event. Enabling a plugin
(`ExtensionService::set_plugin_enabled(name, true)`) clears its buckets in the same
transaction, and so does an install that changes an existing row's artifact.
**Accuracy limits:**

- The cluster-wide disable lags the threshold by up to one flush.
- A crash loses up to one flush interval (5 s) of buffered counts.
- A counts commit that reports an error after it was in fact applied is retried with
  the same counts, which then count twice.
- The flusher reads the rows with `PluginRepo::get_for_update` (Postgres
  `SELECT … FOR UPDATE`, in name order; SQLite a plain read under the single writer).
  A re-enable, which clears the counters, waits for the flush and never lands between
  the read and the add.

**Wiring obligations.**

- A node that shares failure counts must attach its `SharedFailures` to its reloader
  (`PluginReloader::with_failures`).
- Without it, counts carry no row version and are accepted whenever the row is enabled.
  The reloader logs a debug line for each such reload.

**Mixed plugin versions per page (ruling R13).** Each hook call loads the registry.

- A page rendered across a swap can run early hooks on a plugin's old version and later
  hooks on its new one. This is accepted.
- The node flushes the page cache after each committed reload (P18), which drops such
  pages.

**Retention.** Buckets are pruned hourly by the `plugin_failures.prune` job after
`max(24 h, 2 × plugins.failure_window)`. Metrics are M4. "Plugin marked broken on this
node" for instantiation failures is not separate: it counts as a failure like any other.

Every failure (target design): `tracing::warn!(plugin, hook, kind, duration, fuel_used)`, metric
increment (M4), and an upsert into `plugin_failures` as above. Admin sees the reason and
can re-enable.

## 9. Host-side API (Rust)

**M0 API** (`crates/ferrite-plugin-host`): `PluginHost::new(HostConfig, Arc<dyn HostServices>)`,
`install(Manifest, &[u8])`, `apply_filter(&str, FilterValue, &RequestInfo) -> FilterValue`
(never fails), `dispatch_action`, `handle_route`, `is_enabled`, `disabled_reason`,
`failures`; `inspect` is a free function. Hooks are `&str`, not a `HookName` type.

Target shape (the `reload`, DB-backed `InstalledPlugin` and http client parts are M1):

```rust
pub struct PluginHost { /* engine, registry: ArcSwap<Registry>, store access, http client */ }

impl PluginHost {
    pub async fn apply_filter(&self, hook: HookName, value: FilterValue, req: &RequestInfo)
        -> FilterValue;                                   // never fails; failures logged
    pub async fn dispatch_action(&self, plugin: &str, hook: HookName, ev: ActionEvent)
        -> Result<(), PluginError>;                       // used by job worker (retry on Err)
    pub async fn reload(&self, plugins: Vec<InstalledPlugin>) -> Result<(), PluginError>;
}
```

## 10. Guest SDK (`ferrite-sdk`)

- Wraps `wit-bindgen` generated bindings; exposes `Head`, `Post`, `Ctx` types and one
  declarative macro, `export_plugin!` (no proc-macro crate, no registration magic).
  Hooks are plain functions; the macro maps hook names to them:

```rust
use ferrite_sdk::{ActionEvent, Ctx, Head, Result};

fn head(mut h: Head, ctx: &Ctx) -> Result<Head> {
    if let Some(post) = ctx.current_post() {            // requires content.read
        h.meta("description", &post.excerpt);
    }
    Ok(h)
}

fn ping(event: ActionEvent, ctx: &Ctx) -> Result<()> {
    // ctx.http().fetch(&request)?;                      // requires http
    Ok(())
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
    actions: { "post.published" => ping },
    // routes: { "/endpoint" => handler },
}
```

- Filter functions take `Head` or `String` (html/text) and return the same type; the
  macro dispatches on the incoming `filter-value` variant and a mismatch yields
  `invalid-input`. Unknown hook names yield `invalid-input`; `handle_request` with no
  matching route returns a 404.
- Build (**settled in M0**, ruling R1): `cargo build --release --target wasm32-wasip2`
  with an explicit `--target` produces a component directly; `cargo component` is not
  needed. (`plugins/.cargo/config.toml` sets the target only for CWD inside `plugins/`.)
- *(Not in M0: the `ferrite` CLI does not exist yet.)* `ferrite plugin new <name>` scaffolds a plugin crate + manifest;
  `ferrite plugin check <dir>` runs the install-time validation locally.

## 11. Versioning (P9)

- WIT package versions follow semver. Additive changes (new hook names, new optional
  imports) → minor bump (`0.2.0`); since WIT records aren't extensible, adding a field
  to a record is a **breaking** change → new major world.
- The host embeds bindings for every supported world version (`bindgen!` per version
  module) and adapts domain types into each. Support window: current + previous major.

## 12. Hostile test plugins (required by CLAUDE.md)

In `plugins/examples/hostile/`, each with an integration test asserting the page still
renders and the failure is recorded (state as of M0):

| Plugin | Behaviour | Expected |
|---|---|---|
| `loop` | never returns | fuel exhaustion under default limits; page renders |
| `spin-host-fuel` | never returns; test raises fuel | only the epoch deadline stops it |
| `oom` | allocates 1 MiB blocks until the limit | trap; page renders |
| `panic` | `panic!()` on every call | trap; page renders |
| `undeclared-import` | calls `http` without declaring it | install rejected; forced link fails to instantiate |
| `sockets` | opens a TCP socket (`wasi:sockets`) | install rejected |
| `big-output` | returns 3 MiB of HTML (fits the 32 MiB memory cap) | rejected by the 2 MiB output cap (`InvalidOutput`) |
| `slow-import` | spends its call in a slow host import | hard timeout; page renders |
| `wrong-variant` | returns `text` for `render.head` | ignored, recorded |
| `fs-escape` | reads `/etc/passwd` | no preopens → error inside guest; nothing leaks |
| `flapper` | fails every call | auto-disabled after threshold |
| `noisy` | floods stdout (10 MiB) / stderr (1 MiB) | capture capped at 64 KiB per stream; call succeeds |
| `fat-init` | initial linear memory 40 MiB (above the 32 MiB cap) | instantiation fails; treated as a call failure |
| `kv-budget` | `kv.set` 150 times | 101st+ operation refused (budget 100) |
| `long-error` | fails with a 1 MiB `plugin-error` full of newlines and ANSI escapes | logged/recorded text cut to 1 KiB and escaped; page renders |
| `resource-flood` | leaks 5 000 `wasi:cli/stdout` handles | resource cap (1 000) fails the call; host keeps serving |
| `route-xss` | route returns HTML with `<script>` reading `document.cookie` | **M2 — not yet implemented** (needs mounted routes) |
| `route-flood` | many POSTs to its route | **M2 — not yet implemented** (needs rate limiting across 2 nodes) |
