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
upload .wasm + manifest ──► POST /plugins/inspect: validate manifest, inspect imports/exports,
     compile on a scratch host ──► permission report + fingerprint (nothing stored)
     ──► admin approves: POST /plugins with approve = fingerprint ──► same checks again,
     store artifact + plugins row (new: disabled) + plugins.changed in one Tx
     ──► POST /plugins/{name}/enable ──► every node: fetch artifact → compile → registry swap
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
  a newer event arrives (R14).
- **Install pipeline (as built, M2a-2a P3, P8).** `ferrite_http::extensions::Installer`
  is shared by the admin API and the CLI (one per node in `serve`).
  `Installer::inspect_plugin` refuses a component over 32 MiB (`MAX_PLUGIN_BYTES`) or a
  manifest over 64 KiB (`MAX_MANIFEST_BYTES`) before parsing, parses the manifest with
  `Manifest::parse`, builds the permission report against the installed version's grant,
  and compiles the component on a scratch host (in-memory services, never committed) with
  the requested limits treated as approved, so a component that cannot load is refused
  now. Compiles run on the blocking pool, at most two at once per node (`COMPILE_SLOTS`),
  and at most four uploads are read and processed at once per node (`UPLOAD_SLOTS`; a
  fifth is `503 busy`). Nothing is written by an inspection. `Installer::stage_plugin`
  needs the report's fingerprint, uploads the component and returns the rows; the row's
  `granted` is always an object (`Grant::of(manifest)`, closing M1c-2 R21's open item).
  The rows are recorded by `ExtensionAdmin::install_plugin` (API: `manage_plugins` +
  `extensions:write`, audit row `plugin.install`) or `ExtensionService::install_plugin`
  (CLI, operator path, no audit), under the `extensions` lock, with `plugins.changed`, in
  one transaction. When the API's record step fails (for example `503 busy`), the handler
  deletes the uploaded bytes again unless an artifact row names them
  (`Installer::discard_staged`, under the `extensions` lock); anything else left behind
  waits for the artifact GC (M4). Residual race (accepted): a concurrent install of the same bytes
  that uploaded them but has not yet recorded its rows loses its blob, and that plugin
  fails to load until it is reinstalled; closing it needs a GC grace period (M4).
- **Permission report and approval (P3, P6).** `Manifest::permission_report` lists every
  capability as a plain-language line (`key`, `text`, `detail`, `risk`: `low`, `medium` or
  `high`; the API also sends `high_risk` = `risk == "high"`): high risk are `content.read`,
  `content.meta.read` and `options.read` with `"*"`, `content.meta.write` outside the
  plugin's own `plugin.<name>.*` keys, every `http` host, `render.head` (a head script, from
  any `https://` host, runs with the site's origin on every page, as a WordPress plugin's
  would, so it can act as a logged-in admin who views the site; ruling R22), `render.html =
  "raw"`, every route, `mail`, and every limit above its default. Each action hook in
  `[hooks]` is an item too (`key = "hooks"`, `detail` = the hook name; R22): `request.served`
  is medium risk (it receives every page view: path, status, time, referrer host, browser
  class, cache outcome), `post.saved`, `post.published` and `user.created` are low (an item
  or user id). `added` lists what the manifest asks for beyond the installed version's
  grant (a first install: everything). The `request.served` subscription is part of the
  grant (ruling R23): an update that adds it lists it in `added` and has a new fingerprint,
  so an earlier approval does not install it. The low-risk hooks are not part of the grant:
  on an update they are never in `added` and they do not change the fingerprint.
  The `fingerprint` is the lowercase hex SHA-256 of the compact JSON `{"artifact",
  "grant", "plugin", "version"}` (`grant` = `Grant::of(manifest).to_json()`; serde_json
  maps are sorted and a unit test pins one value). `POST /plugins` and `ferrite plugin
  install` install only when the approval is the fingerprint of *this* upload, so an
  approval never covers other bytes or another grant; a mismatch is `422 validation`
  with `errors[0].field = "approve"` and the upload's report, and nothing changes.
- **Grant (stored form v1).** `plugins.granted` = `{"v":1,"capabilities":<[capabilities]
  table, non-default members only>,"limits":{"memory":n|null,"fuel":n|null},"routes":
  [{"cross_origin","rate_limit"}]}`, plus `"hooks":["request.served"]` only when the plugin
  subscribes to `request.served` (R23; left out otherwise, so grants and fingerprints without
  it are unchanged) (`ferrite_plugin_host::Grant`); the capabilities are
  read back through `Manifest::parse`, so a grant obeys every manifest rule. Route
  `cross_origin` and `rate_limit` are part of the grant (relaxing one is widening, ruling
  R7); route paths are not, since every route lives under the plugin's own `/_p/<name>/`
  (R6). The M1c-2 form (a bare `[capabilities]` table) is still read, with no approved
  limits and no route policies, so a legacy row with routes fails closed. A **null**
  `granted` (operator rows from before M2a-2a) counts as `Grant::legacy(manifest)`: the
  manifest's capabilities and `request.served` subscription, with `[limits]` capped at the
  host defaults (logged) until an admin approves more; a reinstall writes a real grant.
- **Install-time check (P2)**: walk the component type's imports. Allowed:
  - `ferrite:plugin/*` interfaces that the manifest's capabilities map to (§4),
  - the WASI subset in §5.
  Anything else → reject with the exact import name. Exports must include the
  `ferrite:plugin/guest` interface of a supported world version. A manifest may declare
  more than the component imports, not less.
- **New install (F38).** A new plugin is installed **disabled**; `POST
  /plugins/{name}/enable` (or `ferrite plugin enable`, or `install --enable`) enables it,
  like WordPress's "activate". Enabling clears its shared failure counters.
