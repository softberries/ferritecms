# FerriteCMS — Roadmap

Detailed design: [`docs/design/`](design/00-overview.md). Each milestone ends with
something runnable and demoable. Work top-down.

Durations are rough estimates (not measurements) for one focused developer. Adding
Postgres, S3 and working cluster mode to the first release (decisions D1, D4, D5, D20)
moves the credible MVP from the original ~3 months to roughly **5 months**.

## M0 — Plugin host spike (de-risk first, ~2 weeks)

The riskiest part. Prove it before building a CMS around it. Design: [05](design/05-plugin-host.md).

- [x] Cargo workspace skeleton (`ferrite-plugin-host`, `ferrite-sdk`, `ferrite-domain`, `wit/`), CI with fmt + clippy.
- [x] WIT `ferrite:plugin@0.1.0` draft: `guest` (apply-filter, on-action, handle-request), `log`, `content` (stub), `kv` (stub).
- [x] Verify toolchain: `wasm32-wasip2` component output, unused imports dropped, WASI subset linking.
- [x] Install-time inspection: reject undeclared `ferrite:*` imports and `wasi:sockets`/`wasi:http`.
- [x] Per-plugin `Linker` with only granted interfaces.
- [x] Limits: `StoreLimits` memory, fuel, epoch deadline + tokio hard timeout.
- [x] Hostile plugins: loop, oom, panic, undeclared-import, sockets, big-output, wrong-variant, fs-escape.
- [x] Hello-world `render.head` plugin via `ferrite-sdk` macros.
- [x] Benchmark: fresh instance per call (pooling allocator) vs. reused instance. Result: ~27 µs fresh, ~24 µs pooling; reuse not needed (05 §6).

**Exit (met):** filter warm call ~27 µs (< 1 ms); all hostile plugins contained; findings written back into design 05.

## M1 — Storage, media, core & serving on both backends (~4 weeks)

Design: [02](design/02-storage.md), [03](design/03-media.md), [06](design/06-themes-rendering.md).

- [ ] `ferrite-config` (toml + env, validation, profiles).
- [ ] `ferrite-store` traits, `ferrite-store-sqlite`, `ferrite-store-postgres`; migration `0001_init` on both.
- [ ] Migration wrapper: directives (phase / min-binary / lossy-down), startup compatibility check, `ferrite migrate status|up|down|verify`, SQLite pre-migration snapshot.
- [ ] Contract test suite (`ferrite-store-tests`) + migration roundtrip tests on both backends; CI with Postgres service.
- [ ] `ferrite-media`: `MediaStore` over `object_store` (local + S3), artifacts table, tests against MinIO.
- [ ] Core services: content + revisions, types, terms, options, redirects, menus, search (FTS5 / tsvector).
- [ ] Transactional outbox (`events`) written by every invalidating write.
- [ ] `ferrite serve`: router, permalinks, redirects, 404, feeds, sitemap.
- [ ] Theme engine: minijinja, hierarchy resolution, safe helpers, dependency recording; `blog` theme.
- [ ] Page cache with dependency index (solo invalidation path).
- [ ] Plugin host wired into rendering (`render.head`, `content.body`, `content.excerpt`), plugins loaded from artifacts.
- [ ] Plugin host follow-ups from M0: `ammonia` sanitising of `html` output and head→HTML rendering (05 §7); `content.read` published-only and preview visibility (05 §7a); reject bare IPs in the http allowlist (SSRF) and implement real `http`/`mail` services; type-scope `meta` and stop `"*"` exposing other plugins' `plugin.<other>.*` meta; admin approval flow for limits above host defaults; revisit pooling allocator under concurrent load.

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
