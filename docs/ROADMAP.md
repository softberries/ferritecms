# FerriteCMS — Roadmap

Detailed design: [`docs/design/`](design/00-overview.md). Each milestone ends with
something runnable and demoable. Work top-down.

Durations are rough estimates (not measurements) for one focused developer. Adding
Postgres, S3 and working cluster mode to the first release (decisions D1, D4, D5, D20)
moves the credible MVP from the original ~3 months to roughly **5 months**.

## M0 — Plugin host spike (de-risk first, ~2 weeks)

The riskiest part. Prove it before building a CMS around it. Design: [05](design/05-plugin-host.md).

- [x] Cargo workspace skeleton (`ferrite-plugin-host`, `ferrite-sdk`, `wit/`), CI with fmt + clippy. (`ferrite-domain` was not needed by the spike; it moves to M1.)
- [x] WIT `ferrite:plugin@0.1.0` draft: `guest` (apply-filter, on-action, handle-request), `log`, `content` (stub), `kv` (stub).
- [x] Verify toolchain: `wasm32-wasip2` component output, unused imports dropped, WASI subset linking.
- [x] Install-time inspection: reject undeclared `ferrite:*` imports and `wasi:sockets`/`wasi:http`.
- [x] Per-plugin `Linker` with only granted interfaces.
- [x] Limits: `StoreLimits` memory, fuel, epoch deadline + tokio hard timeout.
- [x] Hostile plugins: loop, oom, panic, undeclared-import, sockets, big-output, wrong-variant, fs-escape (plus spin-host-fuel, slow-import, flapper, noisy, fat-init, kv-budget, long-error, resource-flood; 05 §12).
- [x] Hello-world `render.head` plugin via `ferrite-sdk` macros.
- [x] Benchmark: fresh instance per call (pooling allocator) vs. reused instance. Result: ~27 µs fresh, ~24 µs pooling; reuse not needed (05 §6).

**Exit (met):** filter warm call ~27 µs (< 1 ms); all hostile plugins contained; findings written back into design 05.

## M1 — Storage, media, core & serving on both backends (~4 weeks)

Design: [02](design/02-storage.md), [03](design/03-media.md), [06](design/06-themes-rendering.md).
Controller rulings: [m1a](superpowers/rulings/m1a.md), [m1b](superpowers/rulings/m1b.md),
[m1c1](superpowers/rulings/m1c1.md), [m1c2](superpowers/rulings/m1c2.md).

- [x] `ferrite-domain` crate (pure types: ids, Post, Term, User, Role, Block, errors; 01 §1), deferred from M0.
- [x] `ferrite-config` (toml + env, validation; M1c-1). Profiles are not implemented as a separate concept: `cluster.mode = auto|solo|cluster` derives solo vs cluster (design 01 §4).
- [x] `ferrite-store` traits, `ferrite-store-sqlite`, `ferrite-store-postgres`; migrations `0001_init` and `0002_search` on both (M1a), `0003_media_variants` (M1c-2, ruling R29).
- [x] Migration wrapper (M1a store side, M1c-1 CLI):
  - [x] Directives (phase / min-binary / lossy-down), compatibility verdict, `min_binary_version` preservation, SQLite pre-migration snapshot.
  - [x] `ferrite migrate status|up|down|verify` CLI (plus `ferrite init`; design 09 §8). The startup check (`startup_check`) runs in `serve` and `seed` (M1c-2).