- **Update (as built, ruling R1).** An upload for an installed name is an update: the
  report's `added` shows what it asks for beyond the installed grant (`update: true` in the
  inspect response; on a stored row's report `update` is always `true`, since that report
  is computed against the row's own grant). Like every install it needs the fingerprint
  of its own report, so a widening update without that approval is refused, nothing is
  stored and the installed version keeps running. There is no "stored but not activated"
  version and no approve-later queue: the operator re-submits with the approval. A
  replaced plugin keeps its enabled state and gets the new grant; a new artifact clears
  its failure counters.
- **Disable/enable/uninstall (as built, P9, R2).** `POST /plugins/{name}/enable|disable`,
  `DELETE /plugins/{name}[?purge=true]`, `ferrite plugin enable|disable|remove [--purge]`.
  Delete removes the row and keeps the plugin's KV and settings for 30 days
  (`PLUGIN_DATA_RETENTION`): the delete `Tx` also clears the failure counters, writes the
  marker option `plugin.<name>.deleted_at` (unix micros, hidden by the `plugin.` rule of
  `options.get`), enqueues a `plugin.purge` job due 30 days later (unique key
  `plugin.purge:<name>:<deleted_at micros>`) and emits `plugins.changed`. The purge runs
  only when no row of that name exists and the marker still names that deletion, under
  the `extensions` lock, so a reinstall within 30 days (which removes the marker) keeps
  the data and only the latest deletion's job purges. With `purge` the data is deleted
  right after the commit and the job, due 2 minutes later (`PLUGIN_PURGE_GRACE`), sweeps
  KV a node wrote before it reloaded and deletes the marker. A daily `plugin.purge_sweep`
  re-enqueues the purge of every marker past the retention, so a purge job that went dead
  is retried. Setting keys a later version dropped from its schema are purged by that
  sweep only.
- **Plugin data is bound to the plugin name only (R22).** KV, settings and sealed secrets
  (sealed under `plugin-setting:<plugin>:<key>`) belong to the name in `plugin.name`; the
  manifest's `publisher` is unverified text, and nothing compares it on an update or a
  reinstall. A component from another publisher installed under the same name (an update,
  or a reinstall within the 30 days) reads the earlier plugin's data and secrets, and the
  report shows no publisher change. Approve an update or a reinstall only from the source
  you installed from; signed plugins with a publisher identity are an M4 item (ROADMAP).

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

