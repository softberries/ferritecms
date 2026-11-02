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
- [x] Store follow-ups from the M1a review (deferred minors; the open ones moved to [Later](#later) in M4-4, P66):
  - Moved to Later: full Unicode case folding for search (`İstanbul` currently indexes as `i̇stanbul`, so `istanbul` misses it; same on both backends).
  - [x] Importer (M3) needs a fallback author: `content_items.author_id` is non-optional. Done in M3-1: content whose author is not in the WXR goes to the importing admin (P9).
  - [x] Core/HTTP input validation should reject NUL in text before it reaches the store (reads with NUL keys return `Invalid`). The public router rejects control characters in paths, `%00` included; the API refuses a NUL in any string of a JSON body with `422 validation` before the handler runs (M2a-1). The store keeps its own check.
  - Moved to Later: revisit locale-aware ordering for admin lists (byte-wise `COLLATE "C"` today).
  - [x] Non-ASCII e-mail case variants are distinct accounts (ASCII-only `email_key`): decided and kept (M2a P14; consistent with RFC 5321's case-sensitive local parts).
- [x] `ferrite-media`: `MediaStore` over `object_store` (local + S3), content sniffing, `process_image` (M1b). Tests run against local and an in-process `s3s-fs` S3 server (no MinIO). The `artifacts` table itself is in the store crates.
  - [x] `media.variants` job calling `process_image` + writing `artifacts` rows and the `media.variants` mapping, with an image-processing concurrency semaphore (M1b ruling R4). Moved to **M2** (media service and uploads; needs the M1.5 job queue). Done in M2a-2b (`MediaVariantsHandler`, `media.image_workers`; design 03 §4).
  - [x] `/_media/<hash>/<name>` serving via `MediaStore::open_read` (streaming; ETag, 304, single ranges, 416, HEAD; `inline_safe` types inline, PDF with `Content-Security-Policy: sandbox`, everything else attachment + `nosniff`; published media only; design 03 §5). Upload limits for `put_stream` apply when uploads exist (M2).
  - [x] Install a process-default rustls `CryptoProvider` in `main` (both `ring` and `aws-lc-rs` are linked, so rustls panics building a client config without a default). Done in `crates/ferrite/src/main.rs` (`ring`).
- [x] Core services (`ferrite-core`):
  - [x] Content: create/update with slug allocation and retry, revisions (keep 50), search indexing, status lifecycle (publish, unpublish, schedule, submit, trash, restore, delete, `publish_due`), permalink-change redirects (including descendants, rulings R1/R2), optimistic `version`.
  - [x] Roles and capabilities (`Actor`, `can`; design 07 §1), including the contributor rules (R8).
  - [x] Terms (`TaxonomyService`), options (`OptionService`), redirects (`RedirectService`, loop check), menus (`MenuService`).
  - [x] Blocks to HTML rendering with sanitising (`blocks`), slug generation (`slug`).
  - [x] Permalinks (`PermalinkRules`: `%slug%`, `%year%`, `%month%`, `%category%`, `%parent_path%`) and the published-only public read facade `SiteReader` (single, front page, archives, term/author/date archives, search, feeds, sitemaps, related, media files, preview) (M1c-2).
  - [x] Content types service (`ContentTypeService`: names checked against the router and permalinks, field schemas validated on every content write, `types.changed`; M2a-1, P18).
  - [x] Search query service for public search (`SiteReader::search`, published-only, capped at 200 matches, ruling R9). Admin search: `GET /api/v1/content/{type}?q=` (M2a-1).
  - [x] Users service (`UserService` with the operator path for the CLI; M2a-1, P14–P17).
  - [x] Media service (`media` rows, `media.variants`; done in M2a-2b, `MediaService`). `SiteReader` reads media rows (`media_file`, featured and inline images).
- [x] Transactional outbox (`events`) written by every invalidating write:
  - [x] Written in the same transaction by content, lifecycle, term, menu and options writes in `ferrite-core` (`events.rs`).
  - [x] Redirect writes emit informational `redirect:<source>` events (M1c-2 Task 3); the dependency-key contract is documented in `events.rs`.
  - [x] `theme.changed`, `plugins.changed` and `plugin.disabled` are emitted by `ExtensionService` and the shared failure counters (M1.5; design 04 §4 "Event kinds").
  - [x] `user:<id>` is emitted by `UserService` on display-name and status changes and deletes (M2a-1).
  - [x] `post:<media id>` for media-row writes outside `seed`: emitted by `MediaService` in the same transaction (M2a-2b, M1c-2 R11).
- [x] `ferrite serve`: router (pure, canonical paths, permalink matching), redirects before the cache, 404/410 shared pages, RSS feeds, sitemaps, `robots.txt`, `/healthz`, `/readyz`, bounded shutdown (M1c-2; design 06 §3, 09 §8).
- [x] `ferrite seed demo` (posts, pages, terms, menus, sample image with variants; M1c-2).
- [x] Theme engine: MiniJinja (fuel, output cap, no loader), hierarchy resolution (incl. front page, m1c2 R17), safe helpers, dependency recording; `blog` theme (M1c-2; design 06 §1–§4).
- [x] Page cache with dependency index and the outbox polling loop, gap handling with late-commit probe (R24) (M1c-2; design 06 §6, 04 §4).
- [x] Plugin host wired into rendering (`render.head`, `content.body`, `content.excerpt`; traced dependency keys and volatile marking), plugins loaded from the `plugins` table + artifacts at startup (M1c-2; design 05 §7a).
- [x] Plugin host follow-ups from M0 (the open one moved to [Later](#later) in M4-4, P66):
  - [x] `ammonia` sanitising of `html` output (R22) and head→HTML rendering with escaping and `script-tag` checks (05 §7).
  - [x] `content.read` published-only, preview visibility scoped to the previewed item (05 §7a); `options.get` never returns `plugin.*` keys.
  - [x] Reject bare IPs and `localhost` in the http allowlist (manifest rule, M2a-2a P7, F5) and implement the real `mail` service (`notify-admins`, M2a-2a P17, P18).
  - [x] The real `http` import with the resolved-address SSRF / DNS-rebinding check (done in M2a-2b Task 11, P16; 05 §6).
  - [x] Type-scope `meta` and stop `"*"` meta grants exposing other plugins' `plugin.<other>.*` meta (M2a-2a P23).
  - [x] Admin approval flow for limits above host defaults, up to ceilings (M2a-2a P5; M2a P28).
  - Moved to Later: revisit the pooling allocator under concurrent load (planned for M4, not done).
- [x] More plugin host follow-ups from the M0 review (all M2a-2a):
  - [x] `HostConfig` max-concurrent-calls semaphore covering filters, actions and routes (`plugins.max_concurrent_calls`, P22); worst-case plugin memory = concurrency × per-call memory cap.
  - [x] Per-render total plugin time budget (`plugins.render_budget_ms`, P22, F37).
  - [x] Size caps on route responses (status, ≤ 32 headers of ≤ 4 KiB, body ≤ 1 MiB; 05 §4b).
  - [x] Success-path tests for `dispatch_action` / `handle_route` (`plugin-host` `tests/hello.rs`).
  - [x] Document `HostServices` cancel-safety (05 §6), with drop tests.
  - [x] Compiles off the runtime: the install pipeline and the reloader prepare components on the blocking pool (`Installer`, at most two compiles per node).

- [x] M1c-2 notes carried over from the M1c-1 reviews (deferred minors; the open ones moved to [Later](#later) in M4-4, P66):
  - [x] The router checks the redirect table before the page cache (`crates/ferrite-http/src/app/handler.rs`); redirect writes also emit informational events.
  - Moved to Later: test harness: a Postgres test that ends right after dropping an uncommitted `Tx` stalls about 5 s at teardown (10-testing).
  - Moved to Later: `ferrite init` validates the written config with `FERRITE_*` env overrides applied, so the validated config can differ from the file (09 §8).
  - [x] When a published parent's slug changes, its children's permalinks change too: 301 redirects from the old descendant paths and their cache keys are emitted (M1c-2 Task 1, rulings R1/R2; capped at 1000 walked descendants).
  - [x] Serve PDFs inline with a CSP `sandbox` header (`crates/ferrite-http/src/app/media.rs`).
  - [x] SVG sniff assertion test (`crates/ferrite-media/src/sniff.rs`, `inline_safe` false for sniffed SVG; plus an HTTP test that an SVG upload is served as an attachment).
  - [x] `ferrite migrate status` opens SQLite with the exclusive data-folder lock, so it fails while a server holds it: documented in the serve docs (09 §8); no read-only open.
  - [x] "Publish now" on a scheduled item: `publish` accepts scheduled items (clears `scheduled_at`; the queued job then does nothing; `POST /api/v1/content/{type}/{id}/publish`, M2a-1).
  - Moved to Later: Postgres `status`, `down` and `verify` may create an empty configured schema (`PgStore::open`); add a `PgOptions` flag so read-only commands do not create it.

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
P1), **M2b** (admin SPA; itself split into **M2b-1**, **M2b-2** and **M2b-3**, M2b P1). Rulings:
[`docs/superpowers/rulings/m2a.md`](superpowers/rulings/m2a.md) (M2a-1),
[`docs/superpowers/rulings/m2a2.md`](superpowers/rulings/m2a2.md) (M2a-2a),
[`docs/superpowers/rulings/m2a2b.md`](superpowers/rulings/m2a2b.md) (M2a-2b),
[`docs/superpowers/rulings/m2b.md`](superpowers/rulings/m2b.md) (M2b).

**M2a (the M2 backend) is done** (M2a-1, M2a-2a, M2a-2b). **M2b-1** (admin foundation and
writing), **M2b-2** (media and content modelling) and **M2b-3** (administration, extensions)
are done. The M2 exit is [`docs/superpowers/m2-exit.md`](superpowers/m2-exit.md); the M2b-3
exit was verified in two parts (M2b R76), the browser and Docker checks in part B. Next is
M3.

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
the reset link points at the M2b admin page (F33; built in M2b-1); a concurrent
install of the same bytes can lose its blob (reinstall; M4 GC grace period); the route CSP's
`form-action 'self'` was checked in Chrome only (R14).

### M2a-2b — media, comments, system, hardening (done)

Plan: [`2026-10-24-m2a2b-media.md`](superpowers/plans/2026-10-24-m2a2b-media.md).
Rulings: [`m2a2b.md`](superpowers/rulings/m2a2b.md).

- [x] Media service and uploads: `POST /media`, `media` row writes emit `post:<media id>` in the same transaction (M1c-2 R11); the `media.variants` job and its concurrency semaphore (moved from M1); upload limits on `put_stream`; `Idempotency-Key`; SVG sanitising on upload (SVG is not processed by `ferrite-media`; design 03 §5 requires sanitisation before inline serving). Migration `0006` (idempotency keys, the sanitised-SVG marker). Done in M2a-2b Tasks 1–6 (P2–P10): uploads typed from the first 8 KiB, documents (docx, xlsx, pptx, odt, ods, odp, zip, txt, csv) accepted and served download-only (R1, replacing P6; 03 §3), `media.max_upload` 256 MiB by default (at most 1 GiB), 8 uploads at once per node, `media.image_workers` slots taken without waiting (F17), the decoded-buffer cap (R12), `Idempotency-Key` with an ownership token (F5, R10; 07 §1).
- [x] Comments: moderation endpoints, public `POST` (rate-limited, honeypot), comment settings. Done in M2a-2b Tasks 7–8 (P11–P14): plain text on posts, on by default with `comments.moderation = "known"` (R2, F19), 3/min and 20/h per address across nodes, the theme form `POST /_comment` and `comments(post)` (06 §4).
- [x] System API: status (incl. the theme fallback flag), nodes, jobs (queued kinds no node knows stay queued, M1.5 P5; dead jobs with `last_error`, retry), per-plugin metrics (`GET /plugins/{name}/metrics`), metrics summary, audit log listing. Done in M2a-2b Tasks 9–10 (P15, P18–P20): `manage_system` + `system:*`, `nodes.meta.job_kinds`, node-local metrics (the registry and time series stay M4).
- [x] Node-reported error text (`nodes.meta` plugin and theme `error`, shown to admins by `GET /plugins` and `GET /themes`, and next by the system API) is passed through verbatim, cut to 1 KiB; it can carry storage-backend error text (local paths, S3 endpoints). Sanitise it or document it as operator diagnostics with the system API (M2a-2a Task 4 review, minor 4). Done in M2a-2b Task 9: scrubbed at the source and again on read (P18, 04 §3, 07 §1).
- [x] Hardening: sanitised `html` block nodes keep ammonia's default URL schemes, so `//evil` (scheme-relative), `ftp:` and `tel:` links pass (`ferrite-core/src/blocks`); decide an allowlist. Real `http` import (HTTPS only, allowlisted hosts, resolved-address SSRF checks, redirects to allowlisted hosts). Done in M2a-2b: the shared URL allowlist `ferrite_domain::html` for content blocks and plugin `html` in Task 12 (P21), the `http` import in Task 11 (P16).
- [x] Auth follow-ups from M2a-1:
  - [x] `__Host-` prefix for the session cookie on `https` sites (defence against cookie tossing from a sibling subdomain; the cookie is `ferrite_session` today). Done in M2a-2b Task 12 (P23; the name follows `site.url`'s scheme, no transition read).
  - [x] Zeroize key material and passwords in memory (`SiteKey` is `Clone`; subkeys, HMAC blocks and password `String` copies stay in memory after use); parked from the M2a-1 Task 3 and Task 4 reviews (logs and `Debug` already redact them). Done in M2a-2b Task 12 (P24; serde_json buffers and argon2 internals are not wiped, 07 §2).
  - [x] Term archives for custom types: `SiteReader::term_archive_page` lists only `post` items, so a taxonomy used only on custom types has 404 term archives (pre-existing since M1c-2; M2a-1 ruling R18). Done in M2a-2b Task 13 (P25: every public type of the taxonomy, `ContentQuery.kinds`).
- [x] Deferred from M1c-2 (the two still open moved on, P26/P27):
  - [x] Term rename records no redirect (only content moves do). Done in M2a-2b Task 13: a slug change of a term with a public archive records a 301. `permalink.*` option changes moved to M3 (P26).
  - Moved to M4 (P27): `ferrite seed demo` atomicity.
  - [x] Redirect hit counting spawns one task per hit, unbounded: use a bounded channel or coalesce (M2a-2b Task 14: per-node buffer of ≤ 10 000 redirects, one `add_hits` per redirect every 10 s).
  - Moved to M4 (P27): `date` filter timezone support.
  - [x] Theme `query(term=…)` is capped at `reading.per_page` (it reads page 1 of the term archive): add a term + limit reader. Done in M2a-2b Task 13 (`SiteReader::term_items`, exactly `limit` items).
  - [x] A child page with slug `page` is shadowed by pagination (`/parent/page/…`): reject it in slug validation. Done in M2a-2b Task 13.
  - [x] A legacy item holding a reserved slug is renamed on update; every top-level write does an extra `types().list()`. Done in M2a-2b Task 13 (an update keeping slug and parent never renames; one `types().list()` per write).
  - [x] Wire the `render.footer` filter into rendering (M2a-2b Task 14: once per page, `footer()` helper).
  - [x] Make the 60 s short TTL configurable (`cache.search_ttl` with `0` disabling it, 06 §6; `plugins.volatile_ttl`, 05 §7a; M2a-2b Task 14).
- [x] Deferred from the M2a-2a final review (ruling R22):
  - [x] `meta.get` makes up to three primary-key reads per budget unit (`post_kind`, then `is_visible` and the meta read): charge 2 units, or fold the type check into the services read (M6). Done in M2a-2b Task 12: 2 units.
  - [x] Test seams (`Installer::with_theme_loaded`, `Installer::with_prepare`, `PluginReloader::with_prepare`, `NodeOptions::{served_flush, listen, boot_barrier}`, `SharedFailures::with_before_disable`) are `#[doc(hidden)] pub` runtime hooks: move all of them behind one dev-only cargo feature in one chore (M11). Done in M2a-2b Tasks 15–16: the `test-seams` feature, checked by `crates/ferrite/tests/seams.rs` (10 §2).

**Exit:** media, comments and the system API work against a real `ferrite serve`: an
upload with an `Idempotency-Key` sent twice gives one item, variants appear, a hostile SVG is
served inline sanitised under the sandbox CSP, a theme-form comment is moderated over the API
and shows on the page, system status and a dead-job retry, a plugin `http` fetch of an
allowlisted host and a refused `127.0.0.1`; workspace tests on both backends.

**Exit verified (2026-10-25):** [`docs/superpowers/m2a2b-exit.md`](superpowers/m2a2b-exit.md).

Known limits (accepted in M2a-2b, see the rulings): a failed upload leaves its blob for M4's GC
(P3); an upload is public by hash before the post that uses it is published (P5); a reused
`Idempotency-Key` with another file is not detected, and a failed finish, an upload over 1 h or
a drop during the commit can give a second item (P10, R10); the stored original keeps its EXIF,
GPS included (R11); under `known` anyone who knows a guest's exact name and e-mail posts
unmoderated as them (F19); a public thread shows the oldest 500 comments (M2b); metrics are
node-local (P20); in Direct S3 mode the download-only and SVG CSP rules do not apply (F15); a
listed host with one non-public address in its DNS answer is unreachable (P16).

### M2b — admin SPA

Three sub-plans (M2b P1), each with its own plan and exit: **M2b-1** foundation and writing,
**M2b-2** media and content modelling, **M2b-3** administration, extensions and the M2 exit.
Rulings: [`m2b.md`](superpowers/rulings/m2b.md). The design language is
[11-admin-ui.md](design/11-admin-ui.md); status per screen is in 07 §3.

#### M2b-1 — foundation and writing (done)

Plan: [`2026-10-25-m2b1-foundation-and-writing.md`](superpowers/plans/2026-10-25-m2b1-foundation-and-writing.md).

- [x] Design system first (product requirement): a state-of-the-art admin, clearly better than the WordPress admin. Study the best admin UIs (Linear, Vercel, Stripe Dashboard, Ghost) and borrow their ideas. Pick deliberate typography (a quality UI font plus a mono, self-hosted), design tokens (color, spacing, radius, shadows), dark mode, a motion language, a command palette and keyboard shortcuts. Treat design quality as a review gate, with Playwright screenshots of every screen in light and dark mode. Done in M2b-1 Tasks 7, 8, 10 and 11: Inter Variable + Geist Mono Variable (P3), the ember accent on graphite tokens with a contrast test (P4), two densities, the motion language (11 §5), the living styleguide at `/admin/_styleguide` (P12), `⌘K` palette and the keyboard map (11 §6); every UI task's contact sheet checked against 11 §8.
- [x] Admin SPA scaffold: Vite + React + TS, shadcn/ui, TanStack Router/Query, Motion, API client generated from `crates/ferrite-http/openapi.json` (CI drift check), embedded via `rust-embed` (`/admin/*` with SPA fallback, hashed assets `immutable`, a placeholder page when `admin/dist` is missing, the admin CSP; 07 §3). Done in M2b-1 Tasks 1 and 4–6.
- [x] The password-reset page (`/admin/reset-password#token=…`, M2a-2a F33): read the token from the fragment, then strip it from the URL and history (`history.replaceState`) and never put it in a URL the page builds; send `Referrer-Policy: no-referrer` (Task 10 review). Done in M2b-1 Tasks 1 and 9 (`no-referrer` on every admin response, P24).
- [x] Screens, the writing part: dashboard (Recharts; M2b-1 Task 12), content list (Task 13), TipTap editor with the publishing panel, autosave, the 412 conflict UI and revisions (Tasks 14–17). API gaps closed in core: author references and allowed actions on content DTOs (P13), status counts (P14), the autosave slot `0007_autosaves` (P15).

**Exit:** a fresh `init` + `seed demo` + `serve` on a release build: sign in as each role,
write and publish a post, schedule one, resolve a conflict between two windows, restore a
revision, reset a password through a real SMTP catcher; the workspace tests on both backends
with and without `admin/dist`; the admin gates and E2E; the contact sheet against 11 §8;
bundle sizes recorded; `docker build` serves `/admin/`.

**Exit verified (2026-10-26), with gaps:** [`docs/superpowers/m2b1-exit.md`](superpowers/m2b1-exit.md)
(the manual flows were automated in Google Chrome). **Not verified:** Safari, and the
Docker image's admin stage (`docker build` serving `/admin/` has never run; the local engine
was down). Before merging, CI must pass on the branch, its `docker` job included.

Known limits (accepted in M2b-1, see the rulings): no media, users, settings or extension
screens yet (P1); no page-view chart until M4 (P16); the editor refuses (opens read-only) any
node or mark the core renderer does not know, h1 included (P18, P19); a derived excerpt goes
stale after body edits until derived again (F39); after Take theirs, new edits are guarded only
by `beforeunload` until saved (R13); an explicit dark or light choice against the OS flashes
one frame on load (11 §4); only Chromium is tested (P22).

#### M2b-2 — media and content modelling (done)

Plan: [`2026-10-26-m2b2-media-and-content-modelling.md`](superpowers/plans/2026-10-26-m2b2-media-and-content-modelling.md).

- [x] Screens: media library (grid/list, drag-and-drop upload with progress and `Idempotency-Key`, alt text, variants), media in the editor (featured image, inline `ferrite:media/<id>` images, picker), custom fields generated from the type schema, SEO fields, content types and the field builder, taxonomies and term trees, menus (drag-and-drop tree, `@dnd-kit`), the comment moderation queue, redirects (table, CSV import/export, hits). Done in M2b-2 Tasks 5–13 (07 §3 per screen); API gaps closed first in Tasks 1–3 (SEO as `seo` on content DTOs, with `0008_autosave_seo` for the autosave slot, P28; media `q`, `kind`, `trash` and restore, P33; comment `item` and counts, redirect `q`, menu `locations`) and Task 14 (`GET /content/refs`, R33).
- [x] Comment pagination (M2a-2b Task 8 review): a public thread shows the oldest 500 approved comments (`MAX_THREAD`), so later ones never appear, their `303 #comment-<id>` lands on a missing anchor and `count` reads 500; page the thread (WordPress `comment-page-N`) and give the real total. Done in M2b-2 Task 4: pages of top-level threads, `comments.per_page` (default 50), `?cpage=N`, the real total, the `303` to the page that holds the comment (P34, R25).
- [x] Follow-ups from M2b-1:
  - [x] Core validates that a written `body`'s root is `doc` (the API accepts any object; the editor checks it, M2b-1 Task 14 review): defence in depth. Done in Task 1, for a body that is sent or restored (R21).
  - [x] A local backup of unsaved edits while a kept copy (Take theirs) occupies the autosave slot, so a crash in that window loses nothing (R13). Done in Task 14 (`localStorage`, P40).
  - [x] The fields editor: a revision that differs only by a `null`-valued custom field counts as unchanged in the revisions drawer (Task 17 review M5 residual). Done in Task 8 (`null` differs from absent).
  - [x] The publish dialog opened (`mod+shift+p`) while the first save moves a new item from `/new` to `/$id` closes with the old screen (seen in the M2b-1 exit; the shortcut is lost, nothing else). Done in Task 14 (the hand-over opens it on `/$id`).

**Exit:** a fresh `init` + `seed demo` + `serve` on a release build: upload images and a PDF by
drag and drop, set alt text, see variants; set a featured image and an inline image and
publish; define a type with every field kind, write an item, change the schema and save the
item again; build a category tree; build the primary menu and see it on the site; moderate
comments from the queue and see the public page; page a thread past 50 threads; import and
export redirects and follow one; the workspace tests on both backends with and without
`admin/dist`; the admin gates and E2E; the contact sheet against 11 §8; bundle sizes against
the M2b-1 baseline.

**Exit verified (2026-10-26), with gaps:** [`docs/superpowers/m2b2-exit.md`](superpowers/m2b2-exit.md)
(the flows were automated in Google Chrome). **Not verified:** Safari. The first full E2E run
failed one test on a race between parallel tests (a relation search hitting a type another
test had just deleted; the second run passed); recorded there, not fixed. Before merging, CI
must pass on the branch.

Known limits (accepted in M2b-2, see the rulings): inline images are served as the original
file, without variants or `srcset` (R28); a relation field can point at an item of any type
the nav shows (P37); terms have no manual order (P35); a term with sub-terms cannot be
deleted until they are moved (R31); custom fields of a type without a schema are API-only
(P36); a schema change deletes a removed field's stored values when each item is next saved
(P39, R29); last-write-wins resources are guarded only by a client-side re-read (P31); a
restored media item comes back published even if it was unpublished before it was trashed
(R23); there is no search inside the media Trash (R22); a media search filtered by kind can
return short pages (R24); a comment page whose threads exceed 2 000 rows does not show its
latest replies (R25); moderators' comments and replies are not rate limited (R34); menu
custom links cannot be `mailto:` (R32); `refs` is a reserved type and taxonomy name (R36); an
undone redirect delete restarts its hits at 0 (P38); numbers take only `.` as the decimal
separator (R30).

#### M2b-3 — administration, extensions and the M2 exit (done)

Plan: [`2026-10-27-m2b3-administration-and-m2-exit.md`](superpowers/plans/2026-10-27-m2b3-administration-and-m2-exit.md).

- [x] Screens: own account (password, TOTP enrolment with QR code, own tokens), users (roles, tokens, TOTP), settings, themes (upload, activate, settings form from `theme.toml [settings]`), plugins (install flow with the permission report and fingerprint approval, enable/disable, delete/purge, settings with write-only secrets, per-plugin metrics charts), system (status, schema, backends, nodes reading `nodes.meta` incl. theme/plugin state, jobs with dead-job retry (deferred from M1.5), audit log). Import ships with M3, backups with M4 (P25). Done in M2b-3 Tasks 6–11 (07 §3 per screen; the account screen also lists and ends sessions); API gaps closed first in Tasks 1–3: user filters (P46), an admin's TOTP reset (P43), the password for one's own e-mail change (P44), sessions (P45); theme and plugin manifest facts, retained plugin data and its purge (P47), `ferrite theme pack` (P42); audit filters, `mail` in the status and the test mail (P49), typed node `meta`, the site title read at use time (P51), `MeDto.mail_configured` (Ruling A).
- [x] Playwright E2E against a real binary on both backends; Vitest component tests; a CI admin job (`pnpm install --frozen-lockfile`, lint, test, build). M2b-1 built Vitest, the CI `admin` job and Chromium E2E against the release binary (P22). Done in Task 14: every spec in four projects, `chromium-sqlite`, `chromium-pg`, `firefox-sqlite` and `webkit-sqlite` (P55; the site-wide specs in each project's teardown, R62), no engine skips, and the CI matrix job `admin-e2e` (10 §3). Two clean `e2e:all` runs in a row on the final code came in the exit's part B (`m2b3-exit.md` §5.1); the first CI run is still owed (R72). Safari stays a manual check (P55).
- [x] A bundle-size budget in CI, from the M2b-1 baseline (m2b1-exit.md "Bundle sizes") and the M2b-2 numbers (m2b2-exit.md §5). Done in Task 15: `admin/bundle-budget.json` (gzip -9 limits, measured at `3209291` plus 10 %), `pnpm -C admin bundle:check` after `build` in the `admin` job (P56).
- [x] Follow-ups from M2b-1 (parked review minors), done in Tasks 12 and 13:
  - [x] The restore banner's Discard Undo toast is not dismissed on unmount, so Undo after leaving rewrites the discarded draft into that item's slot (Task 16). Task 12: the toast goes with the reopened screen and at its first edit.
  - [x] `storeKept` may re-PUT the kept copy once more after `save()`'s own re-PUT (harmless except a needless stay on failure; Task 16). Task 12.
  - [x] The "Restoring…" toast lingers until the revisions list returns (`editor.tsx`, Task 17). Task 12.
  - [x] Tests for the discard-then-reload-failure fallback and for the dismissal of the kept-copy Discard toast (Task 16). Task 12.
  - [x] Phone status tabs: a neighbour tab can show only its bare count at the edge under the scroll fade (`content-list-trash` mobile; Task 18 contact sheet). Task 13 (`ScrollFade.reveal`, every tab row).
  - [x] Tests for the focus fallback after opening the revisions drawer from the phone sheet (M6) and for the Undo-without-edit-rights message (M7) (Task 17). Task 12.
  - [x] Verify `⌘⌥R` on a macOS browser that reports AltGraph for Option (R18). Task 13 made the code independent of that report (with ⌘ held, the physical key applies whatever AltGraph says; unit test with a real `KeyboardEvent`, R18 as updated); a check on a real Mac joins the manual Safari pass.
  - [x] A server hint (for example a cookie read when `index.html` is served) so an explicit theme choice against the OS does not flash one frame (11 §4, Task 7). Task 13: the `ferrite_admin_theme` cookie (P57).
  - [x] Palette: a snippet for hits that match only in the body (Task 11 review m5). Task 13 ("Matches in text").
- [x] Follow-ups from M2b-2:
  - [x] Responsive images for inline `ferrite:media/<id>` images: the renderer serves the original; give it the variants and a `srcset` like the theme's `image()` helper (R28). Task 4 (06 §4 "Inline images"; same-shape variants only, R41).
  - [x] `FieldDef` target types for relation fields, so a relation searches and accepts only the intended types (P37; a schema change). Task 4 (`types`, core refuses other types, P52) and Task 12 (the field builder and the picker).
  - [x] A migration path for a type or taxonomy named `refs` created before M2b-2 (R36). Closed by ruling P54: no released site predates M2b-2, so none is built.
  - [x] Media restore re-publishes an item that was unpublished before it was trashed (R23): restore its previous status instead. Task 4 (`media.trashed_from`, P53).
  - [x] Moderators' comments and replies are not rate limited (R34): decide whether a separate, higher limit is wanted. Decided by P54: no limit for moderators.
  - [x] The review minors M2b-2 left open (listed in [`m2b2-exit.md`](superpowers/m2b2-exit.md), "Open review minors"). Rust in Task 4 (fixed, or answered by R44–R46; the redirect `q` table scan is carried to M4, R43); admin in Tasks 12 and 13, with the contact-sheet findings 1–3 and 6–8 and the E2E race of its §2 (R71). The unused `qk.comments.lists` and comment i18n keys were removed before M2b-2 merged.
- [x] Docs and the M2 exit. Task 16: [`m2b3-exit.md`](superpowers/m2b3-exit.md), [`m2-exit.md`](superpowers/m2-exit.md).

**Exit:** a fresh `init` + `seed demo` + `serve` on a release build: sign in, enrol TOTP with a
real authenticator code (computed), sign out and back in with it; create a token and use it
with `curl`; create an editor and sign in as them; change settings and see them on the site
(title, front page, permalinks); pack and upload a theme, activate it, customize it; install
the example plugin through the permission report, enable it, set a secret, see its metrics,
delete it with and without erasing data; retry a dead job; filter the audit log; send a test
mail through a real SMTP catcher; the workspace tests on both backends with and without
`admin/dist`; the admin gates and the four E2E projects; the contact sheet against 11 §8; the
bundle budget; `docker build` serving `/admin/`.

**Exit verified (part A 2026-10-27, part B 2026-10-28, R76):**
[`docs/superpowers/m2b3-exit.md`](superpowers/m2b3-exit.md). Part A: every flow ran over the
REST API with `curl` against a fresh release build (a computed RFC 6238 code, a Python SMTP
catcher); the workspace tests on both backends with and without `admin/dist`, the admin
gates without E2E, the bundle budget, `fmt` and `clippy`. Part B: the four E2E projects
passed twice in a row on the final code (764 tests each), after runs that found two product
bugs (a command list's `aria-activedescendant` naming a removed option; a token scope label
truncated without a title) and needed ten test-harness fixes; the contact sheet
against 11 §8 (the account screens' snaps included); `docker build` serving the SPA at
`/admin/`; `chromium-sqlite` on Linux in the Playwright image; actionlint. **Not verified:**
Safari (with `⌘⌥R` on a real Mac), by the user by hand; CI has not run on this branch
(R72); Linux Firefox and WebKit, and Linux on x86_64. **Open from the contact sheet:**
`icon-sm`/`icon-xs` buttons stay under 44 px on touch screens (11 §8; WCAG 2.2 AA's 24 px is
met); dark dropdown menus have a faint edge; the palette's stale results, dimmed to 60 % while
a slow search runs, fall below 4.5:1.

Known limits (accepted in M2b-3, see the rulings): mail is configured in `ferrite.toml` only
(P49); media sizes are the active theme's, read-only (P50); hook latency is a node-local p95
bucket bound (P48); sessions are told apart only by their times (P45); there are no TOTP
recovery codes, and an admin can switch off another user's TOTP (P43); themes cannot be
deleted (M2a-2 P30); moderators are not comment rate limited (P54); a permalink change moves
URLs without redirects until M3 (P58); settings are last-write-wins behind the client-side
guard (P58, P31); the redirect search scans the table (R43); an emptied General field cannot
blank a `ferrite.toml` value (R61); a new audit action shows its raw code until mapped (R69);
Linux WebKit is not Safari (P55).

### Done from the earlier M2 deferred lists

- [x] User service: validate logins and emit `user:<id>` (from M1c-2; M2a-1).
- [x] Previews over HTTP; logged-in cache bypass / `cache.logged_in = "per-user"` variants (from M1c-2; M2a-1).
- [x] Validate `permalink.*` on option write and on type/taxonomy registration (from M1c-2; M2a-1, P19).
- [x] `sessions.prune` periodic job with the sessions table (from M1.5 P26; M2a-1).
- Moved to **Later**: multi-host sites (the cache-key host is pinned to `site.url`, R25), because a second host needs per-host `site.url`, cookies and CSRF origins (M2a P27).

## M3 — WordPress import (~2 weeks) (done)

Design: [08](design/08-wp-import.md).

M3-1 (the import core, the conversion and the CLI; plan
`superpowers/plans/2026-10-28-m3-1-wordpress-import-core.md`, rulings `superpowers/rulings/m3.md`,
exit [`m3-1-exit.md`](superpowers/m3-1-exit.md)) is done:

- [x] Streaming WXR parser → IR (temp SQLite), resumable, idempotent (`import_map` re-runs, P3–P4; resume by the WXR's SHA-256, R27).
- [x] Media import via `MediaStore`, URL rewrite (`--uploads`, `--fetch-media` through the SSRF-checked client; every size and `-scaled` form; links fixed after all content exists).
- [x] Gutenberg + classic → TipTap JSON; shortcode handling (`embed` node and image `caption`, P11; depth guards R31–R34, R48).
- [x] 301 redirects for old permalinks (items, attachment pages, tag and category bases; P17).
- [x] Yoast / Rank Math → SEO fields (`seo.canonical`, `seo.robots`); ACF → custom fields (keys verified in the plugin source and exports A and B, P10, P16).
- [x] `import-report.html` (as built: `report.json` + `report.html`); CLI (`ferrite import wordpress`).
- [x] Tests against real WP exports (as built: exports A and B from a scratch WordPress 7.1.3, checked in, and WordPress's two theme-test files via `FERRITE_WP_CORPUS`; no anonymised production export).
- [x] Batch events per transaction (one `invalidate` per batch, `[all]` past 1 000 keys; P6, R2 of M1.5).
- [x] Carried from the M1.5 final review (rulings/m15.md R17), the second half: the import's writes are fenced in the data by a run token checked first in every batch transaction (P5); the `import` lock is best-effort only.

### M3-2 (done)

M3-2 (the import job, API and admin; plan
`superpowers/plans/2026-10-29-m3-2-import-job-api-admin.md`, rulings P23–P34 and R52–R83 in
`superpowers/rulings/m3.md`, exit [`m3-exit.md`](superpowers/m3-exit.md)) is done, and so is M3:

- [x] The `import.wordpress` job (override `JobHandler::timeout()`, default 300 s, M1.5 P9; take the `import` lock, P13) and `POST /api/v1/import/wordpress` with `Idempotency-Key` (M2a-2b P26); `GET` on runs and the report download. Done in Tasks 1–5: migration `0010_import_jobs` (`import` artifacts; `import_runs.job_id`, `step`, `report_hash`), timeout 6 h and 3 attempts, inputs as `import` artifacts copied to the claiming node, step progress, the report stored as artifacts with a manifest; `/import/jobs/{id}`, `/import/runs`, `/import/runs/{id}` and its `report.json`/`report.html` (P23–P29; design 08 §1, 07 §1, 04 §6). The `import` lock stays best-effort; the run's fence is what counts (as built).
- [x] The admin import screen (upload, progress, link to the report) and the admin fields for `seo.canonical` and `seo.robots`. Done in Tasks 8–9 (P32, P33): `/admin/import` (`g i`) and the SEO section's canonical and robots toggles.
- [x] Deferred from M2a-2b (P26): redirects for `permalink.*` structure changes (today every published URL moves without a redirect; keep the previous structure and redirect by matching it, with the importer's redirect machinery). Done in Task 6 (P30): the `permalink.redirects` job (06 §3).
- [x] Carried from the M1.5 final review (rulings/m15.md R17): run `EXPLAIN (ANALYZE, BUFFERS)` on the Postgres job claim (`crates/ferrite-store-postgres/src/repos/job.rs`, the `claim` query) with ~50k future-scheduled `content.publish_scheduled` rows before the import creates them in bulk. With fewer due jobs than `limit`, the `OR` branch for expired leases stops the index scan from ending at `run_at > now()`, so each idle claim may read every future row (review M5). If it does, split the claim into a `UNION ALL` of the due branch and the expired-lease branch. Done in Task 7 (P31): the old claim read 50 005 rows, so it is now the `UNION ALL` split (`claim_explain`, `FERRITE_PG_EXPLAIN=1`; 04 §6).
- [x] A partial report for a failed run (M3-1 writes none: the CLI prints the error and exits 1). Done in Task 3 (P29): `RunFailed` carries it; the CLI writes it and exits 1; `--uploads` also takes a zip (P24).
- [x] The M3 exit (`m3-exit.md`): an import through the admin on both profiles, the Docker cluster example with an import. Done in Task 10 (P34); what could not be verified is listed there.

## M4 — Ops & release (~2–3 weeks) (done)

Design: [09](design/09-operations.md).

M4-1 (data safety) is done: [`m4-1-exit.md`](superpowers/m4-1-exit.md), rulings in [`rulings/m4.md`](superpowers/rulings/m4.md).

M4-2 (release) is done: [`m4-2-exit.md`](superpowers/m4-2-exit.md), rulings in [`rulings/m4.md`](superpowers/rulings/m4.md) ("M4-2 plan rulings", "M4-2 execution rulings"). Nothing is published: the first tag waits for the release gate in the exit (license, release key, the `release` environment and tag ruleset, GHCR visibility).

M4-3 (examples and docs) is done: [`m4-3-exit.md`](superpowers/m4-3-exit.md) (M4-3a and M4-3b).

M4-4 (the follow-up lists) is done: [`m4-4-exit.md`](superpowers/m4-4-exit.md). With it M4 is done: [`m4-exit.md`](superpowers/m4-exit.md) sums up M4-1 to M4-4. Nothing is released: next are the develop→main pull request and the release gate, under [Before v0.1.0](#before-v010).

- [x] `ferrite backup` / `restore`, `export` / `import-site` / `db copy`, `media sync` / `gc` (M4-1; 09 §4 and §8, 02 §10, 03 §6 and §8), plus the scheduled `[backup]` (`site.backup` job; 09 §4).
- [x] Cluster follow-up (M1.5 ruling R3): waiting for a Postgres named lock holds one connection with `lock_timeout` instead of opening a connection per 250 ms poll (P13), which removes the connect churn while waiting (M4-1 P19, `Store::lock_wait`; 04 §7).
- [x] `self-update` (signed), `rollback`, `rollback --schema`, `--restore-snapshot` (M4-2 P30–P32; 09 §1, §2; run end to end with two real binaries in the exit).
- [x] ACME TLS (solo) (M4-2 P33; 09 §5; tested against Pebble, not against Let's Encrypt).
- [x] `/metrics`, `node_metrics`, dashboard charts wired (M4-2 P34–P36; 01 §7, 09 §6, 07 §3).
- [x] Deferred from M2a-2b (P27): `date` filter time zones (M4-2 P29: jiff with the bundled tzdb; `date(fmt)` formats in `site.timezone`, `tz=` overrides it; permalink dates stay UTC; 06).
- [x] Deferred from M2a-2b (P27): `ferrite seed demo` atomicity (M4-1 P20: blobs first, then every row in one transaction through `ferrite_core::seed::DemoBatch`; a failed seed leaves no rows, its blobs go to the GC; 09 §8).
- [x] Release workflow for six targets and the GHCR image (publishing waits for the release gate), systemd unit, cluster and solo runbooks (M4-2 P27, P37, P38: `release.yml`, `packaging/systemd/`, `docs/operations/`; 09 §7). Checked locally only: one target (`aarch64-apple-darwin`) built and packed here, no GitHub run, nothing published (see [Before v0.1.0](#before-v010)).
- [x] Backward-compat CI job (M4-2 P38: `ci.yml` job `compat`, `.github/scripts/compat.sh`; it has something to run against from the second tag; run by hand in the exit with the M4-1 binary).

### M4-3 — examples and docs (done)

What the M4 list above already named and M4-1/M4-2 did not build. Split in two (M4 P41):
M4-3a (the example plugins, their release asset and `ferrite doctor`) is done:
[`m4-3a-exit.md`](superpowers/m4-3a-exit.md), rulings in [`rulings/m4.md`](superpowers/rulings/m4.md)
("M4-3a plan rulings", "M4-3a execution rulings"). M4-3b (the themes, Markdown import and the
docs site) is done too: [`m4-3b-exit.md`](superpowers/m4-3b-exit.md), rulings in
[`rulings/m4.md`](superpowers/rulings/m4.md) ("M4-3b plan rulings" P54–P65, "M4-3b execution
rulings" R-b1–R-b17). M4-3 as a whole: [`m4-3-exit.md`](superpowers/m4-3-exit.md).

- [x] Example plugins: SEO, privacy-friendly analytics, contact form (M4-3a P42–P47: `plugins/examples/{seo,analytics,contact-form}` with a README each, SDK helpers, one host change for `options.get("site.url"|"site.title")` (X1), core percent-encoding of `request.served` paths (X5), analytics' two storage tiers under the 10 MiB KV quota (`detail_days`, X7, from the final review); shipped as the release asset `ferrite-plugins-<version>.tar.gz` made by `.github/scripts/package-plugins.sh`; 05 §10 and §12, 09 §7).
- [x] Starter themes `docs`, `business`, with the template helpers a docs theme needs (page tree, previous/next, heading anchors, TOC) and Markdown import (M4-3b P54–P62: heading ids by GitHub's rule and `post.toc`; `SiteReader::tree` and the `page_tree()` helper with breadcrumbs and previous/next; `query(order="menu")`; `themes/docs` and `themes/business`, each under 150 KB; `seed demo --profile business`; `ferrite import markdown` in the new crate `ferrite-import-md`; code blocks as `<pre tabindex="0">` (R-b17); 06 §4 and §8, 08 §8, 09 §8).
- [x] Docs site (built with FerriteCMS itself; M4-3b P63: `site/content/` (26 pages), `site/build.sh`, `site/VERIFIED.md` with every claim and how it was checked, `crates/ferrite/tests/docs_site.rs`; axe checks on all three starter themes, P64, in the `public-themes-chromium` Playwright project). Hosting it is the user's decision (`m4-3b-exit.md` "For the user").
- [x] `ferrite doctor`: validates connectivity, permissions, clock skew and versions (M4-3a P48–P53, X2–X4: `ferrite doctor --data DIR [--offline]`, 25 checks in a fixed order, exit 0/2/1/3, nothing written locally (S3: one probe object); 09 §8). `ferrite plugin new` (a plugin crate scaffold) is planned for later in 09 §8, not for M4.

### M4-4 — follow-ups (done)

The open follow-up lists carried into M4 (from M1b to M4-3b), triaged by M4 ruling P66:
M4-4 fixed what could lose data, show a visitor or an admin something wrong, leak across a
permission boundary, flake CI or make the first release irreproducible, plus cheap
consistency fixes. What needs the user or a first real run is under
[Before v0.1.0](#before-v010); everything else moved to [Later](#later), each item with its
pointer. Exit: [`m4-4-exit.md`](superpowers/m4-4-exit.md); rulings in
[`rulings/m4.md`](superpowers/rulings/m4.md) ("M4-4 plan rulings" P66–P84, "M4-4 execution
rulings" R-c1–R-c3). No migration (the schema stays at 12), no WIT change.

- [x] Media follow-ups (from M1b):
  - [x] Document the S3 lifecycle rules: abort incomplete multipart uploads, expire `tmp/` (backstop for cancelled or failed staging cleanup). 09 §4a (M4-1 P22); its JSON example's field names are marked not verified.
  - [x] S3 `copy_if_not_exists` config option (M4-1 P16: `[media.s3] copy_if_not_exists`; unset, S3 still takes the head + copy fallback and `ArtifactRef.created` stays advisory; values per provider in 09 §4a).
  - [x] Decide `aws-lc-sys` vs `ring` for the static build: ring only (M4-2 P28). object_store uses `aws-base` + `reqwest` + `ring`, reqwest `rustls-no-provider`; aws-lc is gone from `Cargo.lock`, and CI's `packaging` job fails if it comes back.
  - [x] Expose `copy_if_not_exists` and `conditional_put` in `S3Config` (M4-1 P16: both are set on the builder when configured, on the explicit-keys and the `from_env` path alike; `conditional_put` defaults to object_store's `etag`). Note the explicit-keys vs `from_env` difference: with explicit `access_key_id`/`secret_access_key` the builder reads no `AWS_*` environment (so `AWS_COPY_IF_NOT_EXISTS`, `AWS_CONDITIONAL_PUT`, session tokens and instance/web-identity credentials do not apply); only the no-keys path uses `from_env`.
  - [x] `build_s3` replaced any client options read from the environment with its own timeouts. M4-4 (P77 group, Task 4): `s3_client_options` applies every `AWS_*` client option from the environment, then `allow_http` and its own timeouts last (03).
  - [x] Artifacts GC (`artifacts.gc` job, `ferrite media gc`; design 03 §6).
  - [x] `ferrite media regenerate`: regenerate variants after a theme change adds sizes (M2a-2b P8, P27; older uploads fall back to the original for the new sizes).
  - [x] Optional location-metadata stripping on upload: `[media] strip_metadata = true` removes EXIF and XMP losslessly from JPEG, PNG and WebP originals on upload and import (M4-1 P17; an image whose metadata cannot be removed is refused, R1; 03 §4). Off by default, so the stored original keeps its EXIF, GPS included, as WordPress does (M2a-2b R11).
- [x] Deferred from M2b-3 ([`m2-exit.md`](superpowers/m2-exit.md) "Open follow-ups"):
  - [x] An E2E of the Users screen's "Send a reset e-mail" through the SMTP sink (M4-4 Task 7: `users_reset_email_reaches_the_sink`).
  - [x] From the M2b-3 final review (`m2b3-exit.md`), M4-4 P77:
    - [x] Theme `homepage` goes through `web_url` in `ThemeDto::from_manifest`, as plugins' does.
    - [x] The plugin host's `mail_prefix` uses core `mail::subject_title` (`MAIL_TITLE_PREFIX_CHARS` is gone).
    - [x] Reset, TOTP-issuer and test mail fall back to the configured title when the site title read fails, as plugin `notify-admins` does (one helper, `settings::site_title_or_configured`).
    - [x] A blank `q` means no filter on every list endpoint (`GET /users` answered `422`).
    - [x] `check_relations` treats an item the actor cannot read as missing, so it no longer reveals its type; `check_parent` follows the same rule (an unreadable parent is "unknown parent"; authors and contributors can no longer pick another user's unpublished page as a parent).
  - [x] Test harness, from Task 16 of M2b-3 (M4-4 Task 7): the WebKit fetch rule counts navigation requests of the main frame only; the media 404 guard excuses only files the test did not upload; `Command` passes its ref through `useImperativeHandle`; the Vitest test `renders_the_shell_with_translated_title` has a 20 s per-test timeout.
  - [x] Contact sheet, from M2b-3 (M4-4 Task 6): `icon-sm`/`icon-xs` buttons reach 44 px on coarse pointers (11 §4; the `xs`/`sm` text buttons remain, see Later); popups draw a `--popover-edge` ring, stronger in dark (dark `--shadow-*` are transparent, not `none`, so rings draw); the palette's stale rows keep full colour with `aria-busy` and a spinner (P81), and nine other loading states do the same (R-c3).
- [x] Deferred from M3 ([`m3-exit.md`](superpowers/m3-exit.md) "Open follow-ups"):
  - [x] The orphan sweep (with `artifacts.gc`, M4-1: GC steps 2 and 3, 03 §6): `import` inputs left when a lease expires on the last attempt or the runner's timeout or a panic ends it (R70), blobs an upload stored but did not record when the 5 s lock wait failed or the request was dropped during it (R68), and report files an attempt replaced (P29).
  - [x] Import job: a job whose lease expired no longer counts as live (M4-4 P80, both stores and `job_may_start`); a truncated zip is refused, not retried (Task 5's regression test `a_truncated_zip_is_refused_not_retried` passed before and after; no code change).
  - [x] Uploads zip: entries that collide after NFC are ambiguous, none is read, each is reported as missing media (M4-4 P79).
  - [x] Admin (Tasks 8–9 minors; M4-4 Task 6): the SEO fields count code points and keep a BOM like the server, and announce limit crossings in a live region; the import screen's live regions exist before their first text; the in-progress wait offers Retry.
  - [x] `examples/cluster/` names `minio/minio` and `minio/mc`, which can no longer be pulled (2026-10-29); the M3 exit ran with `bitnamilegacy/minio` and `bitnamilegacy/minio-client` through an override (`m3-exit.md` §2). Pick maintained images and pin them by digest. M4-1 P21: RustFS 1.0.4 and `amazon/aws-cli` 2.34.0, with `postgres:18` and `caddy:2`, all pinned by digest (09 §7).
  - [x] Load flakes seen in the M3 exit's first Postgres workspace run (M4-4 Task 7): `serve` publishes its first heartbeat before serving (`Registry::beat_once`), so `job_kinds_are_in_meta::postgres` reads a complete row; the rate-window tests run again in a fresh window when theirs rolled over (`in_one_window`), and argon2 is optimised in test builds (P82).
  - [x] A database error deleting an import input's row is logged at `warn` (the expected foreign-key refusal stays `info`); the `job` span names the node that ran it, and `import.wordpress` logs `started` and `done` (M4-4 Task 5).
- [x] Deferred from M4-1 ([`m4-1-exit.md`](superpowers/m4-1-exit.md)):
  - [x] `media gc` with skipped deletes exited 0 (R16, R19). M4-4 P74: exit 2 for a refused store (another install's marker, an unreadable marker, a failed claim) or a failed delete; the first pass that only writes the marker stays 0.
  - [x] The daily `artifacts.gc` and `site.backup` periodics were both due at 00:00 UTC (R17 accepted residual). M4-4 P75: `Periodic::offset`; `artifacts.gc` is due at 03:00 UTC, `site.backup` keeps 00:00, the keys are unchanged (a backup running past 03:00, or a node starting late in the day, can still meet a GC pass).
  - [x] `restore`: a failure of the final check after the folder is in place exited 1; a `--keep` prune deleting the snapshot being read gave "the repository is damaged"; `--force` with an absolute media root inside the old folder left the restored site without media. M4-4 P68: exit 2 "restored, but restore could not check it"; the message names a `--keep` prune; an absolute media root inside an occupied or absent target is refused before anything moves (the new install id stays tested on the archive path only).
  - [x] A snapshot taken by the `site.backup` job holds that job's own row as `running`, so a restored site claimed it again (M4-1 exit §3). M4-4 P69: `restore` marks running `site.backup` rows `done`; other running rows are left to lease reclaim.
- [x] Deferred from M4-2 ([`m4-2-exit.md`](superpowers/m4-2-exit.md)):
  - [x] `--restore-snapshot` checked only the SQLite header. M4-4 P73: a read-only open of the snapshot, refused unless its applied schema equals the record's `schema_from`.
  - [x] `--restore-snapshot` by a non-root owner whose database has a group they are not in refused. M4-4 P73: a refused gid-only change keeps the owner's group with a warning (the decision is unit-tested; the refused `chgrp` itself is not, see Later).
  - [x] A crash between the last move and the final rename of `replace_db` left no `ferrite.db`, and the next `serve` created an empty site. M4-4 P70: opening a SQLite store refuses when `ferrite.db` is missing and a tmp exists (naming the `mv`); R-c2: also when the run had moved `ferrite.db`'s `-wal`/`-shm` into `backups/`; `rollback --restore-snapshot` refuses to start over such leftovers; `doctor` reports them; a tmp from which nothing moved only warns.
  - [x] `rollback --schema` on Postgres did not detect a live site. M4-4 P72: it refuses while the node registry shows a heartbeat fresher than `STALE_AFTER`; cluster mode with every node stopped is allowed (`migrate down` has no such check, see Later).
  - [x] `self-update --data` only warned when the new binary's `migrate status` failed on a config error. M4-4 P71: the new binary's `config check --data` runs first and refuses the update on exit 1 (with the hint about the service's environment); `--skip-config-check` overrides.
  - [x] Dashboard: the 7-day axis now has a tick for a first bucket that starts at local midnight (M4-4 Task 6).
  - [x] Flaky tests (M4-4 Task 7): the plugin tests that assert a plugin's output render with a 5 s deadline; `node.rs` waits are ceilings (`CONVERGE` 5 s, `SETTLE` 10 s), not latency bounds.
  - [x] `.github/scripts/sums.sh` defaults to 7 assets (the six target tarballs and the plugins asset); a partial set passes `EXPECTED` (checked in M4-4 Task 8; no change needed).
- [x] Deferred from M4-3a ([`m4-3a-exit.md`](superpowers/m4-3a-exit.md)):
  - [x] Release archives are byte-reproducible (M4-4 P83: `.github/scripts/repro-tar.sh`, used by `package.sh` and `package-plugins.sh`; `test-repro.sh` in CI's `packaging` job and in each release build job).
  - [x] SDK: `Head` lives in `ferrite-sdk/src/head.rs` (`lib.rs` 245 lines); `form` tests for lowercase hex and a multibyte sequence split across pairs; `plain_link_has_no_type` kept, as its reviewer said (M4-4 Task 7).
  - [x] Contact form: `valid_email` refuses Unicode format (Cf) characters (M4-4 Task 4).
  - [x] Doctor: `doctor/media.rs` passes the S3 fields in; the unreachable `MediaSection::Local` arm is gone (M4-4 Task 7).
  - [x] CI runs the plugins workspace's host unit tests (M4-4 P84).
- [x] Deferred from M4-3b ([`m4-3b-exit.md`](superpowers/m4-3b-exit.md)):
  - [x] The front page answered at `/` and at its own permalink. M4-4 P67: the permalink (any `?cpage=N` kept) answers `301` to `/` with `Cache-Control: public, max-age=3600`; `/` never redirects; children keep their paths; previews are unaffected; the sitemap, `SiteReader::tree` and menus name the front page `/` (06 §3).
  - [x] `image()`'s `srcset` lists only files of the chosen file's shape (Task 2).
  - [x] `crates/ferrite-theme/build.rs` reruns on `themes/` changes, so a new theme folder is embedded without `cargo clean` (Task 7).
  - [x] `TreeNode::excerpt` is documented as the stored excerpt (no `content.excerpt` filter), on the type and in 06 §4 (Task 2).
  - [x] Card images no longer spend the bridge budget: `ContentTree.media` carries the checked media and seeds the render's media map (Task 2).
  - [x] `menu_order_cmp` became `menu_order_key` with `sort_by_cached_key` (Task 2).
  - [x] Markdown import: a 256 MiB total-bytes cap; only folders that hold a page count toward the 5000 limit; a 50 000-folder cap on the walk (M4-4 P78).
  - [x] WordPress import sources must be `http(s)` URLs (M4-4 P77; `ImportError::Source`).
  - [x] `public-themes.spec.ts` names the stale binary and the rebuild command when a theme stylesheet differs (Task 7).
  - [x] `site/build.sh` prints `FERRITE_SERVER__LISTEN='[::1]:PORT'` for an IPv6 loopback URL (Task 7).
- [x] Deferred from M1c-2: automated accessibility checks (axe) in CI for the starter themes (06 §8). Done in M4-3b (P64): `admin/e2e/public-themes.spec.ts` in the `public-themes-chromium` project, its own step in the `admin-e2e` job.
- [x] Deferred from M2a-2a: a GC grace period for staged artifacts with the artifacts GC, which closes the same-bytes concurrent-install race (`Installer::discard_staged`; Task 4 review round 1). Done in M4-1: `discard_staged` is gone, failed installs leave their bytes to the GC (03 §6).
- [x] Performance, from the M1c-2 final review: sitemaps no longer walk every public row (M4-4 P76: `ContentRepo::file_bounds`, one query per type for the index's files and `lastmod`; a kind file seeks to its cursor; both backends).

## M5 — AI assistants (MCP)

Design: [12](design/12-mcp.md). Plan: [`plans/2026-11-01-m5-mcp.md`](superpowers/plans/2026-11-01-m5-mcp.md). Done (2026-11-02): [`m5-exit.md`](superpowers/m5-exit.md).

- [x] OAuth storage: migration `0013_oauth` (`oauth_clients`, `oauth_requests`, `oauth_codes`, `oauth_grants`, `api_tokens.oauth_grant_id`) on both backends with contract tests (12 §4).
- [x] Core `OAuthService`: clients (Client ID Metadata Documents and dynamic registration), pending requests, codes, grants, PKCE S256, refresh rotation with replay detection, revocation, audit `oauth.connect` / `oauth.disconnect`; grant-bound tokens in `AuthService` (12 §3).
- [x] OAuth HTTP endpoints (`/.well-known/…`, `/oauth/authorize|token|register|revoke`), the consent API under `/api/v1/oauth/`, the REST audience rule (an OAuth token is refused by `/api/v1`), the `mcp.enabled` off switch (12 §2, §3, §6).
- [x] Markdown in and out: `ferrite_core::blocks::markdown` and the importer's `markdown_to_body`, with `ferrite-block` fences for what Markdown cannot express (12 §5.3).
- [x] The `/mcp` endpoint (`rmcp`, Streamable HTTP, stateless): the guard (off switch, `405`, Origin, bearer, `401` challenge), 120 tool calls per user per minute, server instructions (12 §2, §5.1).
- [x] 56 tools in 13 areas, 2 resources (`ferrite://site`, `ferrite://guide/writing`) and 1 resource template (`ferrite://content/{type}/{id}`), and the prompts `write_post`, `review_seo`, `site_health`, `tidy_media`; successful writes audited as `mcp.<tool>`; no tool for the human-only actions (12 §5).
- [x] Admin: the consent screen (`/admin/connect`), Account → Connected apps, Users → a user's apps, Settings → AI assistants, the dashboard card, the palette entry (12 §8).
- [x] Docs, website and README: the guide `site/content/02-guides/08-ai-assistants.md`, the [MCP tools](../site/content/05-reference/02-mcp-tools.md) reference (kept in step with the server by `crates/ferrite-http/tests/api/mcp_reference.rs`), mentions in the Introduction, First site, Concepts and REST API pages, the landing page and the README (12 §9).
- [x] Exit: real-client checks (MCP Inspector, Claude Code, the OAuth flow in the E2E), the full gates, [`m5-exit.md`](superpowers/m5-exit.md) and [`rulings/m5.md`](superpowers/rulings/m5.md).
- [x] When 0.2.0 ships, drop the release caveat ("Coming in the next release (0.2.0); build from source today", and "arrive in 0.2.0" in the Introduction's status box) from: `site/brand/landing.html` (the pill, the line under the hero buttons, the quickstart comment lines, the AI section's caveat), `site/content/index.md`, `site/content/01-getting-started/02-first-site.md`, `site/content/01-getting-started/03-concepts.md`, `site/content/02-guides/index.md`, `site/content/02-guides/08-ai-assistants.md`, `site/content/03-developers/03-rest-api.md`, `site/content/05-reference/02-mcp-tools.md` and `README.md` (the intro bullet and the AI section), and the matching `site/VERIFIED.md` row.

Follow-ups (not in M5):

- Checks with the hosted clients themselves (Claude custom connectors, ChatGPT developer mode), which need a public HTTPS site; Claude Code was checked against a local site (a personal token connected; without one it asked to authenticate).
- Firefox and WebKit runs of `admin/e2e/mcp.spec.ts`, and the consent path for an account that must change its password (Task 9 rulings).
- A failed audit append after a committed write only logs (the write may go unaudited; Task 5 ruling).
- `content_preview_link` needs the human's admin session: no signed, expiring preview links for the model (Task 6 ruling).
- CI flake seen on PR #8: `ferrite-plugin-host` `tests/limits.rs` `concurrency_cap_skips_without_failure` (a 1 s `action_deadline` trapped with `interrupt` on a loaded runner; passed on rerun; not touched by M5).
- Pruning unused `oauth_clients` rows (Task 1 Minor 7); several resources per grant (Task 2 Minor 5).
- From design 12 §10: plugins contributing tools (a WIT `mcp` interface), a stdio bridge, per-tool allow lists per grant, MCP elicitation for confirmations, the ChatGPT Apps SDK widgets.

## Before v0.1.0

What the first release needs from the user or from a first real run (M4 ruling P66). Nothing
here is code that can be written and tested locally.

- [x] The release gate (the user's; [`m4-2-exit.md`](superpowers/m4-2-exit.md) §6, with the plugins asset as the seventh signed asset, `m4-3a-exit.md` §6): done 2026-11-01: the license (`AGPL-3.0-only` in `LICENSE`, with the plugin and theme permission in `LICENSE-EXCEPTION.md`, both in every binary and plugin asset; `MIT OR Apache-2.0` before it was never released), the minisign key pair and the real `crates/ferrite/release.pub` (key id `432051207B9BA065`), the repository made public, the GitHub environment `release` (deployment policy: tags `v*` only; secrets `MINISIGN_SECRET_KEY` / `MINISIGN_PASSWORD`), the tag ruleset "release tags" (creating, moving or deleting a `v*` tag needs the admin role). The first tag `v0.1.0` (on 22ecc83; migrations 0001–0012 are frozen) published the [release](https://github.com/softberries/ferritecms/releases/tag/v0.1.0) and the image; the GHCR package answers anonymous pulls (no visibility change was needed); its Actions access was not looked at.
- [x] The develop→main pull requests: #3 (M4), #4 (license, release key, CI fixes) and #5 (AGPL, Pages, README), each merged with every `ci.yml` job green (#4 and #5; #3 was merged before its run finished).
- [x] The first real `release.yml` runs (2026-11-01): the dry run 41302415395 and the tag run 41313266797 built all six targets (with `test-repro.sh` in each build job, so GNU tar 1.34 on ubuntu-22.04 and bsdtar on the macOS runners passed), the `plugins` job, the signature with the release key (`verify-sig.sh` against `release.pub`), the release and both images with their `0.1.0`, `0.1` and `latest` tags. Checked afterwards from the published files: the aarch64-apple-darwin archive verified with `minisign` and `shasum`, the README quickstart (`init`, `seed demo`, `user create`, `serve`: site and `/admin/` answer 200), `self-update --check` and doctor's `release.latest` ("0.1.0 is up to date"), and the image run through `init`, `user create`, `serve` and `ferrite doctor --offline` (11 ok, 0 warn, 0 error). Still open: the `compat` job from the second tag; the image carries no `org.opencontainers.image.licenses` label.
- [ ] Checks against real providers, never run here: S3 backup repositories (tested against `s3s-fs` only), the scheduled Postgres export's S3 upload and its abort, the owner marker's conditional create (`PutMode::Create`), AWS's answer to `copy_if_not_exists` values and the 09 §4a rows marked not verified (`m4-1-exit.md`); doctor against a wrong S3 bucket (`m4-3a-exit.md`); a real SMTP provider for the contact form and doctor's `smtp.connect`; ACME against Let's Encrypt (staging first) and doctor's `tls.acme_directory` success path; the systemd doctor command in 09 §8 and the cluster example's `chown 65532` + `chmod 600`, written from the docs (`m4-3a-exit.md` final review).
- [x] Hosting the docs site: GitHub Pages at https://softberries.github.io/ferritecms/, published by `.github/workflows/pages.yml` on every push to `main` (`site/export-static.sh` mirrors a FerriteCMS-built copy; no search on the static copy). Syntax highlighting in the `docs` theme is still open (Later).
- [ ] A manual Safari pass of the admin and the public themes, and real mobile devices (never done: [`m2-exit.md`](superpowers/m2-exit.md) §5, [`m3-exit.md`](superpowers/m3-exit.md); WebKit runs only as Playwright's build).
- [ ] `zip =9.1.3` is young (published 2026-10-27, M3 R59): recheck it on the next dependency update before the tag.

## Later

- Multi-site and multi-host sites (cache-key host pinned to `site.url`, M1c-2 R25, M2a P27); read replicas; multi-region.
- Signed plugin registry with verified publishers.
- TOTP recovery codes (M2a P9: none; an operator or, since M2b-3, an admin switches TOTP off, M2b P43) and a user agent per session so sessions can be told apart by more than their times (M2b P45; needs a migration).
- JS/Go/Python plugin SDKs (after checking Component Model toolchain maturity).
- Built-in forms, i18n/multilingual content, full-text search UI.
- WP database user import with legacy password-hash verification.
- Managed hosting offering.
- An atomic KV increment import (`kv.incr`) for counters such as the `counter` example's (whose read-modify-write can lose counts under at-least-once delivery): a WIT change, so it needs a new world version (M2a-2a ruling R19); the `analytics` example has the same caveat (M4 P44).
- Plugin world gaps the M4-3a examples met (M4 P42; each needs a new world version, 05 §11, and is named in the plugin's README): a modified date on `post` (for `article:modified_time` and JSON-LD `dateModified`), the user's role in `route-request` (instead of the `viewers` id lists, P46), a Reply-To in `notify-admins` (the contact form puts the visitor's address in the body), a way for a plugin page to take a signed-in post (the contact form has no per-message delete: a sandboxed page posts with `Origin: null`, which only `cross_origin` routes take, and those are served anonymously, P45), admin pages for plugins (the stats and inbox pages are plugin routes), and a `kv.get` that tells a failed read from a missing key (the host answers `None` for both and an empty list for a failed `kv.list`, so the analytics example lists before writing a new day and a failure of both can overwrite one).
- Docs and themes, later (M4-3b, left out on purpose): syntax highlighting in the `docs` theme (code blocks are plain, the language is only a `language-x` class), search snippets and section-scoped search, a static export (`ferrite site export --html`), versioned docs, admonitions, footnotes and task lists in Markdown import, YAML front matter (TOML only; the error says how to convert), per-menu-item attributes, and content types shipped by a theme.
- SEO example, later (M4 P43): IndexNow pings, `robots.txt` and sitemap changes (core owns both), content analysis.
- `ferrite doctor`, later (M4 P48, P53): `--json` output, a clock check against an HTTP `Date` header, detecting that it runs as another user than the service, and whether the binary's update path is writable for `self-update`.
- Comments, later items (M2a-2b P29): comments on pages and custom types (a per-type flag), per-item closing, comment action hooks for plugins (e.g. anti-spam) and admin notification mails.
- More upload types through a `media.extra_types` setting (M2a-2b P29; documents are accepted since R1), and `tel:` links through a URL-scheme setting (P21).
- Passkeys (WebAuthn, `webauthn-rs`): deferred past M2 (M2a P3; the newest `webauthn-rs` was a pre-release and the ceremony needs the SPA). There is no `passkeys` table yet.

### Follow-ups moved here from M4-4 (M4 P66)

Open items of the M1b–M4-3b follow-up lists that M4-4 did not fix, plus the ones M4-4
found, by origin. Each exit's "Not verified" and "Open follow-ups" sections and the named
rulings have the detail.

- From M4-4 ([`m4-4-exit.md`](superpowers/m4-4-exit.md)):
  - About 65 admin call sites use `Button`'s `xs`/`sm` text sizes, below 44 px on touch screens (WCAG AA's 24 px is met; 11 §4 "Known gap").
  - Sitemaps: `ContentRepo::file_bounds` still visits every published row's key of the type on each cache miss (and the row for `updated_at`); a covering index `(type, status, published_at DESC, id DESC) INCLUDE (updated_at)` needs a migration (02).
  - `migrate down` on Postgres has no live-node check (only `rollback --schema` has one, P72; the cluster runbook says to stop every node).
  - `--restore-snapshot`'s refused `chgrp` path is untested (only the `owner_plan` decision is; it needs a group the test user is not in).
  - A stray `backups/rolled-back-<ts>.db-wal` with no `ferrite.db.restore-*.tmp` beside it is not refused on open (reached only by running a refusal's recovery steps out of order); `db copy --from sqlite:` does not run the leftover check (its source is opened read-only).
  - Links built from `PageItem.url` (listing cards, related items, breadcrumbs from items, feeds, plugin `Post.url`, the API's `url`, redirect rows) still name the front page's permalink and take one `301` hop (06 §3).
  - Tests: `node.rs`'s fixed `sleep(reload_retry_wait())` can pass without the retry having run (it needs product instrumentation); the E2E `ownMedia` note runs after the asynchronous `r.json()` (a small lenient gap); an `in_one_window` retry is invisible in passing output; the hour-window store tests keep a 30 s margin and no retry; `cluster` `invalidation_latency_under_1s` failed once (4.5 s listener-cut maximum against its 1 s bound) while two cluster test binaries ran at once (CI runs test binaries one at a time; the bound is M1.5's); the `dashboard-metrics` 404 fix rests on an inferred cause; the E2E `failed_backup_restore_says_so_and_keeps_the_backup` failed once in `chromium-pg` during the exit's second full run (its toast did not appear; 10 of 10 alone; cause not established); macOS `ld` prints `__eh_frame section too large` for the `ferrite` bin.
- From M4-3b ([`m4-3b-exit.md`](superpowers/m4-3b-exit.md)):
  - The axe checks (P64) cover Chromium at desktop width only, with an empty comment thread: not Firefox, WebKit, the phone layout, a thread that has comments, or the contact-form plugin's own pages.
  - `query(order="menu")` and `latest` page by `(menu_order, id)` and sort ties by title only within the returned rows; a store order `(menu_order, lower(title), id)` would make paging exact (Task 2 review Minor 1).
  - A `page_tree()` miss reads every full row of the type (bodies and fields) for up to 1000 items, and every write in the type invalidates every page that drew the tree: a store projection without `body`/`body_html` on both backends (`crates/ferrite-core/src/site/tree.rs`; final review A Minor 3).
  - Past the 16th distinct `(kind, current)` pair in one render, `page_tree()` rebuilds the tree on every call (same class as the R14 known gap; A Minor 6).
  - Under any future public CSP, the themes' inline accent `<style>` and the contact form honeypot's `style=` need `style-src 'unsafe-inline'` or a hash/nonce (C Minor 8).
- From M4-3a ([`m4-3a-exit.md`](superpowers/m4-3a-exit.md)):
  - Analytics: that the feed, sitemap, `robots.txt`, search results and logged-in views are counted as views is read from `app/handler.rs`, not run live; a compaction whose delete of the full blob fails is not reported by the host, so the next batch adds that day's counts again (approximate counts, R19); no end-to-end test sends a raw `\` or `"` path through the router to the plugin.
  - The plugins asset has no wrapping folder (the P47 layout); the manifest-version rule is checked twice (CI's `tomllib` and `package-plugins.sh`'s awk).
  - SEO: a search page (`/?s=…`) gets the home page's `WebSite` JSON-LD, because a plugin's request path carries no query (a WIT change).
  - Contact form: an inbox blob whose version line is over 1 KiB (`MAX_VERSION_LINE`) is read as corrupt and replaced.
  - Doctor: read-only mounts and ACLs are not tested (mode bits only); `backup_target_on_s3_never_prints_its_keys` cannot show the scrub at work (the test server's HEAD answer has no body).
  - Doctor: the X3 `immutable` read can see a stale page if `serve` starts and checkpoints during doctor's few reads (accepted: a wrong finding or a `SQLITE_CORRUPT` line, never a write).
- From M4-2 ([`m4-2-exit.md`](superpowers/m4-2-exit.md)):
  - Image signing or provenance (cosign, or GitHub artifact attestations).
  - In-place update from the admin (P31), and rollback chains longer than one step (P32).
  - Rollback on a filesystem without hard links: `--restore-snapshot` refuses and the record's rename only warns (09 §2); a rename fallback (no-clobber checked first) would cover them (Task 5 N1/N2).
  - A missing carried snapshot drops the move's own snapshot from the record (Task 3, parked); on Postgres a finished record is carried forward (A→B→rollback A→C keeps A→B's `schema_from`; Task 3, accepted).
  - Warn when `ferrite.prev` is a binary this site never ran (two self-updates before a restart; A-Minor2).
  - After `--restore-snapshot` the media GC deletes the uploads only the moved-aside database refers to, after the grace period (A-Minor5; documented in 09 §2 and the runbook).
  - Metrics: no fuel family (P34), no job queue latency (01 §7), a public-only request count (R6), plugin hook latency as a series (the M2b-3 item below), a separate class or a GET/HEAD-only histogram for page latency (re-review Minor 3); the minute writer takes its baseline after the job runner starts (B-Minor7); a scrape in flight at shutdown can outlast the 1 s background stop (B-Minor8).
  - Dashboard: an E2E seam for the minute tick (the spec waits for real minutes); the Site traffic group's placement.
  - A release dry run's artifacts are signed with a throwaway key the embedded release key does not accept, so they can be self-update-tested only with a `test-seams` build.
- From M4-1 ([`m4-1-exit.md`](superpowers/m4-1-exit.md)):
  - Media-store owner marker on upgrade (R16, R19): a site upgraded to M4-1 deletes nothing on its first GC pass; two sites that already shared a store when they upgraded get an arbitrary owner, and the other skips its deletes for good (a daily error log, and `media gc` exits 2 since M4-4 P74) until the stores are split.
  - `ferrite export --with-media` on Postgres while `serve` runs does not hold `artifacts.gc` (R17): a blob the GC deletes during the export is reported as missing (exit 2, R10).
  - On an S3 backup repository, a run that reuses an old blob no kept snapshot lists does not refresh its age, so a concurrent `--keep` prune elsewhere can delete it (R14).
  - `site.backup` on two Postgres nodes at once is untested; so is loading a scheduled Postgres export with `import-site`.
  - `media regenerate`: the replaced-row race has no test (it needs a seam).
  - GC scaling: the protected hash list is sent again with every per-hash delete, and `protected()` reads every import report manifest on every pass. GC error policy: a failed `tmp/` delete is not tested (a failed row-less blob delete is, since M4-4); a Postgres dry run holds row locks from `mark_orphans` for its rolled-back transaction.
  - Import inputs are still deleted at once (R68, R74); leaving them to the GC would close the same-bytes race class P12 closed for extensions.
  - `db copy`: a panic during the copy skips the undo of a schema step taken for the target (no drop guard).
  - Strip (R1): a JPEG/PNG/WebP is buffered whole (up to 64 MiB); a streaming segment walker would lower the peak.
  - Later surfaces (P13, P15, P18, P23): an ad-hoc live backup of a SQLite site without the schedule, cron-style backup schedules, a job-based `media regenerate`, a Backups screen and API.
- From M3 ([`m3-exit.md`](superpowers/m3-exit.md)):
  - A `run_at`-leading index for the job claim's due branch if future jobs grow past about 500 000 (R54).
  - `permalink.redirects`: an item whose `published_at` moves later during the job gets no redirect (R62); imported posts may get redirects from never-live paths (R63); `LOCK_TYPES` is held for a page of 500; the E2E sweep assumes at most 500 posts.
  - Import job: a corrupt work file is removed on the attempt that fails; the run-folder sweep can race a starting run's `prepare_ir` rename in a narrow window.
  - Import API (Task 5 minors): a cleanup is lost when the request is dropped during its 5 s lock wait; a window where two uploads of the same bytes race; no idle timeout on the `options` part; the `202` fallback path untested; the per-part `413` seam skipped.
  - Uploads zip (Task 3 minor): trailing bytes after the end record are refused.
  - Expand lint (Task 1 minors): the down's match ignores the table name; duplicate literals are not flagged.
- From M2b-3 ([`m2-exit.md`](superpowers/m2-exit.md)):
  - Plugin hook latency across the cluster as a time series, with the metrics registry (the Plugins screen shows this node's p95 as a bucket bound, M2b P48).
  - An index for the redirect `q` substring search if large redirect sets need one (a trigram index is Postgres-only; M2b R43).
- From M2a-2a:
  - Per-node option cache, reintroduced only with a measured need (M2a-2 P14; it needs R13's refresh on `options.changed` on every node, [rulings/m2a.md](superpowers/rulings/m2a.md)).
  - A compile-size or function-count guard (or a compile deadline) for uploaded components (05 §6).
  - Fairness inside the per-render plugin budget (per-plugin shares, or a reservation for renders in the call semaphore; 05 §6 "Known limit").
  - Signed plugins / publisher identity: plugin data and sealed secrets follow the plugin name only, and the manifest's `publisher` is unverified (05 §2; M2a-2a final review M3, ruling R22).
- From M1c-2:
  - Small in-memory LRU for hot small `/_media` files (03 §5).
  - Atom feeds next to RSS 2.0 (06 §3).
  - `Cache-Control: public, max-age=0, s-maxage=<cfg>` on HTML for CDNs (06 §6; HTML pages send no `Cache-Control` today).
  - Signed or scoped direct media: with `[media.s3] public_base_url` (Direct mode) the published-only check and the attachment/`nosniff`/CSP headers do not apply (03 §5). Before `PublicUrlMode::Direct` ships, set `Content-Type` / `Cache-Control` attributes on the S3 path only (object_store's local backend rejects attributes, and the `Multipart` `copy_if_not_exists` mode does not copy them; from M1b).
  - Every hydration renders each body twice (`MediaRefs::of` collects references with a recording render, then the real render runs), including reads that never show the body; add a reader option that skips `html`, or collect the references in one pass (`crates/ferrite-core/src/site/hydrate.rs`; M1c-2 final review).
- From M1.5 ([`m15-exit.md`](superpowers/m15-exit.md)):
  - Template memory bound via a render worker process with an OS memory limit (P23, M1c-2 R14).
  - On-disk artifact / precompiled-component cache keyed by hash + wasmtime version + CPU features (P19).
  - `node.drain` handling (the event kind exists, the loop ignores it; P26).
  - PgBouncer transaction pooling (listeners and locks need direct connections; 02 §6, P26).
  - Run the cluster example end to end (`docker compose up` behind Caddy, the RustFS bucket init, Caddy health checks) in CI or a release check (P24; run by hand in the M3 and M4-1 exits).
  - An OS-level crash test (SIGKILL a `ferrite serve` child mid-job) and a silent network partition of the `LISTEN` connection (TCP proxy) (P25).
- From M1 and M0 (moved from their milestone lists in M4-4):
  - Full Unicode case folding for search (`İstanbul` indexes as `i̇stanbul`, so `istanbul` misses it; both backends) and locale-aware ordering for admin lists (byte-wise `COLLATE "C"` today) (M1a review).
  - Revisit the wasmtime pooling allocator under concurrent load (M0 review; it was planned for M4).
  - Test harness: a Postgres test that ends right after dropping an uncommitted `Tx` stalls about 5 s at teardown (10-testing); `ferrite init` validates the written config with `FERRITE_*` env overrides applied, so the validated config can differ from the file (09 §8); Postgres `status`, `down` and `verify` may create an empty configured schema (`PgStore::open`): a `PgOptions` flag so read-only commands do not create it (M1c-1 reviews).