- [x] Contract test suite (`ferrite-store-tests`) + migration roundtrip tests on both backends; CI with Postgres service.
- [ ] Store follow-ups from the M1a review (deferred minors):
  - [ ] Full Unicode case folding for search (`İstanbul` currently indexes as `i̇stanbul`, so `istanbul` misses it; same on both backends).
  - [ ] Importer (M3) needs a fallback author: `content_items.author_id` is non-optional.
  - [x] Core/HTTP input validation should reject NUL in text before it reaches the store (reads with NUL keys return `Invalid`). The public router rejects control characters in paths, `%00` included; the API refuses a NUL in any string of a JSON body with `422 validation` before the handler runs (M2a-1). The store keeps its own check.
  - [ ] Revisit locale-aware ordering for admin lists (byte-wise `COLLATE "C"` today).
  - [x] Non-ASCII e-mail case variants are distinct accounts (ASCII-only `email_key`): decided and kept (M2a P14; consistent with RFC 5321's case-sensitive local parts).
- [x] `ferrite-media`: `MediaStore` over `object_store` (local + S3), content sniffing, `process_image` (M1b). Tests run against local and an in-process `s3s-fs` S3 server (no MinIO). The `artifacts` table itself is in the store crates.
  - [x] `media.variants` job calling `process_image` + writing `artifacts` rows and the `media.variants` mapping, with an image-processing concurrency semaphore (M1b ruling R4). Moved to **M2** (media service and uploads; needs the M1.5 job queue). Done in M2a-2b (`MediaVariantsHandler`, `media.image_workers`; design 03 §4).
  - [x] `/_media/<hash>/<name>` serving via `MediaStore::open_read` (streaming; ETag, 304, single ranges, 416, HEAD; `inline_safe` types inline, PDF with `Content-Security-Policy: sandbox`, everything else attachment + `nosniff`; published media only; design 03 §5). Upload limits for `put_stream` apply when uploads exist (M2).
  - [x] Install a process-default rustls `CryptoProvider` in `main` (both `ring` and `aws-lc-rs` are linked, so rustls panics building a client config without a default). Done in `crates/ferrite/src/main.rs` (`ring`).
- [ ] Core services (`ferrite-core`):
  - [x] Content: create/update with slug allocation and retry, revisions (keep 50), search indexing, status lifecycle (publish, unpublish, schedule, submit, trash, restore, delete, `publish_due`), permalink-change redirects (including descendants, rulings R1/R2), optimistic `version`.
  - [x] Roles and capabilities (`Actor`, `can`; design 07 §1), including the contributor rules (R8).
  - [x] Terms (`TaxonomyService`), options (`OptionService`), redirects (`RedirectService`, loop check), menus (`MenuService`).
  - [x] Blocks to HTML rendering with sanitising (`blocks`), slug generation (`slug`).
  - [x] Permalinks (`PermalinkRules`: `%slug%`, `%year%`, `%month%`, `%category%`, `%parent_path%`) and the published-only public read facade `SiteReader` (single, front page, archives, term/author/date archives, search, feeds, sitemaps, related, media files, preview) (M1c-2).
  - [x] Content types service (`ContentTypeService`: names checked against the router and permalinks, field schemas validated on every content write, `types.changed`; M2a-1, P18).
  - [x] Search query service for public search (`SiteReader::search`, published-only, capped at 200 matches, ruling R9). Admin search: `GET /api/v1/content/{type}?q=` (M2a-1).
  - [x] Users service (`UserService` with the operator path for the CLI; M2a-1, P14–P17).
  - [ ] Media service (`media` rows, `media.variants`; M2a-2b). `SiteReader` reads media rows (`media_file`, featured and inline images).
- [x] Transactional outbox (`events`) written by every invalidating write:
  - [x] Written in the same transaction by content, lifecycle, term, menu and options writes in `ferrite-core` (`events.rs`).
  - [x] Redirect writes emit informational `redirect:<source>` events (M1c-2 Task 3); the dependency-key contract is documented in `events.rs`.
  - [x] `theme.changed`, `plugins.changed` and `plugin.disabled` are emitted by `ExtensionService` and the shared failure counters (M1.5; design 04 §4 "Event kinds").
  - [x] `user:<id>` is emitted by `UserService` on display-name and status changes and deletes (M2a-1).
  - Not yet emitted because no write path exists: `post:<media id>` for media-row writes outside `seed` (M2a-2b media service).
- [x] `ferrite serve`: router (pure, canonical paths, permalink matching), redirects before the cache, 404/410 shared pages, RSS feeds, sitemaps, `robots.txt`, `/healthz`, `/readyz`, bounded shutdown (M1c-2; design 06 §3, 09 §8).
- [x] `ferrite seed demo` (posts, pages, terms, menus, sample image with variants; M1c-2).
- [x] Theme engine: MiniJinja (fuel, output cap, no loader), hierarchy resolution (incl. front page, m1c2 R17), safe helpers, dependency recording; `blog` theme (M1c-2; design 06 §1–§4).
- [x] Page cache with dependency index and the outbox polling loop, gap handling with late-commit probe (R24) (M1c-2; design 06 §6, 04 §4).
- [x] Plugin host wired into rendering (`render.head`, `content.body`, `content.excerpt`; traced dependency keys and volatile marking), plugins loaded from the `plugins` table + artifacts at startup (M1c-2; design 05 §7a).
- [ ] Plugin host follow-ups from M0:
  - [x] `ammonia` sanitising of `html` output (R22) and head→HTML rendering with escaping and `script-tag` checks (05 §7).
  - [x] `content.read` published-only, preview visibility scoped to the previewed item (05 §7a); `options.get` never returns `plugin.*` keys.
  - [x] Reject bare IPs and `localhost` in the http allowlist (manifest rule, M2a-2a P7, F5) and implement the real `mail` service (`notify-admins`, M2a-2a P17, P18).
  - [ ] The real `http` import with the resolved-address SSRF / DNS-rebinding check (M2a-2b).
  - [x] Type-scope `meta` and stop `"*"` meta grants exposing other plugins' `plugin.<other>.*` meta (M2a-2a P23).
  - [x] Admin approval flow for limits above host defaults, up to ceilings (M2a-2a P5; M2a P28).
  - [ ] Revisit the pooling allocator under concurrent load (M4).
- [x] More plugin host follow-ups from the M0 review (all M2a-2a):
  - [x] `HostConfig` max-concurrent-calls semaphore covering filters, actions and routes (`plugins.max_concurrent_calls`, P22); worst-case plugin memory = concurrency × per-call memory cap.
  - [x] Per-render total plugin time budget (`plugins.render_budget_ms`, P22, F37).
  - [x] Size caps on route responses (status, ≤ 32 headers of ≤ 4 KiB, body ≤ 1 MiB; 05 §4b).
  - [x] Success-path tests for `dispatch_action` / `handle_route` (`plugin-host` `tests/hello.rs`).
  - [x] Document `HostServices` cancel-safety (05 §6), with drop tests.
  - [x] Compiles off the runtime: the install pipeline and the reloader prepare components on the blocking pool (`Installer`, at most two compiles per node).

- [ ] M1c-2 notes carried over from the M1c-1 reviews (deferred minors):
  - [x] The router checks the redirect table before the page cache (`crates/ferrite-http/src/app/handler.rs`); redirect writes also emit informational events.
  - [ ] Test harness: a Postgres test that ends right after dropping an uncommitted `Tx` stalls about 5 s at teardown (10-testing).
  - [ ] `ferrite init` validates the written config with `FERRITE_*` env overrides applied, so the validated config can differ from the file (09 §8).
  - [x] When a published parent's slug changes, its children's permalinks change too: 301 redirects from the old descendant paths and their cache keys are emitted (M1c-2 Task 1, rulings R1/R2; capped at 1000 walked descendants).
  - [x] Serve PDFs inline with a CSP `sandbox` header (`crates/ferrite-http/src/app/media.rs`).
  - [x] SVG sniff assertion test (`crates/ferrite-media/src/sniff.rs`, `inline_safe` false for sniffed SVG; plus an HTTP test that an SVG upload is served as an attachment).
  - [x] `ferrite migrate status` opens SQLite with the exclusive data-folder lock, so it fails while a server holds it: documented in the serve docs (09 §8); no read-only open.
  - [x] "Publish now" on a scheduled item: `publish` accepts scheduled items (clears `scheduled_at`; the queued job then does nothing; `POST /api/v1/content/{type}/{id}/publish`, M2a-1).
  - [ ] Postgres `status`, `down` and `verify` may create an empty configured schema (`PgStore::open`); add a `PgOptions` flag so read-only commands do not create it.

**Exit:** a seeded site renders identically on SQLite+local and Postgres+MinIO; editing via CLI/seed invalidates the right pages.

**Exit verified (2026-10-23):** [`docs/superpowers/m1-exit.md`](superpowers/m1-exit.md). The
demo site rendered identically (byte-identical, or differing only in the seed's clock times)
on SQLite + local media, Postgres + local media and Postgres + S3 (an `s3s-fs` S3 server
standing in for MinIO, which is not available on the verification machine); `seed demo
--force` against a running Postgres-backed server showed up on the cached home page through
the event outbox. On SQLite the folder lock prevents a second process from writing while
`serve` runs, so edit-invalidation there is verified by the `freshness::*::solo` HTTP tests.

## M1.5 — Cluster mode (~3 weeks)

Design: [04](design/04-cluster.md) (as built). Rulings: [`docs/superpowers/rulings/m15.md`](superpowers/rulings/m15.md).

- [x] Node registry + heartbeat, startup validation (mode/backends/site_id/site.url/solo single-live-node/clock skew) (`crates/ferrite-cluster/src/node.rs`; 04 §3, P14, P15, R7, R8).
- [x] Event bus: `LISTEN/NOTIFY` (notify from the inserting statement, per-schema channels) + catch-up polling + gap handling; cache flush and node reload on lost history; events pruned after 24 h (04 §4; P1–P4, R9).
- [x] Job queue: claim with `SKIP LOCKED`, lease token, renewals and local fencing, retries with backoff, dead jobs, periodic scheduler via unique keys, shutdown hand-back (`crates/ferrite-cluster/src/jobs/`; 04 §6; P5–P11, R1, R4–R6).
- [x] Distributed locks (advisory on Postgres, in-process on SQLite), used by migrate (`migrate`), theme/plugin install and activation (`extensions`) and node join (`cluster.join`); `import` reserved for M3 (04 §7; P13).
- [x] Artifact sync for plugins and themes across nodes (themes as deterministic tar artifacts, P17); hot swap of theme, plugins and permalink rules with keep-previous-on-failure and retry; per-node state in `nodes.meta` (04 §8, 05 §2, 06 §1; P18, P19, P22, R13, R14).
- [x] Shared plugin failure counters + cluster-wide auto-disable (05 §8; P20, R12).
- [x] Scheduled publishing as jobs (`content.publish_scheduled` per item, `content.publish_due` safety net every 30 s; P12).
- [x] Cluster integration tests (2–3 in-process nodes on one Postgres schema, plus two `ferrite serve` processes): invalidation latency, job exclusivity, lease expiry after a crash, periodic once-per-period, rolling version coexistence, concurrent first start, reload on every node, cluster-wide auto-disable (`crates/ferrite/tests/cluster/`, `crates/ferrite/tests/serve/site.rs`).
- [x] `docker-compose` example: 2× ferrite + postgres + minio + caddy (`examples/cluster/`, `Dockerfile`; 09 §7). Verified by `ferrite config check` in `crates/ferrite/tests/examples.rs` and by `docker compose config`, `caddy validate` (the Caddyfile) and `docker build` in CI; `docker compose up` is not run (P24).
- [x] `ferrite config check` (09 §8).
- [x] Deferred from M1c-2:
  - [x] Event bus remainder: `LISTEN/NOTIFY`, the lost-history flush on reconnect, and pruning events older than 24 h.
  - [x] Postgres `idle_in_transaction_session_timeout` = 60 s, shorter than the 10-minute late horizon (P4).
  - [x] Invalidation loop: the late-probe resume path is tested (`late_probe_resumes_where_it_stopped`; bound documented in 04 §4).
  - [x] Themes as artifacts and hot swap on `theme.changed`; plugin hot reload on `plugins.changed`; route table rebuilt on permalink option changes.
  - [x] Template memory limits (M1c-2 R14) revisited: decided by P23, deferred to M4 (render worker).

**Exit:** two nodes behind a proxy; a save on node A is reflected on node B within ~1 s; killing a node mid-job doesn't lose the job.

**Exit verified (2026-10-23), with limits:** [`docs/superpowers/m15-exit.md`](superpowers/m15-exit.md).
Automated: in-process multi-node tests and a two-process `ferrite serve` test on one
Postgres schema (invalidation p50 8–14 ms with `LISTEN` over two runs, max 258 ms in the
poll-only phase, under the 1 s bound everywhere; a crashed node's job finished by another
node 2.52 s after the crash with a 2 s test lease). Manual: two `ferrite serve` processes on local Postgres served an edit within one
request round trip. Not verified: a reverse proxy in front (the Caddyfile is checked by
`caddy validate` in CI only; not validated locally, Docker is unavailable here), `docker compose up`, and an OS-level kill of a process mid-job (the crash is
simulated in process, P25).

## M2 — API, auth & admin (~6 weeks)

Design: [07](design/07-admin-api.md). Split three ways (M2a P1), each with its own plan:
**M2a-1** (REST API core and auth), **M2a-2** (extensions, plugin API extras, media,
comments, system endpoints, hardening; itself split into **M2a-2a** and **M2a-2b**, M2a-2
P1), **M2b** (admin SPA). Rulings:
[`docs/superpowers/rulings/m2a.md`](superpowers/rulings/m2a.md) (M2a-1),
[`docs/superpowers/rulings/m2a2.md`](superpowers/rulings/m2a2.md) (M2a-2a).

### M2a-1 — REST API core & auth (done)

Plan: [`2026-10-23-m2a-api.md`](superpowers/plans/2026-10-23-m2a-api.md).

- [x] REST API v1 under `/api/v1` with OpenAPI 3.1 from code (`utoipa`, snapshot `crates/ferrite-http/openapi.json` checked by a test), RFC 9457 problems, cursors, `If-Match` on content, `Cache-Control: no-store` (07 §1; P10–P12, P24).
- [x] Auth: argon2id, DB sessions, CSRF + origin checks (login included), scoped API tokens, DB-backed rate limits across nodes, TOTP, the site key outside the database (07 §2; P4–P9). Passkeys deferred past M2 (P3).
- [x] Roles & capabilities enforced in core, narrowed by token scopes (`Actor` scopes, `require`/`require_read`, `can_read`; P6, R17, R23).
- [x] Users service + API + `ferrite user create|reset-password|list|disable-totp` (P14–P17); login validation (`^[a-z0-9][a-z0-9._-]{0,59}$`, immutable) and `user:<id>` on display-name/status changes and deletes.
- [x] Content, lifecycle and revisions endpoints, "publish now" from scheduled (P13).
- [x] Content types (field schemas) and taxonomies/terms endpoints, registered through core with `types.changed` (P18).
- [x] Menus, redirects (CSV import/export), settings (allowlist) endpoints; `permalink.*` validated on every write against types and taxonomies (P19, R19).
- [x] Previews over HTTP (`/_preview/<id>`) and logged-in requests on the public site (`cache.logged_in = "bypass" | "per-user"`) (P20, P21).
- [x] `sessions.prune` periodic job (sessions, expired tokens, old rate-limit windows; P23).
- [x] Audit rows for user, token, TOTP, type/taxonomy and settings writes (P22).

**Exit:** the API core works against a real `ferrite serve` on both backends: login, create,
publish, preview, logout; auth and revocation hold across nodes.

**Exit verified (2026-10-23):** [`docs/superpowers/m2a1-exit.md`](superpowers/m2a1-exit.md):
the workspace tests on both backends, `api_end_to_end_on_real_binary` (solo and cluster), the
OpenAPI snapshot test and a manual `curl` session against `ferrite serve` (solo, SQLite).

Known limits (accepted in M2a-1, see the rulings): the last-admin check is not serialized
under READ COMMITTED (P16); a login and its e-mail are separate
rate-limit budgets (10 guesses/min per account); fixed windows admit up to 2× a limit
across a window edge (P5); on SQLite a backwards clock jump keeps a rate-limit bucket in its
future window until time catches up; users, terms, menus, redirects and settings are
last-write-wins (P11); no recovery codes for TOTP (P9).

### M2a-2a — extensions and the plugin platform (done)

Plan: [`2026-10-24-m2a2-extensions.md`](superpowers/plans/2026-10-24-m2a2-extensions.md).
Rulings: [`m2a2.md`](superpowers/rulings/m2a2.md).

- [x] Admin API calls the extension writes only through `ExtensionAdmin`, which checks `manage_themes` / `manage_plugins` with the `extensions` scope in core and audits every write (closes M1.5 ruling R11; P8, P10). Plugin delete removes the row and keeps its KV and settings 30 days unless `purge` (`plugin.purge` job, daily `plugin.purge_sweep`; 05 §2; P9, R2).
- [x] Plugin install flow with the permission report and an approval fingerprint of the exact upload; updates that widen capabilities need the new approval in the same request, else nothing is stored and the old version keeps running (P3, P4, R1); limits above host defaults approved per plugin up to ceilings (P5). The install flow always sets `plugins.granted` to an object (closes the M1c-2 R21 item); a legacy null grant has its limits capped at the defaults (R7).
- [x] Themes API (`GET/POST /themes`, `POST /themes/{name}/activate`, settings) + `ferrite theme install|activate|list`; activation refuses a theme that does not load (P12, R10); theme settings validated by `theme.toml [settings]` (P13); a node not serving the active row's artifact is flagged `nodes.meta.theme.fallback`, shown by `GET /themes` (F12).
- [x] `ferrite plugin check|install|list|enable|disable|remove` over the same core paths (P26; `check` instead of a separate `inspect`).
- [x] Plugin routes (`/_p/<plugin>/<path>`, 05 §4b): only manifest paths and methods, body ≤ 1 MiB, response caps, DB-backed per-route rate limits on the client address (across nodes), an `Origin` check unless `cross_origin`, `Content-Security-Policy: sandbox allow-forms; form-action 'self'` (R14), `nosniff`, only allowlisted headers back, only the `user` id passed; hostile `route-xss` and `route-flood` (two nodes) (P15, P16).
- [x] Plugin settings with secrets sealed per plugin and key under the site key, write-only in the API, opened only by the owning plugin (P13, R11).
- [x] `[mail]` (SMTP over lettre, TLS rules R16), the node's mail queue, `mail.notify-admins` (admins only, 1 per call, 10 per plugin per hour; hostile `mail-flood`) and password reset by e-mail with stateless single-use tokens (P17–P19, R15–R17).
- [x] `plugin.action` jobs for `post.saved`, `post.published` and `user.created`, enqueued in the write `Tx`; one registry snapshot per call (M1.5 R13); success-path tests (P20, P29, R18).
- [x] Batched `request.served` (D21, P21): per-node buffer flushed as `plugin.action` jobs.
- [x] Plugin-host follow-ups: bare IPs, meta scoping, limits approval, the call cap and the render budget (P22, R20), cancel-safety docs, compiles off the runtime (see M1 "Plugin host follow-ups").

**Exit:** extensions work against a real `ferrite serve`: inspect → install with the
fingerprint → enable → a plugin route → settings with a secret → theme upload and
activation → password reset through an SMTP server; workspace tests on both backends.

**Exit verified (2026-10-24):** [`docs/superpowers/m2a2a-exit.md`](superpowers/m2a2a-exit.md).

Known limits (accepted in M2a-2a, see the rulings): component compile time is unbounded
(only two compiles per node run at once; 05 §6); no fairness inside the render budget (a
slow early plugin can starve later ones; 05 §6); delivery of actions is at least once
(plugins must be idempotent); `request.served` events are lost on a crash (up to 10 s);
the reset link points at the M2b admin page, which does not exist yet (F33); a concurrent
install of the same bytes can lose its blob (reinstall; M4 GC grace period); the route CSP's
`form-action 'self'` was checked in Chrome only (R14).

### M2a-2b — media, comments, system, hardening (next)

Outline in the M2a-2 plan ("Next plan: M2a-2b"); it gets its own plan.

- [ ] Media service and uploads: `POST /media`, `media` row writes emit `post:<media id>` in the same transaction (M1c-2 R11); the `media.variants` job and its concurrency semaphore (moved from M1); upload limits on `put_stream`; `Idempotency-Key`; SVG sanitising on upload (SVG is not processed by `ferrite-media`; design 03 §5 requires sanitisation before inline serving). Migration `0006` (idempotency keys, the sanitised-SVG marker).
- [ ] Comments: moderation endpoints, public `POST` (rate-limited, honeypot), comment settings.
- [ ] System API: status (incl. the theme fallback flag), nodes, jobs (queued kinds no node knows stay queued, M1.5 P5; dead jobs with `last_error`, retry), per-plugin metrics (`GET /plugins/{name}/metrics`), metrics summary, audit log listing.
- [x] Node-reported error text (`nodes.meta` plugin and theme `error`, shown to admins by `GET /plugins` and `GET /themes`, and next by the system API) is passed through verbatim, cut to 1 KiB; it can carry storage-backend error text (local paths, S3 endpoints). Sanitise it or document it as operator diagnostics with the system API (M2a-2a Task 4 review, minor 4). Done in M2a-2b Task 9: scrubbed at the source and again on read (P18, 04 §3, 07 §1).
- [ ] Hardening: sanitised `html` block nodes keep ammonia's default URL schemes, so `//evil` (scheme-relative), `ftp:` and `tel:` links pass (`ferrite-core/src/blocks`); decide an allowlist. Real `http` import (HTTPS only, allowlisted hosts, resolved-address SSRF checks, redirects to allowlisted hosts).
- [ ] Auth follow-ups from M2a-1:
  - [ ] `__Host-` prefix for the session cookie on `https` sites (defence against cookie tossing from a sibling subdomain; the cookie is `ferrite_session` today).
  - [ ] Zeroize key material and passwords in memory (`SiteKey` is `Clone`; subkeys, HMAC blocks and password `String` copies stay in memory after use); parked from the M2a-1 Task 3 and Task 4 reviews (logs and `Debug` already redact them).
  - [ ] Term archives for custom types: `SiteReader::term_archive_page` lists only `post` items, so a taxonomy used only on custom types has 404 term archives (pre-existing since M1c-2; M2a-1 ruling R18).
- [ ] Deferred from M1c-2:
  - [ ] Term rename and `permalink.*` option changes record no redirects (only content moves do).
  - [ ] `ferrite seed demo` atomicity (each step is its own transaction; a failed run leaves a partial batch).
  - [ ] Redirect hit counting spawns one task per hit, unbounded: use a bounded channel or coalesce.
  - [ ] `date` filter timezone support (`site.timezone`; today UTC).
  - [ ] Theme `query(term=…)` is capped at `reading.per_page` (it reads page 1 of the term archive): add a term + limit reader.
  - [ ] A child page with slug `page` is shadowed by pagination (`/parent/page/…`): reject it in slug validation.
  - [ ] A legacy item holding a reserved slug is renamed on update; every top-level write does an extra `types().list()`.
  - [ ] Wire the `render.footer` filter into rendering (in the hook catalogue, not called yet).
  - [ ] Make the 60 s short TTL configurable (search TTL with `0` disabling it, 06 §6; `plugins.volatile_ttl`, 05 §7a).
- [ ] Deferred from the M2a-2a final review (ruling R22):
  - [ ] `meta.get` makes up to three primary-key reads per budget unit (`post_kind`, then `is_visible` and the meta read): charge 2 units, or fold the type check into the services read (M6).
  - [ ] Test seams (`Installer::with_theme_loaded`, `Installer::with_prepare`, `PluginReloader::with_prepare`, `NodeOptions::{served_flush, listen, boot_barrier}`, `SharedFailures::with_before_disable`) are `#[doc(hidden)] pub` runtime hooks: move all of them behind one dev-only cargo feature in one chore (M11).

### M2b — admin SPA

- [ ] Admin SPA scaffold: Vite + React + TS, shadcn/ui, TanStack Router/Query, Motion, API client generated from `crates/ferrite-http/openapi.json` (CI drift check), embedded via `rust-embed` (`/admin/*` with SPA fallback, hashed assets `immutable`, a placeholder page when `admin/dist` is missing, the admin CSP; 07 §3).
- [ ] Screens: dashboard (Recharts), content list + TipTap editor + revisions + autosave + the 412 conflict UI, media library, content types/field builder, taxonomies, menus, users (roles, tokens, TOTP), comments, redirects, settings, import, system (nodes, jobs, backups).
- [ ] Themes + plugins screens incl. permission report and per-plugin metrics.
- [ ] The password-reset page (`/admin/reset-password#token=…`, M2a-2a F33): read the token from the fragment, then strip it from the URL and history (`history.replaceState`) and never put it in a URL the page builds; send `Referrer-Policy: no-referrer` (Task 10 review).
- [ ] Admin "Nodes" and "Jobs" screens reading `nodes` (incl. `nodes.meta` theme/plugin state) and `jobs` (deferred from M1.5).
- [ ] Comment pagination (M2a-2b Task 8 review): a public thread shows the oldest 500 approved comments (`MAX_THREAD`), so later ones never appear, their `303 #comment-<id>` lands on a missing anchor and `count` reads 500; page the thread (WordPress `comment-page-N`) and give the real total.
- [ ] Playwright E2E against a real binary on both backends; Vitest component tests; a CI admin job (`pnpm install --frozen-lockfile`, lint, test, build).

### Done from the earlier M2 deferred lists

- [x] User service: validate logins and emit `user:<id>` (from M1c-2; M2a-1).
- [x] Previews over HTTP; logged-in cache bypass / `cache.logged_in = "per-user"` variants (from M1c-2; M2a-1).
- [x] Validate `permalink.*` on option write and on type/taxonomy registration (from M1c-2; M2a-1, P19).
- [x] `sessions.prune` periodic job with the sessions table (from M1.5 P26; M2a-1).
- Moved to **Later**: multi-host sites (the cache-key host is pinned to `site.url`, R25), because a second host needs per-host `site.url`, cookies and CSRF origins (M2a P27).

## M3 — WordPress import (~2 weeks)

Design: [08](design/08-wp-import.md).

- [ ] Streaming WXR parser → IR (temp SQLite), resumable, idempotent.
- [ ] Media import via `MediaStore`, URL rewrite.
- [ ] Gutenberg + classic → TipTap JSON; shortcode handling.
- [ ] 301 redirects for old permalinks.
- [ ] Yoast / Rank Math → SEO fields; ACF → custom fields (verify key names on real exports).
- [ ] `import-report.html`; CLI + admin job.
- [ ] Tests against 2–3 real WP exports.
- [ ] Deferred from M1.5: the import job must override `JobHandler::timeout()` (default 300 s, P9), take the `import` lock (P13), and batch its events per transaction (R2: Postgres serialises commits of transactions that ran `NOTIFY`).
- [ ] Carried from the M1.5 final review (rulings/m15.md R17):
  - [ ] Run `EXPLAIN (ANALYZE, BUFFERS)` on the Postgres job claim (`crates/ferrite-store-postgres/src/repos/job.rs`, the `claim` query) with ~50k future-scheduled `content.publish_scheduled` rows before the import creates them in bulk. With fewer due jobs than `limit`, the `OR` branch for expired leases stops the index scan from ending at `run_at > now()`, so each idle claim may read every future row (review M5). If it does, split the claim into a `UNION ALL` of the due branch and the expired-lease branch.
  - [ ] The `import` lock must not be relied on for exclusion: a Postgres advisory lock is not a fence (the holder is not told when its connection is terminated; `LockGuard` docs, 04 §7). Fence the import's writes in the data, for example with a run token checked on every write (review M3).

## M4 — Ops & release (~2–3 weeks)

Design: [09](design/09-operations.md).

- [ ] `ferrite backup` / `restore`, `export` / `import-site` / `db copy`, `media sync` / `gc`.
- [ ] Cluster follow-up (M1.5 ruling R3): waiting for a Postgres named lock holds one connection with `lock_timeout` instead of opening a connection per 250 ms poll (P13), which removes the connect churn while waiting.
- [ ] Media follow-ups (from M1b):
  - [ ] Document the S3 lifecycle rules: abort incomplete multipart uploads, expire `tmp/` (backstop for cancelled or failed staging cleanup).
  - [ ] S3 `copy_if_not_exists` config option (production S3 currently uses the racy head + copy fallback; `ArtifactRef.created` is advisory meanwhile).
  - [ ] Decide `aws-lc-sys` vs `ring` for the static build. object_store's `aws` feature enables `aws-lc-rs` directly (which builds the C library `aws-lc-sys`); object_store also has a `ring` feature. `cargo tree` shows both `ring` and `aws-lc-rs` linked today (rustls pulls both).
  - [ ] Expose `copy_if_not_exists` and `conditional_put` in `S3Config` (today neither is set on the builder, so production S3 uses the head + copy fallback). Note the explicit-keys vs `from_env` difference: with explicit `access_key_id`/`secret_access_key` the builder reads no `AWS_*` environment (so `AWS_COPY_IF_NOT_EXISTS`, `AWS_CONDITIONAL_PUT`, session tokens and instance/web-identity credentials do not apply); only the no-keys path uses `from_env`. Also: `build_s3` replaces any client options read from the environment with its own timeouts.
  - [ ] Before `PublicUrlMode::Direct` ships: set `Content-Type` / `Cache-Control` attributes on the S3 path only (object_store's local backend rejects attributes, and the `Multipart` `copy_if_not_exists` mode does not copy attributes from the source object, so they must be set on the final object explicitly).
  - [ ] Artifacts GC (`artifacts.gc` job, `ferrite media gc`; design 03 §6).
  - [ ] Optional location-metadata stripping on upload: the stored original keeps its EXIF, GPS included, as WordPress does (M2a-2b R11); only variants are re-encoded without EXIF/XMP (03 §4).
- [ ] `self-update` (signed), `rollback`, `rollback --schema`, `--restore-snapshot`.
- [ ] ACME TLS (solo).
- [ ] `/metrics`, `node_metrics`, dashboard charts wired.
- [ ] Release builds for all targets, published Docker image (a `Dockerfile` exists since M1.5, built in CI), systemd unit, cluster runbook.
- [ ] Backward-compat CI job (from first tag).
- [ ] Example plugins: SEO, privacy-friendly analytics, contact form; starter themes `docs`, `business`.
- [ ] Docs site (built with FerriteCMS itself).
- [ ] Deferred from M1c-2:
  - [ ] Small in-memory LRU for hot small `/_media` files (03 §5).
  - [ ] Atom feeds next to RSS 2.0 (06 §3).
  - [ ] `Cache-Control: public, max-age=0, s-maxage=<cfg>` on HTML for CDNs (06 §6; HTML pages send no `Cache-Control` today).
  - [ ] Automated accessibility checks (axe) in CI for the starter themes (06 §8).
  - [ ] Signed or scoped direct media: with `[media.s3] public_base_url` (Direct mode) the published-only check and the attachment/`nosniff`/CSP headers do not apply, and unpublished media and plugin artifacts stay readable by hash (03 §5). Short-lived signed URLs, or a published-only bucket path.
- [ ] Deferred from M2a-2a:
  - [ ] Per-node option cache, reintroduced only with a measured need (M2a-2 P14; theme settings and `site.*` are read per cache miss, plugin settings per call, each one primary-key read). It needs R13's refresh on `options.changed` on every node ([rulings/m2a.md](superpowers/rulings/m2a.md)).
  - [ ] A GC grace period for staged artifacts with the artifacts GC, which closes the same-bytes concurrent-install race (`Installer::discard_staged`; Task 4 review round 1).
  - [ ] A compile-size or function-count guard (or a compile deadline) for uploaded components: inspecting a component of up to 32 MiB has no time bound today; two compile slots per node bound only the count (05 §6).
  - [ ] Fairness inside the per-render plugin budget (per-plugin shares, or a reservation for renders in the call semaphore; 05 §6 "Known limit").
  - [ ] Signed plugins / publisher identity: plugin data and sealed secrets follow the plugin name only, and the manifest's `publisher` is unverified, so a component from another publisher installed under the same name reads the earlier plugin's data (05 §2; M2a-2a final review M3, ruling R22). Verify a publisher signature on install and update, and show a publisher change in the permission report.
- [ ] Deferred from M1.5:
  - [ ] Template memory bound via a render worker process with an OS memory limit (P23, M1c-2 R14); until then an operator-installed hostile theme can OOM every node at once (hot swap).
  - [ ] On-disk artifact / precompiled-component cache keyed by hash + wasmtime version + CPU features (P19; restarts re-fetch and recompile today).
  - [ ] `node.drain` handling (the event kind exists, the loop ignores it; P26).
  - [ ] PgBouncer transaction pooling (listeners and locks need direct connections; 02 §6, P26).
  - [ ] Run the cluster example end to end (`docker compose up` behind Caddy, MinIO bucket init, Caddy health checks) in CI or a release check (P24).
  - [ ] An OS-level crash test (SIGKILL a `ferrite serve` child mid-job) and a silent network partition of the `LISTEN` connection (TCP proxy), neither exercised in M1.5 (P25; m15-exit.md).
- [ ] Performance follow-ups from the M1c-2 final review:
  - [ ] Sitemaps: the index walks every public row of every type (with ancestors, terms and permalinks) just to count files and take `max(lastmod)`, and `/sitemap-<kind>-<n>.xml` walks `n − 1` full files to find its cursor; both re-run on the first crawler request after any write to the kind. Use `sitemap_files` (a count) for the file count, a per-file aggregate for `lastmod`, and keyset seeking instead of walking (`crates/ferrite-http/src/app/feeds.rs`).
  - [ ] Every hydration renders each body twice (`MediaRefs::of` collects references with a recording render, then the real render runs), including reads that never show the body: feeds, plugin `list_posts`/`get_post`. Add a reader option that skips `html`, or collect the references in one pass (`crates/ferrite-core/src/site/hydrate.rs`).

## Later

- Multi-site and multi-host sites (cache-key host pinned to `site.url`, M1c-2 R25, M2a P27); read replicas; multi-region.
- Signed plugin registry with verified publishers.
- JS/Go/Python plugin SDKs (after checking Component Model toolchain maturity).
- Built-in forms, i18n/multilingual content, full-text search UI.
- WP database user import with legacy password-hash verification.
- Managed hosting offering.
- An atomic KV increment import (`kv.incr`) for counters such as the `counter` example's (whose read-modify-write can lose counts under at-least-once delivery): a WIT change, so it needs a new world version (M2a-2a ruling R19).
- Passkeys (WebAuthn, `webauthn-rs`): deferred past M2 (M2a P3; the newest `webauthn-rs` was a pre-release and the ceremony needs the SPA). There is no `passkeys` table yet.