[limits]                                    # may lower the site defaults, or request higher
memory = "16MB"                             # values the admin must approve (≤ the ceilings)
fuel   = 20_000_000
```

Validation: unknown keys are errors; hooks must exist in the declared world version and
require their matching capability (e.g. `render.head` hook needs `render.head = true`).

As built (M2a-2a), `Manifest::parse` also enforces:

- **Size and name.** A manifest over 64 KiB (`MAX_MANIFEST_BYTES`) is refused unparsed.
  The plugin name `inspect` is reserved (`RESERVED_PLUGIN_NAMES`; `/plugins/inspect` is an
  API route, ruling R9).
- **`http` hosts (P7, pre-flight F5).** Plain hostnames only (no scheme, port, path or
  wildcard), stored lowercased with one trailing dot dropped and duplicates removed, so
  grants and fingerprints do not depend on spelling. Refused as bare: anything that
  parses as an IPv4 address, a name without a dot (single labels resolve through search
  domains), a last label that is all digits (`127.1`, `0177.0.0.1`), a label starting
  `0x`, a label `localhost`, and a first label starting `localhost`
  (`localhost4.localdomain4`); IPv6 literals already fail the hostname rule. The
  resolved-address (SSRF, DNS rebinding) check is the `http` import's (M2a-2b, §6 "HTTP
  capability details").
- **`[routes]`.** Keys start with `/`; `methods` a non-empty list of uppercase HTTP
  methods (default `["GET"]`); `rate_limit` is
  `N/s`, `N/min` or `N/h` with `N > 0` (default `10/min`, `DEFAULT_ROUTE_RATE_LIMIT`);
  `cross_origin` a bool.
- **`[settings]` keys** are `[a-z0-9_-]{1,64}` (the theme rule, F23), since they become
  option names, seal purposes and `errors[].field`.
- **`[limits]` (P5).** A request at most the host default always loads. Above the default
  it needs the admin's approval (a high-risk report line; stored in `granted.limits` and
  applied by `PluginHost::prepare_with`), up to the ceilings: memory 256 MiB
  (`LIMIT_CEILING_MEMORY`, never below the default) and fuel 10 × `plugins.default_fuel`
  (`LIMIT_CEILING_FUEL_FACTOR`). On a pooling host (`HostConfig.pooling`) the memory
  ceiling is the default itself, because the pool's slots are that size, so a report never
  offers what the node cannot load (review round 1 of Task 2). Above a ceiling is refused
  at inspection, approved or not.

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

*Built in M2a-2a (P21, Task 12): `ferrite_http::ServedBuffer`, wired into `serve`.*

Fired for every public page response **including cache hits**. Events are buffered in
memory per node and flushed every 10 s (or 1 000 events) as **one** `plugin.action` job
per subscribed plugin — never one job per request. Payload contains no IP, cookie or
full user agent: only path, status, timestamp, referrer host, a coarse UA class
(`browser`/`bot`/`other`) and cache hit/miss. Lost on node crash (acceptable for
analytics; documented).

As built:

- **Recorded:** every response of the public handler after its health, asset and
  preview branches: content pages, archives, search, feeds, robots and sitemaps, 404
  and 410 pages, canonical and redirect-row redirects. **Not recorded:** non-GET/HEAD
  requests, `/healthz`, `/readyz`, `/_media` and `/_theme` assets, every path whose
  first segment percent-decodes to `_preview` in any case (also the 404 variants),
  plugin routes (`/_p/`) and the API (separate axum routes).
- **Event:** `{"path", "status", "ts", "referrer_host", "ua_class", "cache"}`. `path` is
  the raw percent-encoded path without the query, cut to 512 bytes (F36; a decoded
  path could hold a NUL and make the whole job payload invalid). `referrer_host` is the
  lowercased ASCII host of a single absolute `Referer` (≤ 253 bytes), else `null`.
  `ua_class`: `bot` if the user agent contains `bot`, `crawl`, `spider` or `slurp`
  (ASCII case-insensitive), else `browser` if it starts with `Mozilla/`, else `other`.
  `cache`: `hit` (this request did not run the render), `miss`, or `bypass` (logged-in
  request under `cache.logged_in = "bypass"`, or a redirect answered before the cache).
  No query string, IP or IP hash, cookie, header value, full user agent or user id.
- **Buffer:** a push under a lock held for that push only; at 10 000 buffered events
  new ones are dropped and counted. The request never waits for the store or a plugin.
- **Flush:** a per-node task flushes every 10 s, as soon as 1 000 events are buffered,
  and once more at shutdown (after the HTTP drain, inside the background-stop budget;
  the jobs then wait for the next runner). One transaction enqueues one job per enabled
  loaded subscriber on this node (`PluginHost::subscribers`) and per 1 000 events:
  payload `{"events": […]}`, subject `null`, priority -1. Without subscribers the
  events are discarded; without events nothing is enqueued. A failed enqueue loses the
  batch (logged, counted as dropped).
- **Delivery:** the `plugin.action` handler (04 §6) re-checks the row, calls the plugin
  bounded like every action (fuel, memory, action deadline), counts a failure towards
  auto-disable and retries; at least once, so a subscriber must tolerate a repeated
  batch. The hook needs no capability; what the plugin does with the events is limited
  by its own grant (`kv`, …), and the events name nothing of other plugins.
- Example subscriber: `plugins/examples/counter` (adds each batch's size to a KV count).

### 4b. Plugin routes

*Status (M2a-2a, as built): mounted under `/_p/` by `ferrite-http` (`app/plugin_route.rs`); example `router`, hostile `route-xss` and `route-flood` (§12).*

- Mounted at `/_p/<plugin>/<path>` (and `/_p/<plugin>/` for a route `"/"`) for every
  method, in front of the public fallback, so redirects and the page cache never see
  them: plugin responses are **never cached**. Errors are plain text, not problem
  documents.
- Only paths and methods listed in `[routes]`, matched exactly: the raw `<plugin>`
  segment must be a plugin name (never percent-decoded), the rest is percent-decoded
  but dot segments and `%2F` are not resolved, so a URL can only name a route of that
  one plugin. Checks, in the order the code runs them:
  1. plugin not loaded on this node, disabled on this node, no `routes` grant, or
     unknown path → `404`; known path, other method → `405` with `Allow`;
  2. **CSRF / origin**: methods other than GET/HEAD need `Origin` (else the
     `Referer`'s origin) equal to `site.url`'s origin, the API's rule, unless the route
     declares `cross_origin = true` → else `403`. A `cross_origin` request that fails
     the rule is passed on **anonymously** (`user = none`), so a same-site sender
     carrying the visitor's `SameSite=Lax` cookie never identifies them;
  3. no known client address → `503` (`serve` always knows the peer; without it the
     limit could only be one shared bucket);
  4. **rate limit** (P16): the route's `rate_limit` (`N/s`, `N/min`, `N/h`; default
     `10/min`) per client address (IPv6 per /64), fixed windows counted in the
     database (`AuthService::hit_limit`, bucket `route:<plugin>:<32 hex SHA-256 of the
     path>:ip:<keyed hash>`), so it holds across nodes → `429` with `Retry-After`.
     `cross_origin` and `rate_limit` are part of the approved grant (ruling R7); the
     paths are not (R6);
  5. the `plugins` row (`PluginRepo::is_enabled`, one column) missing or disabled →
     `404`: read per request, so a disable or delete answers `404` on every node at
     once, and after checks 2–4 so refused requests cost no read;
  6. more than 100 query pairs → `400`; body over 1 MiB → `413`.
- **Request**: headers passed are only `content-type`, `accept`, `accept-language`,
  `user-agent`, `referer` — the session cookie and `Authorization` are never passed.
  `user` is the id behind the `ferrite_session` cookie (bearer tokens never identify a
  visitor to a plugin; an account that must reset its password counts as anonymous;
  see check 2). `client-ip-hash` is per plugin: 32 hex of HMAC(subkey
  `ferrite:plugin-ip:v1`, `plugin ‖ 0x00 ‖ ip`), so plugins cannot join visitors
  across each other or with session rows.
- **Call**: route deadline 2 s (hard 3 s), fuel and memory as for every call. The wait for
  a call slot (§6) counts toward it: wait and call together take at most 3 s. The
  response must have status 200..=599, ≤ 32 headers, each name and value ≤ 4 KiB and
  valid header text, body ≤ 1 MiB (`validate_route_response`), else the call failed
  (`InvalidOutput`). Any failure (trap, timeout, invalid response) → `502`.
- **XSS containment**: the visitor gets the plugin's status and only its
  `Content-Type`, `Content-Language` and, on a 3xx, a root-relative `Location` (`/…`,
  visible ASCII; `//…`, `/\…`, a tab or an absolute URL makes the answer `502`). Always added:
  `Content-Security-Policy: sandbox allow-forms; form-action 'self'` (opaque origin, so
  scripts — if any — can't read site cookies or call the API as the user; forms may
  only post back to the site, so a plugin page on the site's URL cannot collect
  credentials for another host — ruling R14 of M2a-2a; CSP3 matches `'self'` against
  the response URL's origin, not the sandboxed document's opaque one). Because the document's
  origin is opaque, a browser sends `Origin: null` with its form posts, so check 2 refuses
  a post to a route without `cross_origin = true` (`403`); a form on a plugin page can only
  post to a `cross_origin` route, which is then served anonymously. This is deliberate:
  accepting `Origin: null` would let a sandboxed document on any other site post as the
  visitor. `X-Content-Type-Options:
  nosniff`, `Cache-Control: no-store`, `X-Robots-Tag: noindex`. Plugins can't set
  cookies, CSP, HSTS, CORS or caching headers. JSON and redirects (`303` to same-site
  paths) are the intended response types; HTML is allowed but sandboxed.
