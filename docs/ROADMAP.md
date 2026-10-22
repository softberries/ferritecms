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

- [x] `ferrite-domain` crate (pure types: ids, Post, Term, User, Role, Block, errors; 01 §1), deferred from M0.
- [x] `ferrite-config` (toml + env, validation; M1c-1). Profiles are not implemented as a separate concept: `cluster.mode = auto|solo|cluster` derives solo vs cluster (design 01 §4).
- [x] `ferrite-store` traits, `ferrite-store-sqlite`, `ferrite-store-postgres`; migrations `0001_init` and `0002_search` on both (M1a).
- [x] Migration wrapper (M1a store side, M1c-1 CLI):
  - [x] Directives (phase / min-binary / lossy-down), compatibility verdict, `min_binary_version` preservation, SQLite pre-migration snapshot.
  - [x] `ferrite migrate status|up|down|verify` CLI (plus `ferrite init`; design 09 §8). The startup check (`startup_check`) exists but is only called once `serve` does (M1c-2).
- [x] Contract test suite (`ferrite-store-tests`) + migration roundtrip tests on both backends; CI with Postgres service.
- [ ] Store follow-ups from the M1a review (deferred minors):
  - [ ] Full Unicode case folding for search (`İstanbul` currently indexes as `i̇stanbul`, so `istanbul` misses it; same on both backends).
  - [ ] Importer (M3) needs a fallback author: `content_items.author_id` is non-optional.
  - [ ] Core/HTTP input validation (M1c) should reject NUL in text before it reaches the store (reads with NUL keys return `Invalid`).
  - [ ] Revisit locale-aware ordering for admin lists (byte-wise `COLLATE "C"` today).
  - [ ] Non-ASCII e-mail case variants are distinct accounts (ASCII-only `email_key`); decide before auth ships (M2).
- [x] `ferrite-media`: `MediaStore` over `object_store` (local + S3), content sniffing, `process_image` (M1b). Tests run against local and an in-process `s3s-fs` S3 server (no MinIO). The `artifacts` table itself is in the store crates; wiring is M1c.
  - [ ] M1c: `media.variants` job calling `process_image` + writing `artifacts` rows (via `insert_if_absent`; `ArtifactRef.created` is advisory) and the `media.variants` mapping.
  - [ ] M1c: image-processing concurrency semaphore in the job runner (peak ~3 x w x h x 4 bytes per image; ruling R4).
  - [ ] M1c: `/_media/<hash>/<name>` serving via `MediaStore::open_read` (streaming; headers, ranges, 416 mapping; design 03 §5). Use `inline_safe(mime)`: everything else is `application/octet-stream` + `Content-Disposition: attachment` + `nosniff`. Always pass an upload limit to `put_stream` (below the 5 GB single-CopyObject ceiling).
  - [x] M1c: install a process-default rustls `CryptoProvider` in `main` (both `ring` and `aws-lc-rs` are linked, so rustls panics building a client config without a default). Done in `crates/ferrite/src/main.rs` (`ring`).
- [ ] Core services (`ferrite-core`):
  - [x] Content: create/update with slug allocation and retry, revisions (keep 50), search indexing, status lifecycle (publish, unpublish, schedule, submit, trash, restore, delete, `publish_due`), permalink-change redirects, optimistic `version`.
  - [x] Roles and capabilities (`Actor`, `can`; design 07 §1), including the contributor rules (R8).
  - [x] Terms (`TaxonomyService`), options (`OptionService`), redirects (`RedirectService`, loop check), menus (`MenuService`).
  - [x] Blocks to HTML rendering with sanitising (`blocks`), slug generation (`slug`).
  - [ ] Content types service (the `ContentTypeRepo` exists in the store; no core service validates or manages types yet).
  - [ ] Search query service (core only indexes; `SearchRepo::search` is not wrapped yet).
  - [ ] Users service (M2 with auth).
  - [ ] Media wiring into core (`media` rows, `media.variants`; see M1c items above).
- [ ] Transactional outbox (`events`) written by every invalidating write:
  - [x] Written in the same transaction by content, lifecycle, term, menu and options writes in `ferrite-core` (`events.rs`).
  - [x] Redirect writes emit informational `redirect:<source>` events (M1c-2 Task 3); the dependency-key contract is documented in `events.rs`.
- [ ] `ferrite serve`: router, permalinks, redirects, 404, feeds, sitemap.
- [ ] Theme engine: minijinja, hierarchy resolution, safe helpers, dependency recording; `blog` theme.
- [ ] Page cache with dependency index (solo invalidation path).
- [ ] Plugin host wired into rendering (`render.head`, `content.body`, `content.excerpt`), plugins loaded from artifacts.
- [ ] Plugin host follow-ups from M0: `ammonia` sanitising of `html` output and head→HTML rendering (05 §7); `content.read` published-only and preview visibility (05 §7a); reject bare IPs in the http allowlist (SSRF) and implement real `http`/`mail` services; type-scope `meta` and stop `"*"` exposing other plugins' `plugin.<other>.*` meta; admin approval flow for limits above host defaults; revisit pooling allocator under concurrent load.
- [ ] More plugin host follow-ups from the M0 review:
  - [ ] `HostConfig` max-concurrent-calls semaphore (worst-case plugin memory = concurrency × per-call memory cap).
  - [ ] Per-render total plugin time budget (today each hook call has its own deadline; a page with many plugins can add them up).
  - [ ] Size caps on route responses (body and headers).
  - [ ] Success-path tests for `dispatch_action` / `handle_route` (M0 tests only cover rejection paths: unknown plugin, not subscribed, no matching route).
  - [ ] Document `HostServices` cancel-safety: futures are dropped at the hard timeout.
  - [ ] `install` compiles synchronously: run it under `spawn_blocking` when called from tokio.

