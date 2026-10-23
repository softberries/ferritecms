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
  - [ ] Core/HTTP input validation (M1c) should reject NUL in text before it reaches the store (reads with NUL keys return `Invalid`). (The public router already rejects control characters in paths, `%00` included.)
  - [ ] Revisit locale-aware ordering for admin lists (byte-wise `COLLATE "C"` today).
  - [ ] Non-ASCII e-mail case variants are distinct accounts (ASCII-only `email_key`); decide before auth ships (M2).
- [x] `ferrite-media`: `MediaStore` over `object_store` (local + S3), content sniffing, `process_image` (M1b). Tests run against local and an in-process `s3s-fs` S3 server (no MinIO). The `artifacts` table itself is in the store crates.
  - [ ] `media.variants` job calling `process_image` + writing `artifacts` rows and the `media.variants` mapping, with an image-processing concurrency semaphore (M1b ruling R4). Moved to **M2** (media service and uploads; needs the M1.5 job queue). In M1 only `ferrite seed demo` writes media rows and variants, in-process.
  - [x] `/_media/<hash>/<name>` serving via `MediaStore::open_read` (streaming; ETag, 304, single ranges, 416, HEAD; `inline_safe` types inline, PDF with `Content-Security-Policy: sandbox`, everything else attachment + `nosniff`; published media only; design 03 §5). Upload limits for `put_stream` apply when uploads exist (M2).
  - [x] Install a process-default rustls `CryptoProvider` in `main` (both `ring` and `aws-lc-rs` are linked, so rustls panics building a client config without a default). Done in `crates/ferrite/src/main.rs` (`ring`).
- [ ] Core services (`ferrite-core`):
  - [x] Content: create/update with slug allocation and retry, revisions (keep 50), search indexing, status lifecycle (publish, unpublish, schedule, submit, trash, restore, delete, `publish_due`), permalink-change redirects (including descendants, rulings R1/R2), optimistic `version`.
  - [x] Roles and capabilities (`Actor`, `can`; design 07 §1), including the contributor rules (R8).
  - [x] Terms (`TaxonomyService`), options (`OptionService`), redirects (`RedirectService`, loop check), menus (`MenuService`).
  - [x] Blocks to HTML rendering with sanitising (`blocks`), slug generation (`slug`).
  - [x] Permalinks (`PermalinkRules`: `%slug%`, `%year%`, `%month%`, `%category%`, `%parent_path%`) and the published-only public read facade `SiteReader` (single, front page, archives, term/author/date archives, search, feeds, sitemaps, related, media files, preview) (M1c-2).
  - [ ] Content types service (the `ContentTypeRepo` exists in the store; no core service validates or manages types yet).
  - [x] Search query service for public search (`SiteReader::search`, published-only, capped at 200 matches, ruling R9). An admin search service arrives with M2.
  - [ ] Users service (M2 with auth).
  - [ ] Media service (`media` rows, `media.variants`; M2). `SiteReader` reads media rows (`media_file`, featured and inline images).
- [x] Transactional outbox (`events`) written by every invalidating write:
  - [x] Written in the same transaction by content, lifecycle, term, menu and options writes in `ferrite-core` (`events.rs`).
  - [x] Redirect writes emit informational `redirect:<source>` events (M1c-2 Task 3); the dependency-key contract is documented in `events.rs`.
  - [x] `theme.changed`, `plugins.changed` and `plugin.disabled` are emitted by `ExtensionService` and the shared failure counters (M1.5; design 04 §4 "Event kinds").
  - Not yet emitted because no write path exists: `user:<id>` (M2 user service), `post:<media id>` for media-row writes outside `seed` (M2 media service).