- Themes post forms to routes with a helper: `{{ plugin_route("contact-form", "/submit") }}`
  (06 §4).

### 4c. Plugin settings

*Status (M2a-2a, as built; P13, `ferrite_core::ext_settings`): the schema, the API, sealed
secrets and declared-keys-only reads are built; the admin form is M2b.*

- `[settings]` uses the same field schema as theme settings (`text`, `textarea`, `bool`,
  `number`, `select`, `color`, `media`, `secret`) — one validator in core
  (`ext_settings::validate_value`, over `ferrite_domain::setting`) and one renderer in the
  admin. Keys are `[a-z0-9_-]{1,64}` (§3).
- Stored in `options`, one option per set key: `plugin.<name>.settings.<key>`. `GET
  /plugins/{name}/settings` answers the schema, every non-secret value as the plugin reads
  it, for each `secret` only whether it is set (`secrets`), and the keys whose stored
  value no longer fits the definition (`invalid`). `PATCH` checks every entry against the
  row's schema first (an undeclared key or a bad value is `422`, nothing written), then
  writes all of them under the `extensions` lock in one `Tx`, with `options.changed` per
  key and one audit row `plugin.settings {name, keys}` (never values); `null` deletes a
  key's option. Reads need `manage_plugins` + `extensions:read`, writes `extensions:write`.
- **Secrets.** A `secret` value is stored sealed, `{"sealed": "v1.…"}`, under the site key
  with the purpose `plugin-setting:<plugin>:<key>` as associated data, so a stored value
  opens for that plugin and key only. The API never returns it after it is set, the audit
  never holds it, `options.get` never reads any `plugin.*` option, and only the owning
  plugin's `settings.get` opens it. A lost or changed site key makes stored secrets
  unreadable (they read as unset; admins re-enter them).
- **`settings.get`** (always linked) answers only keys the manifest declares (an
  undeclared key is `none` and costs no budget): the stored value if it fits the declared
  definition, else the declared default, else `none`. A stored value that no longer fits
  (its type changed in an update) or a sealed one that does not open answers the default
  and is logged, so it never reaches the guest. Values are read per call (no option
  cache, P14).
- Changing settings flushes pages through `options.changed`: the plugin's read records
  `option:plugin.<name>.settings.<key>`, so exactly the pages whose plugin calls read the
  key are dropped (the finer `plugin:<name>` key is not recorded, and plugin install/enable
  changes flush through `plugins`, see §7a).
- A deleted plugin's settings are kept 30 days with its KV (§2); keys a later version
  dropped from its schema are deleted by the daily purge sweep only.