- [ ] M1c-2 notes carried over from the M1c-1 reviews (deferred minors):
  - [ ] The router must check the redirect table before the page cache (01 §3), or redirect writes must emit invalidation events (they emit none today).
  - [ ] Test harness: a Postgres test that ends right after dropping an uncommitted `Tx` stalls about 5 s at teardown (10-testing).
  - [ ] `ferrite init` validates the written config with `FERRITE_*` env overrides applied, so the validated config can differ from the file (09 §8).
  - [ ] When a published parent's slug changes, its children's permalinks change too: add 301 redirects from the old child paths and invalidate the children's cache keys (`ContentService::update` only handles the item itself).
  - [ ] Serve PDFs inline with a CSP `sandbox` header (`application/pdf` is on the `inline_safe` allowlist in `ferrite-media/src/sniff.rs`; the `/_media` handler must add the header).
  - [ ] Add an SVG sniff assertion test (an SVG upload must sniff to a type for which `inline_safe` is false).
  - [ ] `ferrite migrate status` opens SQLite with the exclusive data-folder lock, so it fails while a server holds it. Either document this in the serve docs or add a read-only open for `status`.
  - [ ] "Publish now" on a scheduled item currently requires unpublishing first (`publish` accepts draft and pending only); allow scheduled -> published directly (a one-line change in `content/lifecycle.rs`).
  - [ ] Postgres `status`, `down` and `verify` may create an empty configured schema (`PgStore::open`); add a `PgOptions` flag so read-only commands do not create it.

**Exit:** a seeded site renders identically on SQLite+local and Postgres+MinIO; editing via CLI/seed invalidates the right pages.

## M1.5 — Cluster mode (~3 weeks)

Design: [04](design/04-cluster.md).

- [ ] Node registry + heartbeat, startup validation (mode/backends/site_id/clock skew).
- [ ] Event bus: `LISTEN/NOTIFY` + catch-up polling + gap handling; cache flush on lost history.
- [ ] Job queue: claim with `SKIP LOCKED`, leases, retries, dead jobs, periodic scheduler via unique keys.
- [ ] Distributed locks (advisory), used by migrate / install / import.
- [ ] Artifact sync for plugins and themes across nodes; per-node plugin state.
- [ ] Shared plugin failure counters + cluster-wide auto-disable.
- [ ] Cluster integration tests (2–3 in-process nodes): invalidation latency, job exclusivity, lease expiry, rolling version coexistence.
- [ ] `docker-compose` example: 2× ferrite + postgres + minio + caddy.

**Exit:** two nodes behind a proxy; a save on node A is reflected on node B within ~1 s; killing a node mid-job doesn't lose the job.

## M2 — API, auth & admin (~6 weeks)

Design: [07](design/07-admin-api.md).

- [ ] REST API v1 with OpenAPI (`utoipa`), problem+json errors, cursors, `If-Match`.
- [ ] Auth: argon2id, DB sessions, CSRF, API tokens, rate limiting, TOTP; passkeys stretch.
- [ ] Roles & capabilities enforced in core.
- [ ] Admin SPA scaffold: Vite + React + TS, shadcn/ui, TanStack Router/Query, Motion, generated API client, embedded via `rust-embed`.
- [ ] Screens: dashboard (Recharts), content list + TipTap editor + revisions, media library, content types/field builder, taxonomies, menus, users, comments, redirects, settings, system/nodes/jobs.
- [ ] Themes + plugins screens incl. permission report and per-plugin metrics.
- [ ] Plugin routes (`/_p/…`), plugin settings, `mail`, batched `request.served` (D21).
- [ ] Playwright E2E against a real binary on both backends.
- [ ] Hardening: sanitised `html` block nodes keep ammonia's default URL schemes, so `//evil` (scheme-relative), `ftp:` and `tel:` links pass (`ferrite-core/src/blocks`); decide an allowlist.
- [ ] SVG sanitising on upload (SVG is not processed by `ferrite-media`; design 03 §5 requires sanitisation before inline serving).

## M3 — WordPress import (~2 weeks)

Design: [08](design/08-wp-import.md).

- [ ] Streaming WXR parser → IR (temp SQLite), resumable, idempotent.
- [ ] Media import via `MediaStore`, URL rewrite.
- [ ] Gutenberg + classic → TipTap JSON; shortcode handling.
- [ ] 301 redirects for old permalinks.
- [ ] Yoast / Rank Math → SEO fields; ACF → custom fields (verify key names on real exports).
- [ ] `import-report.html`; CLI + admin job.
- [ ] Tests against 2–3 real WP exports.

## M4 — Ops & release (~2–3 weeks)

Design: [09](design/09-operations.md).

- [ ] `ferrite backup` / `restore`, `export` / `import-site` / `db copy`, `media sync` / `gc`.
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
- [ ] Release builds for all targets, Docker image, systemd unit, cluster runbook.
- [ ] Backward-compat CI job (from first tag).
- [ ] Example plugins: SEO, privacy-friendly analytics, contact form; starter themes `docs`, `business`.
- [ ] Docs site (built with FerriteCMS itself).

## Later

- Multi-site; read replicas; multi-region.
- Signed plugin registry with verified publishers.
- JS/Go/Python plugin SDKs (after checking Component Model toolchain maturity).
- Built-in forms, i18n/multilingual content, full-text search UI.
- WP database user import with legacy password-hash verification.
- Managed hosting offering.