- [x] `ferrite serve`: router (pure, canonical paths, permalink matching), redirects before the cache, 404/410 shared pages, RSS feeds, sitemaps, `robots.txt`, `/healthz`, `/readyz`, bounded shutdown (M1c-2; design 06 §3, 09 §8).
- [x] `ferrite seed demo` (posts, pages, terms, menus, sample image with variants; M1c-2).
- [x] Theme engine: MiniJinja (fuel, output cap, no loader), hierarchy resolution (incl. front page, m1c2 R17), safe helpers, dependency recording; `blog` theme (M1c-2; design 06 §1–§4).
- [x] Page cache with dependency index and the outbox polling loop, gap handling with late-commit probe (R24) (M1c-2; design 06 §6, 04 §4).
- [x] Plugin host wired into rendering (`render.head`, `content.body`, `content.excerpt`; traced dependency keys and volatile marking), plugins loaded from the `plugins` table + artifacts at startup (M1c-2; design 05 §7a).
- [ ] Plugin host follow-ups from M0:
  - [x] `ammonia` sanitising of `html` output (R22) and head→HTML rendering with escaping and `script-tag` checks (05 §7).
  - [x] `content.read` published-only, preview visibility scoped to the previewed item (05 §7a); `options.get` never returns `plugin.*` keys.
  - [ ] Reject bare IPs in the http allowlist (SSRF) and implement real `http`/`mail` services (M2).
  - [ ] Type-scope `meta` and stop `"*"` meta grants exposing other plugins' `plugin.<other>.*` meta (M2).
  - [ ] Admin approval flow for limits above host defaults (M2).
  - [ ] Revisit the pooling allocator under concurrent load (M4).
- [ ] More plugin host follow-ups from the M0 review:
  - [ ] `HostConfig` max-concurrent-calls semaphore (worst-case plugin memory = concurrency × per-call memory cap). (Public content pages are bounded by the render slots: a page holds its slot for the whole pipeline, data load, plugin filters, head and template (R28, R32), so at most `2 × parallelism` pages run plugin calls at once. Plugin calls outside page renders, such as action hooks and plugin routes, are not capped.)
  - [ ] Per-render total plugin time budget (today each hook call has its own deadline; a page with many plugins can add them up).
  - [ ] Size caps on route responses (body and headers).
  - [ ] Success-path tests for `dispatch_action` / `handle_route` (M0 tests only cover rejection paths: unknown plugin, not subscribed, no matching route).
  - [ ] Document `HostServices` cancel-safety: futures are dropped at the hard timeout.
  - [ ] `install` compiles synchronously: run it under `spawn_blocking` when called from tokio (`serve` installs before it binds, so startup only).

- [ ] M1c-2 notes carried over from the M1c-1 reviews (deferred minors):
  - [x] The router checks the redirect table before the page cache (`crates/ferrite-http/src/app/handler.rs`); redirect writes also emit informational events.
  - [ ] Test harness: a Postgres test that ends right after dropping an uncommitted `Tx` stalls about 5 s at teardown (10-testing).
  - [ ] `ferrite init` validates the written config with `FERRITE_*` env overrides applied, so the validated config can differ from the file (09 §8).
  - [x] When a published parent's slug changes, its children's permalinks change too: 301 redirects from the old descendant paths and their cache keys are emitted (M1c-2 Task 1, rulings R1/R2; capped at 1000 walked descendants).
  - [x] Serve PDFs inline with a CSP `sandbox` header (`crates/ferrite-http/src/app/media.rs`).
  - [x] SVG sniff assertion test (`crates/ferrite-media/src/sniff.rs`, `inline_safe` false for sniffed SVG; plus an HTTP test that an SVG upload is served as an attachment).
  - [x] `ferrite migrate status` opens SQLite with the exclusive data-folder lock, so it fails while a server holds it: documented in the serve docs (09 §8); no read-only open.
  - [ ] "Publish now" on a scheduled item currently requires unpublishing first (`publish` accepts draft and pending only); allow scheduled -> published directly (a one-line change in `content/lifecycle.rs`).
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
- [x] Cluster integration tests (2–3 in-process nodes on one Postgres schema, plus two `ferrite serve` processes): invalidation latency, job exclusivity, lease expiry after a crash, periodic once-per-period, rolling version coexistence, concurrent first start, reload on every node, cluster-wide auto-disable (`crates/ferrite/tests/cluster/`, `crates/ferrite/tests/serve.rs`).
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

Design: [07](design/07-admin-api.md).