Filters run in priority order (ties: plugin name), each receiving the previous output.
Actions run as `plugin.action` jobs ([04](04-cluster.md#job-queue)), never on the request
path.

*As built (M2a-2 P20):* `ferrite_core::actions::enqueue` runs inside the write's `Tx`
(content `after_save` → `post.saved`; publish and the scheduler's publish →
`post.published`; `UserService::create` / `OperatorUsers::create` → `user.created`), so
a rolled-back write enqueues nothing. It reads the `plugins` rows in that `Tx` and
enqueues one job per enabled row whose stored manifest lists the hook, in name order;
the subject is the item or user id and the payload is the catalogue's id object, never
content or secrets. A plugin reading the item through `content.get-post` sees what
the public site sees (a draft's `post.saved` subject is not readable). The binary's
`PluginActionHandler` re-reads the row (missing or disabled: dropped, P29), then calls
`PluginHost::dispatch_action`, which holds one loaded instance for the whole call
(R13), bounded by fuel, memory and the action deadline; outcomes in 04 §6.
`PluginHost::subscribers(hook)` lists the enabled loaded subscribers in priority order.
**Actions are delivered at least once:** a job whose lease lapses (a stalled node) runs
again on another node, and a call that failed after a side effect (a `meta.set`, a KV
write) is retried, so an action handler must be idempotent (e.g. set a value rather than
append, or key KV writes by the event's subject). Action jobs run at priority -1, below
core jobs, so slow actions never hold up scheduled publishing.

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
- **Status: not implemented yet (unchanged through M2a-2a)** — components are compiled on every load (`serve` startup and each reload that prepares a row). Planned: cached on disk keyed by
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
- **Call concurrency cap (P22, M2a-2a).** One semaphore per host, `plugins.max_concurrent_calls`
  permits (`HostConfig.max_concurrent_calls`, default `max(8, 4 × available_parallelism)`, > 0;
  with pooling at most `POOL_SLOTS` = 256). Every filter, action and route call takes a permit
  before it instantiates and holds it for the whole call; it waits at most its deadline for one.
  No permit in time → the call is skipped: a filter is skipped and the page volatile, an action
  or route gets `PluginError::Busy` (route → `503` + `Retry-After: 1`, action job →
  `JobError::Busy`: released with the attempt refunded, due again in 1–5 s, R20(3)). A
  `Busy` skip is **never** counted as a plugin failure: it is load, not misbehaviour, so a flood
  of route requests cannot auto-disable a healthy plugin (Task 8 m7). The wait counts toward an
  action's or route's deadline (R22): its epoch and hard deadlines run from when it began
  waiting (`CallDeadline::after_wait`), so wait and call together take at most the hard deadline
  (1.5 × the deadline; a slot free at once gives the full deadlines). A call the wait shortened
  that then runs out of time is `Busy`, not a failure (`ended_by_budget`), like a budget cut.
- **Plugin actions and job workers (Task 11 carry).** At most `min(cluster.workers − 1,
  max_concurrent_calls)` (at least 1) `plugin.action` jobs call plugins at once per node
  (`ferrite::jobs::action_slots`, a slot in `PluginActionHandler`); one over that is released at
  once (`JobError::Busy`) instead of waiting, so slow plugins never hold every job worker and core
  jobs (scheduled publishing, purges) always find one. A busy release refunds the attempt
  (`JobRepo::release(…, count_attempt = false)`, delay `BUSY_RETRY_MIN` 1 s + up to
  `BUSY_RETRY_JITTER` 4 s), so a burst of saves never pushes actions to `dead`; the cost is
  re-claims every few seconds while the node stays saturated. With `cluster.workers = 1` the
  only worker can still be held (logged at boot).
- **Per-render budget (P22, F37).** A page's filters share `plugins.render_budget_ms` (default
  1 000, `HttpConfig.render_budget`), counted from when the page takes its render slot
  (`PluginRequest::until`, `run_filter` → `PluginHost::apply_filter_until`). Each call's epoch
  deadline is `min(hook deadline, until − now)` and its hard deadline `min(1.5 × hook deadline,
  until − now)`, so no call outlives the budget; waiting for a permit counts against it. Once
  the budget is spent the remaining plugins are skipped and the page is volatile. A call the
  budget ended is not counted (R20(4)): an epoch interrupt when the budget shortened the epoch
  deadline, a hard timeout when it shortened the hard deadline (`CallDeadline::ended_by_budget`);
  a call that ran out of its own deadline still is. `plugins.render_budget_ms` must be at most
  60 000 and at least `plugins.render_deadline_ms` (else every render call would be cut, and a
  looping plugin never counted); an end that does not fit an `Instant` means no budget.
- **Skip logging.** Busy and budget skips are warned at most once per plugin (busy) or hook
  (budget) per minute, with the number of skips since (`skipped_since_last_warning`), and logged
  at debug otherwise. A budget warning names `spent_by`, the plugin called last before the budget
  ran out in that filter chain (or "an earlier filter of the page").
- **Known limit: no fairness inside the budget.** Plugins run in priority order and the first ones
  may use the whole budget; a plugin that stays just under its deadline is never a failure, so a
  slow high-priority plugin can starve later ones (head and SEO plugins included) on every page,
  and on listings it runs per item. Renders, actions and routes also share one semaphore with no
  reservation for renders. `spent_by` is the only signal; per-plugin shares are a later item.
- **Cancel-safety.** A call's future can be dropped at any await: at the hard deadline, when an
  HTTP client disconnects, when a job loses its lease. The store (and its instance) and the
  permit are dropped with it, and nothing is recorded as a failure (failures are recorded only
  when a call returns). `HostServices` implementations must not hold a `Tx` across an await they
  cannot finish (a dropped `Tx` rolls back) and keep each write to one statement or one short
  `Tx`; `StoreHostServices::meta_set` is one `Tx` (value + `invalidate` event, all or nothing).
  Tested by dropping calls mid-import (`limits.rs` `dropped_call_frees_its_slot_and_instance`,
  `ferrite-http` `dropped_meta_write_is_all_or_nothing`).
- Host-import budgets per call: `content` ≤ 100 calls, `kv` ≤ 100 ops / 1 MiB, `meta.set`
  ≤ 1 MiB written (post id + key + value) with each value ≤ 64 KiB, `http` ≤ 3
  requests (each bounded to 5 s, 1 MiB out, 2 MiB in; see "HTTP capability details"),
  `mail` 1 per call; exceeding → the import
  returns an error, the guest decides. In M0, `meta`, `options` and `settings` calls also
  count against the `content` budget (conservative; spec named only content/kv/http). Guest-supplied list
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
- **Manifest limits (M0 ruling R3, as built in M2a-2a P5)**: the effective limit is the
  manifest's `[limits]` value when it is at most the host default (the default when none
  is given); above the default only when the stored grant approves at least that value
  (`granted.limits`) and it is within the ceiling (§3), else `prepare_with` refuses the
  plugin (`LimitAboveDefault` / `LimitAboveCeiling`). A legacy null grant approves nothing
  above the defaults: its limits are capped at them (§2). Approved limits raise the
  worst-case plugin memory per node to `max_concurrent_calls` × the largest approved cap.
- **Compile time is not bounded (known limit).** Inspecting an upload compiles a
  component of up to 32 MiB with no time limit; only the two compile slots per node
  (`COMPILE_SLOTS`, and four upload slots) bound how many run at once. Uploads are
  admin-only; a wasmtime compile-size or function-count guard is a later item.
- Data crossing the boundary is copied (Component Model canonical ABI). Strings/lists
  from the guest have size caps (e.g. `head` ≤ 64 KB total, `html` ≤ 4× input or 2 MB).

### HTTP capability details

*As built (M2a-2b Task 11, P16/P17).* `http` is linked only when the manifest lists hosts
(§3); every `http.fetch` call makes the page volatile (§7a).

**Host checks** (`state.rs`, in this order; each refusal is an error string to the guest,
never a plugin failure): the per-call budget (3 fetches); the method, exactly one of `GET
HEAD POST PUT PATCH DELETE`; the URL parses; scheme `https`; no userinfo; port absent or
443; the host, lowercased with one trailing dot dropped, exactly in the manifest's `http`
list (an IP literal never is); at most 32 request headers, each name + value ≤ 4 KiB, none
of `host cookie connection proxy-authorization proxy-connection transfer-encoding
content-length te trailer upgrade keep-alive expect` nor any `proxy-*` (ignoring case);
a body ≤ 1 MiB. The request then goes to `HostServices::http_fetch(plugin, hosts, req)`
with the canonical URL and the plugin's list (default: "http is not configured");
`StoreHostServices` hands it to the node's `Egress` (`ferrite-http`
`plugins/egress.rs`, built once in `serve`).

**Egress** (one reqwest 0.13 client per node, rustls with the platform verifier):

- *Resolved addresses (SSRF, DNS rebinding).* The client's resolver looks the name up once
  (`tokio::net::lookup_host`, port 0 so the URL's 443 applies) and refuses the whole
  answer when **any** address is not public; reqwest connects only to the addresses it
  returned (no second lookup), and the connected peer address is checked again. Refused
  (`is_public_ip`): IPv4 `0/8 10/8 100.64/10 127/8 169.254/16 172.16/12 192.0.0/24
  192.0.2/24 192.88.99/24 192.168/16 198.18/15 198.51.100/24 203.0.113/24 224/4 240/4`;
  IPv6 `::/96` (unspecified, loopback, IPv4-compatible), `64:ff9b:1::/48`, `100::/64`,
  `2001::/32`, `2001:2::/48`, `2001:db8::/32`, `3fff::/20`, `fc00::/7`, `fe80::/10`,
  `fec0::/10`, `ff00::/8`; IPv4-mapped `::ffff:0:0/96`, NAT64 `64:ff9b::/96` and 6to4
  `2002::/16` are judged by the IPv4 address they embed. An IP-literal URL host is refused
  before any lookup.
- *Redirects.* reqwest follows none (its redirect policy cannot see the calling plugin's
  list, pre-flight F27); `Egress::fetch` follows at most 3, each `Location` joined to the
  current URL and checked like the first URL (listed host, `https`, port 443, no
  userinfo, no IP literal), else "redirect not allowed" (also for a 4th hop). 301/302/303
  continue as `GET` without a body (`HEAD` stays `HEAD`), 307/308 keep method and body;
  `Authorization` is dropped when the host changes. A `Location` is never followed
  outside this loop.
- *Bounds.* 5 s for the whole fetch (connects, hops and body); the response body is read
  in chunks up to 2 MiB (one byte more, or a larger `Content-Length`: "response body over
  2 MiB"); at most 64 response headers, `Set-Cookie` dropped. The plugin call's own
  deadline still applies: on the render path (200 ms) a slow fetch ends the call (a
  timeout, counted toward auto-disable like any other, §8).
- *No ambient state.* No proxy (`HTTP_PROXY` and the like are ignored), no cookie store,
  no `Referer`, no decompression; `User-Agent: ferrite/<version> plugin/<name>`, which
  the guest cannot replace.
- *Errors* name the class only ("address not allowed", "redirect not allowed", "timed
  out", "TLS error", "connection failed", "request failed", …), never a resolved
  address; details go to the debug log.

Tests: `ferrite-plugin-host` `tests/capabilities.rs` `fetch_preflight_refusals`;
`ferrite-http` `plugins/egress.rs` (unit `public_ip_table`) and `tests/plugins/egress.rs`
(an in-test TLS server for `api.example.test`/`evil.example.test` behind the
`Egress::for_tests` resolver and roots seam).

### Mail capability details (as built, M2a-2a P17, P18)

`mail.notify-admins(subject, text-body)` (linked only for `mail = "admins"`) checks, in
order, inside the call (`HostState`): refused during a render filter (R20: no side
effects on the render path); at most one mail per call; a subject of 1 to 200 characters
(`MAX_MAIL_SUBJECT_CHARS`) without control characters or Unicode line separators; a body
of at most 64 KiB (`MAX_MAIL_BODY_BYTES`). Then `StoreHostServices::notify_admins`: without
`[mail]` it answers `mail is not configured`; the per-plugin hourly limit, 10 mails
(`MAIL_PER_PLUGIN_HOUR`, a DB-backed window on bucket `mail:plugin:<name>`, shared by every
node; a refused second mail in one call uses no hourly slot); the recipients are the
active admins (at most 50, `MAIL_MAX_ADMINS`; unusable addresses skipped, none left is an
error), all in `Bcc`, so admins do not see each other's addresses and the plugin never
learns them. The subject gets the site-title prefix `[<site.title>] ` (01 §4) and the body
the footer `Sent by plugin <name>`. The mail is handed to the node's mail queue and the
call returns: SMTP never runs inside the plugin call (F28); a full queue (16 in flight) is
an error to the guest, an SMTP failure is only logged. Example: `plugins/examples/notifier`
(mails the admins on `post.published`); hostile: `mail-flood` (§12).

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
  backslashes, whitespace or control characters. A root-relative src, or one on the site's
  host (any scheme or port), may not point under `/_p/` or `/_media/` (bytes a plugin route or
  an upload serves), counted from the root and from `site.url`'s base path, after
  percent-decoding (repeatedly), lowercasing, collapsing `//` and resolving `.`/`..` (ruling
  R22); `/_theme/…` and external `https://` hosts stay allowed. A head script runs with the
  site's origin, which is why `render.head` is high risk (§2). `script-tag.json-ld` must
  parse as JSON; it is re-serialised compactly with every `<`, `>` and `&` written as
  `\u00XX`, so neither `</script>` nor `<!--` can occur inside the element. Anything else is
  dropped and logged.
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
  `Post.url` is absolute. `meta.get` and `meta.set` work only on items `content` could return
  (so an action cannot annotate a draft; the visibility check is a read just before the write's
  `Tx`).
- **Meta is type-scoped (P23, M2a-2a).** `meta.get` and `meta.set` also act only on items of
  the plugin's `content.read` types: the host looks the item's type up through
  `HostServices::post_kind` (any status) and refuses others (`meta.set` → `"no such post"`, the
  same answer as a missing or hidden item). A meta-only plugin must therefore declare the types
  it annotates (`meta-writer`: `content.read = ["post"]`). Cost (review m5, accepted): over
  `StoreHostServices` one `meta.get` is three store reads (`post_kind`, the visibility check, the
  meta row) for one unit of the call's `content` budget, on the render path too. A `"*"` (or `plugin.*`) meta pattern
  never matches another plugin's `plugin.<other>.*` keys; only a pattern naming that namespace
  does, and a plugin's own `plugin.<name>.*` keys need no grant.
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
- **Loading (plan ruling P3, ruling R21; grants as built in M2a-2a).** At startup and on
  every reload `serve` installs every enabled `plugins` row from its artifact in the media
  store: the bytes must match the row's hash, the `manifest` column (the
  `ferrite-plugin.toml` table as JSON) is read back with `Manifest::from_json` (validated
  by `Manifest::parse`), and its `plugin.name` must equal the row's name. The `granted`
  column is read as a `Grant` (§2): its capabilities must cover the manifest's or the
  plugin is skipped (so a bare `{}` skips every plugin that asks for any capability), and
  its approved limits are applied by `prepare_with`. A null `granted` (legacy operator
  rows) is `Grant::legacy`: the manifest's capabilities with `[limits]` capped at the host
  defaults. A row that fails any step is logged, reported in `nodes.meta`, and skipped; it
  never stops the site. Install, update and delete go through the API and CLI (§2).

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
increment (M4), and an upsert into `plugin_failures` as above.

**What counts (as built through M2a-2a).** Filter, action (`plugin.action` jobs) and route
calls count: a trap, a timeout, an invalid result or route response (a route answers
`502`). Not counted: a call skipped for want of a call permit (`Busy`, P22) or cut by the
per-render budget (R20(4)), refusals inside an import (the guest decides what to do with
the error), and a call that was dropped (cancel-safety, §6). **Admin view (M2a-2a):** `GET
/plugins` and `GET /plugins/{name}` show `enabled`, `disabled_reason` (`auto: <n> failures
in <window>` after an automatic disable) and each live node's state (`loaded`, `failed`
with its error, or `disabled`); `POST /plugins/{name}/enable` (or `ferrite plugin enable`)
re-enables and clears the counters.

**Per-plugin metrics (as built, M2a-2b P20).** `GET /plugins/{name}/metrics` (admins:
`manage_plugins` + `extensions:read`; unknown plugin `404`) answers two parts:

- `failures`: the plugin's shared `plugin_failures` buckets of the last 24 hours
  (`window_secs = 86400`), summed per hook and hour (hours aligned to the Unix epoch,
  `PluginFailureRepo::buckets`, newest hour first, at most 24 × 64 rows). Every node's
  flushed failures are in it; the newest few seconds may not be flushed yet.
- `node`: the answering node's call statistics since its host was built
  (`PluginHost::call_stats`), per hook: `calls` that ran (successes, failures and budget
  cuts), `failures` (as counted above), `busy` (no call slot), `budget_cut` (a call the
  render budget or a slot wait cut short, or one skipped because the page's budget was
  spent; R24 N2 makes a wait-shortened cut a failure from Task 12 on), `total_micros`,
  `max_micros` and `buckets` (calls that ran by duration: ≤ 1, ≤ 5, ≤ 25, ≤ 100,
  ≤ 500 ms, longer). The counters live in one mutex-guarded map per host (plugin →
  hook → counters; a call allocates only for a plugin or hook not seen before), at
  most 64 hooks per plugin (route hooks are named by the manifest); each node counts
  its own, so a load balancer answers with whichever node it picked. A plugin's
  counters survive an update or a re-enable (the same name stays loaded) and are
  dropped when a commit removes it from the host (uninstall, or a reload without its
  row), so a later install of the same name starts at zero; a call still running on
  the old instance at that moment can add one last entry. `PluginHost::totals` (calls
  that ran, failures; every plugin) feeds `GET /system/metrics/summary`.

Plugin names and hook names in the answer come from validated manifests and the host;
error texts are not part of it. A metrics registry, `/metrics` and time series are M4.

## 9. Host-side API (Rust)

**M0 API** (`crates/ferrite-plugin-host`): `PluginHost::new(HostConfig, Arc<dyn HostServices>)`,
`install(Manifest, &[u8])`, `apply_filter(&str, FilterValue, &RequestInfo) -> FilterValue`
(never fails), `dispatch_action`, `subscribers` (M2a-2a), `handle_route`, `is_enabled`,
`disabled_reason`, `failures`; `inspect` is a free function. Hooks are `&str`, not a `HookName` type.

M2a-2a adds `prepare_with(manifest, wasm, approved: ApprovedLimits)` (limits above the
defaults as approved, §3), `Grant` (`of`, `legacy`, `to_json`, `from_json`),
`Manifest::{permission_report, from_json}` and `PermissionReport`, `config()`,
`apply_filter_until(hook, value, req, until: Instant) -> FilterOutcome` (the render
budget; `apply_filter_traced` is the same without one), `PluginError::Busy { plugin }` from
`dispatch_action` / `handle_route` (no call slot in time; not a failure),
`HostConfig.max_concurrent_calls`, and `HostServices::post_kind(id) -> Option<String>` (the
item's type whatever its status, for the `meta` type check). `HostServices` futures are dropped
at the hard deadline: see Cancel-safety in §6.

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
- `ferrite plugin check --data DIR --dir PATH` (M2a-2a) runs the install-time validation
  and prints the permission report against that site's `[plugins]` limits, storing
  nothing. `ferrite plugin new <name>` (scaffold a plugin crate + manifest) is later.

## 11. Versioning (P9)

- WIT package versions follow semver. Additive changes (new hook names, new optional
  imports) → minor bump (`0.2.0`); since WIT records aren't extensible, adding a field
  to a record is a **breaking** change → new major world.
- The host embeds bindings for every supported world version (`bindgen!` per version
  module) and adapts domain types into each. Support window: current + previous major.

## 12. Hostile test plugins (required by CLAUDE.md)

In `plugins/examples/hostile/`, each with an integration test asserting the page still
renders and the failure is recorded (M0, plus the M2a-2a rows at the end):

| Plugin | Behaviour | Expected |
|---|---|---|
| `loop` | never returns | fuel exhaustion under default limits; page renders |
| `spin-host-fuel` | never returns; test raises fuel | only the epoch deadline stops it |
| `oom` | allocates 1 MiB blocks until the limit | trap; page renders |
| `panic` | `panic!()` on every call | trap; page renders |
| `undeclared-import` | calls `http` without declaring it | install rejected; forced link fails to instantiate |
| `sockets` | opens a TCP socket (`wasi:sockets`) | install rejected |
| `big-output` | returns 3 MiB of HTML (fits the 32 MiB memory cap) | rejected by the 2 MiB output cap (`InvalidOutput`) |
| `slow-import` | spends its call in a slow host import (`render.head`; since M2a-2a also a `post.saved` action reading the subject) | hard timeout; page renders. M2a-2a: installed as `slow-a`/`slow-b`/`slow-c` behind gated services it fills the call cap (later calls skipped as `Busy`, not failures) and shares one render budget (`plugin-host` `tests/limits.rs`); dropped mid-import it frees its permit and instance |
| `wrong-variant` | returns `text` for `render.head` | ignored, recorded |
| `fs-escape` | reads `/etc/passwd` | no preopens → error inside guest; nothing leaks |
| `flapper` | fails every call | auto-disabled after threshold |
| `noisy` | floods stdout (10 MiB) / stderr (1 MiB) | capture capped at 64 KiB per stream; call succeeds |
| `fat-init` | initial linear memory 40 MiB (above the 32 MiB cap) | instantiation fails; treated as a call failure |
| `kv-budget` | `kv.set` 150 times | 101st+ operation refused (budget 100) |
| `long-error` | fails with a 1 MiB `plugin-error` full of newlines and ANSI escapes | logged/recorded text cut to 1 KiB and escaped; page renders |
| `resource-flood` | leaks 5 000 `wasi:cli/stdout` handles | resource cap (1 000) fails the call; host keeps serving |
| `route-xss` | `GET /x`: HTML with `<script>` reading `document.cookie`, plus `Set-Cookie`, `Authorization`, CSP, HSTS, CORS and caching headers; echoes the request header names it got. `GET /panic` panics, `GET /loop` never returns; `/status` (600), `/crlf` (CRLF in a header value), `/headers` (40 headers) | served only inside `sandbox allow-forms; form-action 'self'` with `nosniff`/`no-store`, every other header dropped, no cookie/`Authorization` seen; panic, loop and the invalid responses are `502` and the site keeps serving (`ferrite-http` `tests/app/plugin_route.rs`, `plugin-host` `tests/hostile.rs`) |
| `route-flood` | `POST /submit` (`rate_limit = "5/min"`, `cross_origin = true`) flooded from one client across two nodes | 6th POST is `429` on either node, another client still passes (`crates/ferrite/tests/cluster/routes.rs`) |
| `mail-flood` | `post.saved`: 50 `notify-admins` in one call; `post.published`: a subject with CR/LF and `Bcc:` header injection, and one over 200 characters; `render.head`: a send from a render filter | one mail per call, at most 10 per plugin per hour across nodes, injection and oversize refused, the render-path send refused; admins only, in `Bcc`; refusals reach the guest as errors, never as failures (`ferrite-http` `tests/plugins/mail.rs`) |
| `ssrf` | `http = ["api.example.test"]`; `GET /get?u=<url>` fetches any URL; `render.head` fetches `https://169.254.169.254/latest/meta-data/` on every page | the metadata URL is refused by the host check (not listed), the page renders, the head is volatile and nothing counts as a failure; with the listed host resolving to loopback, private, link-local, CGNAT, IPv4-mapped, NAT64, 6to4 or IPv6 internal addresses, or to an answer mixing a public and an internal one, the fetch is "address not allowed" and no connection is made (`ferrite-http` `tests/plugins/egress.rs`) |

Test plugins added in M2a-2a (not hostile, `plugins/examples/`): `router` (routes `/hello`,
`/echo`, `/go`, `/away`, `/big`, `/whoami`), `notifier` (`mail` on `post.published`),
`counter` (`request.served` subscriber counting events in KV), `settings-probe` (reports
`settings.get` and `options.get` answers, secrets included), and `hello-v2` (a manifest-only
`0.2.0` of `hello` asking for `kv`, used with `hello`'s component for widening updates).
`meta-writer` now declares `content.read = ["post"]` (P23).
M2a-2b adds `fetcher` (`http = ["api.example.test"]`, route `GET /get?u=<url>` answering
`ok <status> <body>` or `err <message>`; optional `m`, `h`, `b` and `n` parameters set the
method, headers, a body and several fetches per call).