- [ ] REST API v1 with OpenAPI (`utoipa`), problem+json errors, cursors, `If-Match`.
- [ ] Auth: argon2id, DB sessions, CSRF, API tokens, rate limiting, TOTP; passkeys stretch.
- [ ] Roles & capabilities enforced in core.
- [ ] Admin SPA scaffold: Vite + React + TS, shadcn/ui, TanStack Router/Query, Motion, generated API client, embedded via `rust-embed`.
- [ ] Screens: dashboard (Recharts), content list + TipTap editor + revisions, media library, content types/field builder, taxonomies, menus, users, comments, redirects, settings, system/nodes/jobs.
- [ ] Themes + plugins screens incl. permission report and per-plugin metrics.
- [ ] Admin API calls `ExtensionService` (theme/plugin install, activation, enable/disable) only after `require(ManageThemes)` / `require(ManagePlugins)`: the service is an unauthenticated system path (M1.5 ruling R11).
- [ ] Plugin routes (`/_p/…`), plugin settings, `mail`, batched `request.served` (D21).
- [ ] Playwright E2E against a real binary on both backends.
- [ ] Hardening: sanitised `html` block nodes keep ammonia's default URL schemes, so `//evil` (scheme-relative), `ftp:` and `tel:` links pass (`ferrite-core/src/blocks`); decide an allowlist.
- [ ] SVG sanitising on upload (SVG is not processed by `ferrite-media`; design 03 §5 requires sanitisation before inline serving).
- [ ] Deferred from M1c-2:
  - [ ] Media service: `media` row writes emit `post:<media id>` in the same transaction (R11); the `media.variants` job and its concurrency semaphore (moved from M1); upload limits on `put_stream`.
  - [ ] User service: validate logins (slug-like, no `/`; author URLs `/author/<login>/` rely on it) and emit `user:<id>` on login/display-name changes and deletes.
  - [ ] Previews over HTTP (`SiteReader::preview` and the plugin preview scope exist; the route needs auth); logged-in cache bypass / `cache.logged_in = "per-user"` variants.
  - [ ] Term rename and `permalink.*` option changes record no redirects (only content moves do).
  - [ ] Validate `permalink.*` on option write and on type/taxonomy registration (today only `serve` checks them at startup).
  - [ ] `ferrite seed demo` atomicity (each step is its own transaction; a failed run leaves a partial batch).
  - [ ] Redirect hit counting spawns one task per hit, unbounded: use a bounded channel or coalesce.
  - [ ] Multi-host sites: the cache-key host is pinned to `site.url` (R25).
  - [ ] Theme install/activate (CLI/admin), refusing a broken theme on activation, and flagging a startup fallback to `blog` in the admin (06 §7).
  - [ ] `date` filter timezone support (`site.timezone`; today UTC).
  - [ ] Theme `query(term=…)` is capped at `reading.per_page` (it reads page 1 of the term archive): add a term + limit reader.
  - [ ] A child page with slug `page` is shadowed by pagination (`/parent/page/…`): reject it in slug validation.
  - [ ] A legacy item holding a reserved slug is renamed on update; every top-level write does an extra `types().list()`.
  - [ ] Wire the `render.footer` filter into rendering (in the hook catalogue, not called yet).
  - [ ] Make the 60 s short TTL configurable (search TTL with `0` disabling it, 06 §6; `plugins.volatile_ttl`, 05 §7a).
  - [ ] The M2 install flow must always set `plugins.granted` (a non-null `{}` skips every plugin that needs a capability, R21).
- [ ] Deferred from M1.5:
  - [ ] `ferrite theme install|activate|list` and `ferrite plugin …` commands over `ExtensionService` (P21; today `install_theme_dir` / `install_plugin_file` are library calls only).
  - [ ] `plugin.action` jobs (action hooks off the request path, 04 §6); each call must snapshot the plugin registry once (R13) (P26).
  - [ ] `sessions.prune` periodic job with the sessions table (P26).
  - [ ] Admin "Nodes" and "Jobs" screens reading `nodes` (incl. `nodes.meta` theme/plugin state) and `jobs` (queued kinds no node knows stay queued, P5; dead jobs with `last_error`).

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

- Multi-site; read replicas; multi-region.
- Signed plugin registry with verified publishers.
- JS/Go/Python plugin SDKs (after checking Component Model toolchain maturity).
- Built-in forms, i18n/multilingual content, full-text search UI.
- WP database user import with legacy password-hash verification.
- Managed hosting offering.
