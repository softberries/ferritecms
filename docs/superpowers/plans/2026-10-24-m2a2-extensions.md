# M2a-2a — Extensions & Plugin Platform Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Admins install, approve, enable, configure and remove plugins and themes over the API and the CLI, behind `manage_plugins` / `manage_themes` (closing M1.5 R11); plugins get their remaining v0.1 platform: routes under `/_p/`, settings with sealed secrets, `mail` to admins, action hooks as `plugin.action` jobs and batched `request.served`; users can reset a forgotten password by e-mail; plugin calls get a concurrency cap and a per-render time budget.

**Architecture:** No migration: everything fits the existing tables (`plugins.granted`, `options`, `rate_limits`, `jobs`, `plugin_kv`). `ferrite-core` gains the actor-checked `ExtensionAdmin` (wrapping the system path `ExtensionService`), the extension-settings schema and validator, a `Mailer` trait, password-reset tokens and the enqueueing of `plugin.action` jobs in write transactions. `ferrite-plugin-host` gains the permission report, grants with approved limits, the real `settings`/`mail` imports, route-response caps, a call semaphore and a call budget. `ferrite-http` gains `extensions::Installer` (the install pipeline moved out of the binary), the `/plugins` and `/themes` endpoints, `/_p/` routes, the SMTP mailer, password-reset endpoints and the `request.served` buffer. The binary gains `ferrite plugin …` / `ferrite theme …`, the `plugin.action` and `plugin.purge` job handlers and the `[mail]` wiring. M2a-2 is split in two (P1): this plan is **M2a-2a**; **M2a-2b** (media, comments, system API, hardening, the remaining deferred items) is outlined at the end.

**Tech Stack:** axum 0.8.9 (+ feature `multipart`, multer 3.1.0), utoipa 6.0.0 / utoipa-axum 0.3.0, lettre 0.11.23 (new), wasmtime 50, sqlx 0.9, tokio.

**Spec:** `docs/design/07-admin-api.md` §1 (endpoint table, M2a-2 rows), §2 (password reset); `docs/design/05-plugin-host.md` §2–§4c, §6–§9, §12; `docs/design/06-themes-rendering.md` §1, §4, §7; `docs/design/04-cluster.md` §4, §6–§8; `docs/design/02-storage.md` §3–§4; `docs/design/01-architecture.md` §4, §8; `docs/design/09-operations.md` §8; ROADMAP "M2a-2" and the plugin-host follow-up lists; rulings `docs/superpowers/rulings/{m1c2,m15,m2a}.md` stay binding. The controller copies P1–P30 below into a new `docs/superpowers/rulings/m2a2.md` at the start and records execution rulings there (referenced as "M2a-2 P<n>/R<n>").

## Global Constraints

- **Earlier rulings stay binding**: store parity (M1a R7/R13/R15 `validate_text` on every caller-text bind, R8 `COLLATE "C"`, R18 poisoning), M1c-1 P1 (never `read()`/`begin()` while holding a `Tx`; no slow I/O — compiling a component, fetching an artifact, sending mail — inside a `Tx`), M1c-1 P2 (correctness from versions, unique constraints and single-statement atomics), M1c-2 R20 (no writes on the render path), R21 (non-null `granted` must cover the manifest), the dependency-key contract (`ferrite-core/src/events.rs`), M1.5 P5/P28 (job kinds and payload `v`; events versioned by kind), P18/R13/R14 (hot swap, per-call registry snapshot, retry), P21 (install writes under the `extensions` lock), M2a P6 (scopes on `Actor`), P8 (CSRF + origin), P10/R25 (problem slugs), P11 (last-write-wins outside content), P17 (CLI = operator path), P22 (audit rows in the write `Tx`), R6 (rate-limit hits outside any `Tx`), R26 (sweep test, `role_can_edit`).
- **Conventions carried from M2a-1** (use these, do not re-invent): path parameters through `ApiPath<T>`; `?limit` through `check_limit(limit, DEFAULT_LIMIT, max)`; a bad cursor is `invalid_cursor()`; known unique constraints map to fields through `UNIQUE_FIELDS` in `api/problem.rs` (add a row when a new constraint can fire); an input error naming one member is `CoreError::invalid_field(field, msg)` → `422 validation` with `errors[0].field`; unpaged lists are `{ items, next_cursor: null }` ("Always `null`" doc line, as `MenuList`).
- **Capabilities are checked in core** (07 §1): `require(actor, cap)` for writes, `require_read(actor, area)` for admin reads, `can_read`/`role_can_edit` for content; this plan adds `require_reader(actor, cap)` (role holds `cap`, then `<area>:read`) for extension reads. Handlers call the core check **before** any expensive work (compiling, fetching, unpacking), then core re-checks on write.
- **Named locks** (`ferrite_store::{LOCK_MIGRATE, LOCK_EXTENSIONS, LOCK_TYPES, LOCK_IMPORT}`, `ferrite_cluster::LOCK_JOIN`): extension writes take `extensions` (`EXTENSIONS_LOCK_WAIT` 30 s, then `503 busy`); nothing here takes `types`. No new lock.
- **OpenAPI**: every new endpoint has a `#[utoipa::path]` registered through `routes!` in `api::v1()`; regenerate `crates/ferrite-http/openapi.json` with `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi` in the same commit. **Every new unsafe route must pass `unsafe_operations_need_auth_and_csrf`** (`crates/ferrite-http/tests/api/sweep.rs`); its `UNAUTHENTICATED` list grows by exactly `POST /api/v1/auth/password-reset` and `POST /api/v1/auth/password-reset/confirm` (Task 10). A multipart or tar endpoint must authenticate before reading the body (the sweep sends a non-JSON body).
- **Problem slugs as built (R25):** `not-found`, `method-not-allowed`, `internal`, `forbidden`, `unauthenticated`, `rate-limited`, `validation`, `precondition-failed`, `precondition-required`, `conflict`, `busy`, `bad-request`, `malformed-json`, `malformed-query`, `malformed-header`, `malformed-body`, `unsupported-media-type`, `too-large`, `invalid-credentials`, `csrf`, `password-reset-required`. This plan adds one: `mail-unavailable` (503). `/_p/` routes are not API: their errors are plain text.
- **Tests:** HTTP API tests use `crates/ferrite-http/tests/api/` (`api_tests!`, `Api`, `Client`, `Bearer`); plugin wiring tests `crates/ferrite-http/tests/plugins/` (`profile_tests!`, `Site`); real-binary tests the merged `crates/ferrite/tests/serve/` harness (`Profile`, `Server`); multi-node tests `crates/ferrite/tests/cluster/` (`Cluster`) or `TestDb::peer()`. Storage changes ship contract tests on both backends (`crates/ferrite-store-tests`, registered in `tests/contract.rs`). Test plugins live in `plugins/examples/` (members of `plugins/Cargo.toml`, built by `plugin_wasm`). Plugin-host changes include hostile plugins (CLAUDE.md).
- `thiserror` in libraries, `anyhow` only in `crates/ferrite`; no `unwrap`/`expect` outside tests; no `#[allow(dead_code)]`; `cargo fmt`, `cargo clippy --all-targets -- -D warnings`; `CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace` passes.
- **New dependencies, exact pins** (checked with `cargo search` on 2026-10-24 and compiled and run in the scratch probe): `lettre = { version = "=0.11.23", default-features = false, features = ["builder", "smtp-transport", "tokio1", "tokio1-rustls", "ring", "webpki-roots"] }` (ferrite-http); axum's existing workspace entry gains feature `"multipart"` (pulls `multer 3.1.0`). Probe-verified API shapes: `AsyncSmtpTransport::<Tokio1Executor>::from_url(&str)?.timeout(Some(d)).build()`, `AsyncTransport::send(&t, Message).await`, `Message::builder().from(Mailbox).to(Mailbox).subject(..).header(ContentType::TEXT_PLAIN).body(String)`; `smtp://host:port` is plain SMTP, `?tls=required` STARTTLS, `smtps://` implicit TLS; `axum::extract::Multipart::next_field()` / `Field::chunk()`; a route-level `DefaultBodyLimit::max(n)` applied as `OpenApiRouter::new().routes(routes!(h)).layer(..)` overrides the outer API limit. No other new crate.
- **Exact values** (constants unless marked config): plugin component upload ≤ 32 MiB (`MAX_PLUGIN_BYTES`), manifest part ≤ 64 KiB; theme upload ≤ `ferrite_theme::MAX_BUNDLE_BYTES`; at most 2 component compiles at once per node (`COMPILE_SLOTS`); manifest limit ceilings memory 256 MiB, fuel 10 × `plugins.default_fuel`; plugin data kept 30 days after delete (`PLUGIN_DATA_RETENTION`); route body ≤ 1 MiB, route response body ≤ 1 MiB, ≤ 32 response headers, each ≤ 4 KiB; route rate limit default `10/min`; `request.served` flush every 10 s or 1 000 events, buffer ≤ 10 000 events; `notify-admins` subject ≤ 200 chars, body ≤ 64 KiB, 1 mail per call, 10 mails per plugin per hour, ≤ 50 recipients; password reset token valid 1 h, request limits 5/min per IP and 3/h per account input; SMTP timeout 10 s; `plugins.max_concurrent_calls` (config) default `max(8, 4 × available_parallelism)`; `plugins.render_budget_ms` (config) default 1 000.

## Review Focus

1. **A plugin cannot get more than the admin approved.** An install or update without the fingerprint of the report the admin saw is refused with nothing written and the old version still running; a fingerprint for one artifact or grant does not approve another; limits above the host defaults load only when approved, and never above the ceilings. *Tests in Task 2 (`fingerprint_binds_artifact_and_grant`, `limits_above_default_need_approval`) and Task 4 (`install_needs_matching_approval`, `widening_update_without_approval_keeps_old_version`).*
2. **A hostile plugin route cannot reach the visitor's session or flood the site.** `route-xss`'s HTML runs in a sandboxed opaque origin (CSP `sandbox allow-forms`), never receives the cookie or `Authorization`, cannot set cookies or a cross-origin `Location`, and a flood is cut by the per-route limit counted across two nodes. *Tests in Task 8 (`route_xss_is_contained`, `route_flood_is_limited_across_nodes`, `route_response_caps`).*
3. **A plugin cannot spam or read secrets it was not given.** `mail-flood` is cut at 10 mails/h, recipients are active admins only; `secret` settings are never returned by the API after being set, `options.get` never returns them, and only the owning plugin's `settings.get` opens them. *Tests in Task 9 (`mail_flood_is_capped`) and Task 6 (`secrets_are_write_only_and_owner_scoped`).*
4. **Password reset does not leak accounts or outlive its use.** The request answers `202` the same way for known and unknown e-mails; a token works once, expires after 1 h, dies with any password change, and does not bypass TOTP. *Tests in Task 10 (`reset_does_not_enumerate`, `reset_token_single_use_and_expiry`, `reset_keeps_totp`).*
5. **An admin cannot brick the site with a theme or lose a plugin's data by accident.** Activating a theme that does not load is refused and the current theme keeps serving; deleting a plugin keeps its KV and settings for 30 days unless `purge=true`, and reinstalling within that time finds them. *Tests in Task 5 (`activation_refuses_broken_theme`) and Task 1 (`delete_keeps_data_until_purge`).*

---

### Task 1: `ExtensionAdmin` in core — actor checks, audit, plugin delete and purge

**Files:**
- Create: `crates/ferrite-core/src/extensions/admin.rs`, `crates/ferrite-core/tests/extension_admin.rs`
- Modify: `crates/ferrite-core/src/extensions.rs` (becomes `extensions/mod.rs` or keeps the file with `mod admin;`), `crates/ferrite-core/src/{actor.rs,lib.rs,jobs.rs}`, `crates/ferrite-store/src/repos.rs` (`PluginKvRepo::delete_all`), both backends' `src/repos/plugin.rs`, `crates/ferrite-store-tests/src/plugin.rs`, `crates/ferrite-store-tests/tests/contract.rs`, `crates/ferrite/src/{jobs.rs,serve.rs}`

**Interfaces — Produces:**
- `ferrite_core::require_reader(actor: &Actor, cap: Capability) -> Result<(), CoreError>` — `can(role, cap)` else `Forbidden(cap.as_str())`, then `require_read(actor, <area of cap.scope()>)`.
- `PluginKvRepo::delete_all(&self, plugin: &str, limit: u32) -> Result<u64, StoreError>` — deletes up to `limit` (1..=`MAX_PRUNE_BATCH`, else `Invalid`) keys of `plugin`; callers loop until a batch comes back short.
- `ExtensionService::delete_plugin(&self, name: &str) -> Result<InstalledPlugin, CoreError>` (system path; `extensions` lock; one `Tx`: row read (`NotFound`), row deleted, `plugin_failures().clear`, `plugins.changed`; returns the deleted row) and `ExtensionService::purge_plugin_data(&self, name: &str, deleted_at: Timestamp, settings: &[String]) -> Result<bool, CoreError>` (system path; `false` and nothing deleted when a row `name` exists with `installed_at > deleted_at`, i.e. reinstalled; else KV in batches outside any `Tx` and, in one `Tx`, options `plugin.<name>.settings.<key>` for every listed key with `options.changed` each).
- `ferrite_core::jobs::{PLUGIN_PURGE_KIND = "plugin.purge", PLUGIN_PURGE_VERSION = 1, PluginPurge { v, name: String, deleted_at: Timestamp, settings: Vec<String> }, plugin_purge_job(&PluginPurge) -> NewJob}` — `run_at = deleted_at + PLUGIN_DATA_RETENTION` (30 d), `unique_key = "plugin.purge:<name>:<deleted_at micros>"`.
- `ferrite_core::ExtensionAdmin` (exported), built `ExtensionAdmin::new(store: Arc<dyn Store>)`, `with_lock_wait(Duration)`:
  - `authorize_plugins(&self, actor) -> Result<(), CoreError>` = `require(actor, ManagePlugins)`; `authorize_themes(&self, actor)` = `require(actor, ManageThemes)` (handlers call these before uploads are compiled or unpacked).
  - `list_plugins(&self, actor) -> Vec<InstalledPlugin>` / `get_plugin(&self, actor, name)` (`require_reader(ManagePlugins)`); `list_themes(&self, actor) -> Vec<InstalledTheme>` (`require_reader(ManageThemes)`).
  - `install_plugin(&self, actor, artifact: &Artifact, row: &InstalledPlugin) -> Result<(), CoreError>` — `require(ManagePlugins)`; `row.granted` must be a JSON object (`invalid_field("approve", "a grant is required")` otherwise, closing the M1c-2 R21 item); same writes as `ExtensionService::install_plugin` plus audit `plugin.install` `{name, version, artifact, granted}` in the same `Tx`.
  - `set_plugin_enabled(&self, actor, name, enabled)` — audit `plugin.enable` / `plugin.disable`.
  - `delete_plugin(&self, actor, name, purge: bool) -> Result<(), CoreError>` — `delete_plugin` writes plus, in the same `Tx`, audit `plugin.delete {name, purge}` and (when `!purge`) `plugin_purge_job` with the setting keys from the row's `manifest.settings` table; with `purge` the data is purged right after the commit through `purge_plugin_data` (`deleted_at` = the commit's `Timestamp::now()`).
  - `install_theme(&self, actor, artifact, row, activate)`, `activate_theme(&self, actor, name)` — `require(ManageThemes)`, audit `theme.install` / `theme.activate`.
  - The private `write_*` helpers of `extensions.rs` take the audit row so `ExtensionService` (no audit, operator path) and `ExtensionAdmin` share one write per operation.
- Binary: `PluginPurgeHandler { extensions: Arc<ExtensionService> }` (kind `plugin.purge`, versions `[1]`, `Permanent` on a bad payload) added to `jobs::handlers`.

**Tests:**
- Store (both backends): `plugin_kv_delete_all_in_batches` — 25 keys of `a`, 3 of `b`; `delete_all("a", 10)` → 10, 10, 5, 0; `b` untouched; `limit = 0` → `Invalid`.
- Core (`tests/extension_admin.rs`, both backends): `extension_writes_need_capabilities` (editor → `Forbidden("manage_plugins")` / `Forbidden("manage_themes")`; admin token with `extensions:read` → list ok, install `Forbidden("extensions:write")`; author `list_plugins` → `Forbidden("manage_plugins")`); `install_requires_a_grant` (`granted = null` → `InvalidField { field: "approve" }`, nothing written); `writes_are_audited` (one audit row per write with the action names above, no secret in `details`); `delete_keeps_data_until_purge` (Review Focus 5: KV and a setting survive delete; a `plugin.purge` job exists with `run_at` 30 d out; running `purge_plugin_data` after a reinstall returns `false` and keeps the data; without reinstall returns `true` and both are gone; `purge = true` removes them at once).
- Binary (`crates/ferrite/tests/node.rs`): `purge_job_runs_through_the_runner` (a due `plugin.purge` job completes and the KV is gone).

- [ ] **Step 1:** Write the tests above (store, core, binary).
- [ ] **Step 2:** Run `cargo test -p ferrite-store-tests --test contract plugin_kv_delete_all`, `cargo test -p ferrite-core --test extension_admin` — expect compile failures.
- [ ] **Step 3:** Implement the store method on both backends, then `require_reader`, the core services and the job handler.
- [ ] **Step 4:** Re-run with `FERRITE_TEST_PG_URL` set plus `cargo test -p ferrite --test node purge_job` — expect PASS on both backends.
- [ ] **Step 5:** Commit `feat(core): ExtensionAdmin with capability checks, audit, plugin delete and purge`.

### Task 2: Permission report, grants with approved limits, manifest hardening

**Files:**
- Create: `crates/ferrite-plugin-host/src/report.rs`, `crates/ferrite-plugin-host/tests/report.rs`
- Modify: `crates/ferrite-plugin-host/src/{lib.rs,manifest.rs,error.rs}`, `crates/ferrite-http/src/plugins/{load.rs,reload.rs}`, `crates/ferrite-http/tests/plugins/{common.rs,reload.rs}`, `crates/ferrite/src/serve.rs` (`host_config`)

**Interfaces — Produces (plugin host):**
- `ApprovedLimits { memory: Option<u64>, fuel: Option<u64> }` (`Copy`, `Default` = nothing approved); `LIMIT_CEILING_MEMORY = 256 MiB`; `LIMIT_CEILING_FUEL_FACTOR = 10`.
- `PluginHost::prepare_with(&self, manifest: Manifest, wasm: &[u8], approved: ApprovedLimits) -> Result<PreparedPlugin, PluginError>` — effective limit per resource = the manifest's request when ≤ default, or when ≤ ceiling **and** ≤ the approved value; otherwise `LimitAboveDefault` (unapproved) or new `PluginError::LimitAboveCeiling { limit, requested, ceiling }`. `prepare(m, w)` = `prepare_with(m, w, ApprovedLimits::default())`.
- `Grant { capabilities: Capabilities, limits: ApprovedLimits }` with `Grant::of(&Manifest) -> Grant` (exactly what the manifest asks), `to_json(&self) -> Value` = `{"v":1,"capabilities":<[capabilities] table as JSON>,"limits":{"memory":n|null,"fuel":n|null}}`, `Grant::from_json(manifest_plugin_table: &Value, granted: &Value) -> Result<Grant, String>` (also accepts the M1c-2 bare `[capabilities]` table, with no approved limits).
- `PermissionReport { plugin: String, version: String, artifact: String, items: Vec<PermissionItem>, added: Vec<PermissionItem> /* items not covered by `current` */, limits: Vec<LimitItem>, fingerprint: String }`; `PermissionItem { key: &'static str /* e.g. "content.read" */, text: String /* plain language, e.g. "read published posts and pages" */, detail: Vec<String>, high_risk: bool }`; `LimitItem { name, requested, default, ceiling, above_default: bool }`. High risk: `render.html = "raw"`, any `http` host, `mail`, `routes` (one item per route with methods), `content.meta.write` beyond `plugin.<name>.*`, any `"*"` pattern, any limit above default.
- `Manifest::permission_report(&self, artifact: &ArtifactHashText /* &str, 64 hex */, current: Option<&Grant>, defaults: (u64 /* memory */, u64 /* fuel */)) -> Result<PermissionReport, PluginError>` (`LimitAboveCeiling` refused here, before anything is stored). `fingerprint` = lowercase hex SHA-256 of the compact JSON of `{"plugin":name,"version":v,"artifact":hash,"grant":Grant::of(self).to_json()}` (serde_json maps are sorted: no `preserve_order` in the workspace).
- Manifest validation: an `http` host that parses as an IPv4 address, whose labels are all digits, or that is `localhost` / ends with `.localhost` is a problem at `capabilities.http` ("bare IP addresses and localhost are not allowed").

**Interfaces — Produces (http):** `check_row` returns `(Manifest, ApprovedLimits)` from `Grant::from_json`; `PrepareFn` becomes `Fn(&PluginHost, Manifest, &[u8], ApprovedLimits)`; the reloader calls `prepare_with`.

**Tests** (`tests/report.rs` and unit tests): `report_lists_capabilities_in_plain_language` (the design example manifest of 05 §3: items for content.read `post, page`, meta read/write `seo.*` (high risk: outside own namespace), options, kv, http `api.indexnow.org` (high risk), render head/text, routes `/submit POST` (high risk), mail (high risk)); `report_diff_against_current_grant` (current = v1's grant; v2 adds an http host → `added` holds exactly that item); `fingerprint_binds_artifact_and_grant` (Review Focus 1: changing the artifact hash, version, any capability or a limit changes it; equal inputs give equal fingerprints); `limits_above_default_need_approval` (Review Focus 1: memory 64 MiB with default 32 MiB → `prepare` refuses, `prepare_with(approved 64 MiB)` loads, approved 48 MiB refuses; 300 MiB → `LimitAboveCeiling` even approved); `grant_json_roundtrip_and_legacy_form`; `bare_ip_and_localhost_hosts_rejected` (`127.0.0.1`, `10.0.0.1`, `1234`, `localhost`, `a.localhost` refused; `api.example.com`, `x1.example` accepted); http `reload_honours_approved_limits` (a row whose manifest asks 64 MiB loads only with `granted.limits.memory = 64 MiB`).

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-plugin-host --test report` and `cargo test -p ferrite-http --test plugins reload_honours` — expect FAIL.
- [ ] **Step 3:** Implement `report.rs`, `Grant`, `prepare_with`, the manifest check; switch the reloader.
- [ ] **Step 4:** Run `cargo test -p ferrite-plugin-host` and `cargo test -p ferrite-http --test plugins` — expect PASS (existing hostile tests unchanged).
- [ ] **Step 5:** Commit `feat(plugin-host): permission report, grants with approved limits, no bare-IP hosts`.

### Task 3: The install pipeline as a library — `ferrite_http::extensions::Installer`

**Files:**
- Create: `crates/ferrite-http/src/extensions.rs`, `crates/ferrite-http/tests/plugins/install.rs`
- Modify: `crates/ferrite-http/src/lib.rs`, `crates/ferrite-theme/src/{bundle.rs,lib.rs}` (`pack_embedded`), `crates/ferrite/src/{extensions.rs,site.rs,node.rs}`, `crates/ferrite/tests/extensions.rs`

**Interfaces — Consumes:** Task 1 `ExtensionService`, Task 2 `PermissionReport`, `Grant`, `prepare_with`.

**Interfaces — Produces:**
- `ferrite_theme::pack_embedded(name: &str) -> Result<Vec<u8>, ThemeError>` (same bytes `pack(&ThemeSource::Embedded(..))` gives; unknown name `NotFound`).
- `ferrite_http::extensions::ExtensionError` (`thiserror`): `Manifest(ManifestError)`, `Component(PluginError)`, `Theme(ThemeError)`, `TooLarge { limit: u64 }`, `Media(MediaError)`, `NameMismatch { bundle: String, row: String }`, `HashMismatch`, `Approval(String)`, `Core(CoreError)`; `impl From<ExtensionError> for api::Problem` (`Manifest`/`Component`/`Theme`/`Approval`/`NameMismatch` → `422 validation` with `errors` from the manifest problems or `field = "approve"`; `TooLarge` → `413 too-large`; `Core` via `From<CoreError>`; the rest `500 internal`).
- `Installer::new(store: Arc<dyn Store>, media: Arc<MediaStore>, host_config: HostConfig) -> Result<Installer, PluginError>` — owns one scratch `PluginHost` over `InMemoryServices` (prepare never commits) and a `Semaphore(COMPILE_SLOTS = 2)`; compiles run under `spawn_blocking`.
- `Installer::inspect_plugin(&self, manifest_toml: &str, wasm: Bytes) -> Result<Inspected, ExtensionError>` — parse, hash, report against the current row's grant (if installed), prepare with the requested limits treated as approved (so a component that cannot load is refused now); nothing written. `Inspected { manifest: Manifest, manifest_json: Value, report: PermissionReport, wasm: Bytes }`.
- `Installer::stage_plugin(&self, inspected: Inspected, approve: &str) -> Result<(Artifact, InstalledPlugin), ExtensionError>` — `approve` must equal `report.fingerprint` (`Approval("the approval does not match this upload; inspect it again")`); uploads with `put_artifact`; returns the artifact and a row with `granted = Grant::of(manifest).to_json()` (always an object), `enabled = false`. The caller records it (`ExtensionAdmin::install_plugin` for the API, `ExtensionService::install_plugin` for the CLI).
- `Installer::stage_theme(&self, bundle: Bytes) -> Result<(Artifact, InstalledTheme), ExtensionError>` — size ≤ `MAX_BUNDLE_BYTES` before parsing, `Theme::load(Bundle)` on the blocking pool, upload; the row's `manifest` JSON is the M1.5 object plus `"settings": <SettingsSchema JSON>` and `"images"` (Task 6 defines `SettingsSchema`; until then `"settings": {}`).
- `Installer::theme_from_artifact(&self, row: &InstalledTheme) -> Result<Theme, ExtensionError>` — moved from `crates/ferrite/src/site.rs::artifact_theme` unchanged in behaviour (bounded read, SHA-256 check, unpack, name check); `site.rs` calls it.
- `Installer::stage_embedded(&self, name: &str) -> Result<(Artifact, InstalledTheme), ExtensionError>` — `pack_embedded` + `stage_theme`.
- Binary: `install_plugin_file` and `install_theme_dir` become thin wrappers over `Installer` + `ExtensionService` (operator path; `install_plugin_file` approves `Grant::of(manifest)`, i.e. honours the manifest as an operator decision, and now writes it non-null).
- `nodes.meta.theme.fallback: bool` — `true` when the node runs a fallback (startup) instead of the active row's theme (`NodeStatus::set_theme` gains the flag; `load_theme` sets it when the active row's artifact failed).

**Tests** (`tests/plugins/install.rs`, both profiles): `inspect_writes_nothing` (no artifact object, no row); `stage_requires_the_fingerprint` (wrong / stale fingerprint → `Approval`; correct → row with object `granted` and the requested limits); `oversized_uploads_are_refused` (`MAX_BUNDLE_BYTES + 1` theme → `TooLarge` before unpacking); `broken_component_refused_at_inspect` (`undeclared-import` → `Component`); `compile_slots_bound_concurrency` (3 concurrent inspects; at most 2 compiles in flight, via `with_prepare`-style counter); binary `crates/ferrite/tests/extensions.rs` keeps passing; `install_plugin_file_writes_a_grant`; `fallback_is_flagged_in_meta` (tampered active artifact → `meta.theme.fallback = true`).

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-http --test plugins install` — expect compile failure.
- [ ] **Step 3:** Implement; move `artifact_theme`; make the binary wrappers delegate.
- [ ] **Step 4:** Run `cargo test -p ferrite-http --test plugins`, `cargo test -p ferrite --test extensions`, `cargo test -p ferrite` — expect PASS on both profiles.
- [ ] **Step 5:** Commit `refactor(http): install pipeline as a library with approval fingerprints`.

### Task 4: Plugins API — list, inspect, install, enable/disable, delete

**Files:**
- Create: `crates/ferrite-http/src/api/{plugins.rs,multipart.rs,dto/extensions.rs}`, `crates/ferrite-http/tests/api/plugins.rs`
- Modify: `Cargo.toml` (axum `multipart`), `crates/ferrite-http/src/api/{mod.rs,state.rs,dto.rs}`, `crates/ferrite-http/tests/api/{main.rs,harness.rs}`, `crates/ferrite/src/serve.rs`, `crates/ferrite-http/openapi.json`

**Interfaces — Produces:**
- `ApiParts`/`ApiState` gain `extensions: Arc<ExtensionAdmin>`, `installer: Arc<Installer>`, `media: Arc<MediaStore>` (node states are read through `store.read().nodes()`); `serve` builds them (`Installer::new(store, media, host_config(&config.plugins))`).
- `api::multipart::read_parts(mp: Multipart, spec: &[(&'static str, usize /* max bytes */)]) -> Result<BTreeMap<&'static str, Bytes>, Problem>` — unknown or repeated part → `400 malformed-body` naming it; a part over its limit → `413 too-large`; a missing required part → `422 validation` with `field` = part name; wrong `Content-Type` → `415`.
- Endpoints (`Authed`; the core `authorize_plugins` runs before the body is read):
  - `GET /plugins` → `PluginList { items: Vec<PluginDto>, next_cursor: null }`; `PluginDto { name, version, enabled, disabled_reason, installed_at, updated_at, report: PermissionReportDto /* of the stored grant */, nodes: Vec<PluginNodeState { node, status, error }> /* from nodes.meta.plugins */ }`.
  - `GET /plugins/{name}` → `PluginDto`.
  - `POST /plugins/inspect` (multipart `manifest` ≤ 64 KiB, `component` ≤ 32 MiB; route body limit 33 MiB) → 200 `PermissionReportDto { plugin, version, artifact, items, added, limits, fingerprint, update: bool }`; nothing installed.
  - `POST /plugins` (multipart `manifest`, `component`, `approve` ≤ 128 B) → 201 `PluginDto`, `Location: /api/v1/plugins/<name>`; installed disabled for a new plugin, a replaced one keeps its state (its new grant replaces the old one).
  - `POST /plugins/{name}/enable`, `POST /plugins/{name}/disable` → 200 `PluginDto`.
  - `DELETE /plugins/{name}?purge=true|false` (default false) → 204.
- No `If-Match` (P11).

**Tests** (both profiles; the harness gains `Client::multipart(api, path, parts)`): `plugin_install_flow` (inspect `hello` → report; install with the fingerprint → 201, disabled; enable → `plugins.changed` and the hook runs on `/` within one poll; disable; delete → 204, row gone); `install_needs_matching_approval` (Review Focus 1: no `approve` → 422 `field = approve`; fingerprint of another upload → 422; nothing stored); `widening_update_without_approval_keeps_old_version` (Review Focus 1: v2 of a fixture adding `kv = true`, the v1 fingerprint → 422, the row still names v1's artifact; v2's fingerprint → 201 and `report.added` was the `kv` item); `editor_cannot_manage_plugins` (403 `manage_plugins` before the upload is compiled: the compile counter does not move); `multipart_limits` (33 MiB component → 413; unknown part → 400); `delete_with_purge`; the sweep test passes with the new routes.

- [ ] **Step 1:** Write the tests above and add the `v2` fixture (`plugins/examples/hello-v2`, `kv = true`).
- [ ] **Step 2:** Run `cargo test -p ferrite-http --test api plugins` — expect FAIL.
- [ ] **Step 3:** Implement; wire `serve`; regenerate the snapshot.
- [ ] **Step 4:** Run `cargo test -p ferrite-http --test api` (sweep included) and `cargo test -p ferrite --test serve` — expect PASS.
- [ ] **Step 5:** Commit `feat(api): plugins with inspection, approval fingerprints, enable/disable and delete`.

### Task 5: Themes API — list, upload, activate (refusing a broken theme)

**Files:**
- Create: `crates/ferrite-http/src/api/themes.rs`, `crates/ferrite-http/tests/api/themes.rs`
- Modify: `crates/ferrite-http/src/api/{mod.rs,dto/extensions.rs}`, `crates/ferrite-http/openapi.json`

**Interfaces — Produces:**
- `GET /themes` → `ThemeList { items: Vec<ThemeDto>, next_cursor: null }`: every row plus every embedded theme without a row; `ThemeDto { name, version, description, origin: "installed" | "embedded", active: bool, installed_at: Option, nodes: Vec<ThemeNodeState { node, name, hash, origin, error, fallback }> }` (node states from `nodes.meta.theme`, on the active entry only).
- `POST /themes` (`Content-Type: application/x-tar`, body ≤ `MAX_BUNDLE_BYTES`, route-level body limit; `?activate=true` optional) → 201 `ThemeDto`; a theme that does not load → 422 with one `errors[]` entry per template problem (`templates/<file>:<line>: …`).
- `POST /themes/{name}/activate` → 200 `ThemeDto`: a row → `Installer::theme_from_artifact` first (a failure → 422, the active theme unchanged, no event); an embedded theme without a row → `stage_embedded` + install + activate; unknown → 404.
- Both writes go through `ExtensionAdmin` (`authorize_themes` before the body is read).

**Tests** (both profiles): `theme_upload_and_activate` (a `pack`ed fixture folder → 201; activate → `theme.changed`, `/` renders the new theme within one poll on the test node); `activation_refuses_broken_theme` (Review Focus 5: a row whose artifact was tampered → 422, `/` still renders the old theme, no `theme.changed` row); `embedded_theme_activates_without_a_row` (after activating a custom theme, `POST /themes/blog/activate` → 200, a `blog` row exists); `broken_upload_lists_template_errors`; `theme_list_shows_node_state` (`fallback` false; after a startup with a tampered active artifact on the serve harness, `fallback` true); `editor_cannot_manage_themes`.

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-http --test api themes` — expect FAIL.
- [ ] **Step 3:** Implement; regenerate the snapshot.
- [ ] **Step 4:** Re-run with the sweep — expect PASS on both profiles.
- [ ] **Step 5:** Commit `feat(api): themes list, upload and activation that refuses broken themes`.

### Task 6: Theme and plugin settings, sealed secrets

**Files:**
- Create: `crates/ferrite-core/src/ext_settings.rs`, `crates/ferrite-core/tests/ext_settings.rs`, `crates/ferrite-http/tests/api/settings_ext.rs`
- Modify: `crates/ferrite-core/src/{lib.rs,error.rs,extensions/admin.rs}`, `crates/ferrite-http/src/api/{plugins.rs,themes.rs,problem.rs,dto/extensions.rs}`, `crates/ferrite-http/src/extensions.rs` (schema conversions), `crates/ferrite-http/src/plugins/services.rs`, `crates/ferrite-plugin-host/src/{state.rs,call.rs,registry.rs}`, `crates/ferrite/src/serve.rs`, `crates/ferrite-http/openapi.json`

**Interfaces — Produces:**
- `ferrite_core::ext_settings`: `SettingType { Text, Textarea, Bool, Number, Select, Color, Media, Secret }` (serde lowercase), `SettingDef { key: String, kind: SettingType, label: String, default: Option<Value>, options: Vec<String> }`, `SettingsSchema(Vec<SettingDef>)` (`Serialize`/`Deserialize`, stored in the theme row's `manifest.settings`); `validate_value(def, &Value) -> Result<(), CoreError>` — text ≤ 1 000 chars, textarea ≤ 65 536, secret 1..=4 096 chars, number a finite JSON number, bool, select one of the options, color `#rgb`/`#rrggbb`, media a UUID string; no NUL anywhere. Errors are a new `CoreError::InvalidKey { key: String, message: String }` (the key is data, so `InvalidField`'s `&'static str` does not fit), mapped in `problem.rs` to `422 validation` with `errors[0].field = key` (add it to `problem_mapping_table`).
- `SECRET_PURPOSE` = `"plugin-setting:<plugin>:<key>"`; a stored secret option value is `{"sealed": "v1.…"}`.
- `ExtensionAdmin` (now `new(store, key: SiteKey)`): `theme_settings(&self, actor, name, schema) -> Map<String, Value>` (`require_reader(ManageThemes)`; values as templates see them, `ThemeManifest::settings_values` semantics); `set_theme_settings(&self, actor, name, schema, patch: Map<String, Value>) -> Map` (`require(ManageThemes)`; `null` resets a key; unknown key → `Invalid`; `secret` kind refused for themes; writes option `theme.<name>.settings` as one object, `options.changed`, audit `theme.settings {keys}`); `plugin_settings(&self, actor, name) -> PluginSettingsView { values: Map /* secrets omitted */, secrets: BTreeMap<String, bool /* set */> }` and `set_plugin_settings(&self, actor, name, patch)` — schema read from the plugin row's manifest `settings` table; one option per key `plugin.<name>.settings.<key>` (secrets sealed with `auth::seal(key, purpose, …)`), `options.changed` per key, audit `plugin.settings {keys}` (never values).
- HTTP: `GET/PATCH /themes/{name}/settings` (`ThemeSettingsDto { schema: Vec<SettingDefDto>, values }`; schema from the active theme in `AppState`, the row's `manifest.settings`, or the embedded theme), `GET/PATCH /plugins/{name}/settings` (`PluginSettingsDto { schema, values, secrets }`); PATCH body `{ key: value | null }` → the full DTO.
- Plugin host: `settings.get(key)` returns `None` for keys not declared in the manifest's `[settings]`, the declared `default` (JSON text) when the service has no value, else the service's value. `StoreHostServices::with_site_key(self, SiteKey)`; `setting_get` opens `{"sealed": …}` values with the plugin's purpose (failure: logged, `None`).

**Tests:** core (both backends): `setting_values_validated_per_kind` (one case per rule); `theme_settings_roundtrip_and_reset`; `secrets_are_write_only_and_owner_scoped` (Review Focus 3: PATCH a secret → GET shows `secrets.key = true` and no value; the stored option starts `{"sealed":"v1.`; `options.get` from a plugin granted `options.read = ["*"]` returns `None` for it; another plugin's `settings.get` of the same key name reads its own namespace only; opening under another purpose fails). Plugin host: `settings_get_defaults_and_undeclared`. HTTP: `theme_settings_change_shows_on_page` (accent colour PATCH → `/` reflects it within one poll; `option:theme.blog.settings` invalidation), `plugin_settings_over_http`, `editor_cannot_patch_extension_settings`.

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-core --test ext_settings`, `cargo test -p ferrite-plugin-host settings_get`, `cargo test -p ferrite-http --test api settings_ext` — expect FAIL.
- [ ] **Step 3:** Implement; write `"settings"` in `stage_theme`; regenerate the snapshot.
- [ ] **Step 4:** Re-run — expect PASS on both backends.
- [ ] **Step 5:** Commit `feat: theme and plugin settings with sealed plugin secrets`.

### Task 7: `ferrite theme …` and `ferrite plugin …` CLI

**Files:**
- Create: `crates/ferrite/src/ext_cmd.rs`
- Modify: `crates/ferrite/src/{cli.rs,lib.rs}`, `crates/ferrite/tests/cli.rs`, `docs/design/09-operations.md` §8

**Interfaces — Produces** (every subcommand takes `--data DIR`, opens the store and media and runs the startup migration check like `user`; operator path, P26):
- `ferrite theme install --dir PATH [--activate]`, `ferrite theme activate NAME` (same refusal as the API: loads the artifact first; embedded names install from the embedded files), `ferrite theme list` (`<name>  <version>  <installed|embedded>  [active]`).
- `ferrite plugin check --dir PATH` (manifest + inspection on a scratch host; prints the report; nothing stored; exit 1 on problems), `ferrite plugin install --dir PATH [--enable] [--yes]` (`PATH` holds `ferrite-plugin.toml` and one `*.wasm`; prints the report, high-risk lines prefixed `!`; without `--yes` asks `install? [y/N]` on a terminal and refuses when stdin is not one), `ferrite plugin list` (`<name>  <version>  <enabled|disabled>  [<reason>]`), `enable NAME`, `disable NAME`, `remove NAME [--purge]`.

**Tests** (`tests/cli.rs`, SQLite): `plugin_check_prints_report`, `plugin_install_needs_yes_without_tty`, `plugin_install_enable_list_remove`, `plugin_remove_keeps_data_without_purge`, `theme_install_activate_list`, `theme_activate_refuses_broken` (tampered artifact → exit 1, active theme unchanged).

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite --test cli plugin_ theme_` — expect FAIL.
- [ ] **Step 3:** Implement; document in 09 §8.
- [ ] **Step 4:** Re-run — expect PASS.
- [ ] **Step 5:** Commit `feat(cli): ferrite theme and ferrite plugin commands`.

### Task 8: Plugin routes under `/_p/`

**Files:**
- Create: `crates/ferrite-http/src/app/plugin_route.rs`, `crates/ferrite-http/tests/app/plugin_route.rs`, `plugins/examples/router/`, `plugins/examples/hostile/route-xss/`, `plugins/examples/hostile/route-flood/`, `crates/ferrite/tests/cluster/routes.rs`
- Modify: `crates/ferrite-http/src/app.rs` (`build_site`/`build_app` mount), `crates/ferrite-http/src/app/state.rs`, `crates/ferrite-core/src/auth/{service.rs,limits.rs,key.rs}`, `crates/ferrite-plugin-host/src/{lib.rs,validate.rs,call.rs}`, `crates/ferrite-plugin-host/tests/hello.rs`, `crates/ferrite-theme/src/render.rs` (`plugin_route` helper), `crates/ferrite/tests/cluster/{main.rs,harness.rs}`, `plugins/Cargo.toml`, `docs/design/05-plugin-host.md` §4b, §12, `docs/design/06-themes-rendering.md` §4

**Interfaces — Produces:**
- Core: `AuthService::hit_limit(&self, scope: &str, ip: Option<IpAddr>, max: u32, window: Duration) -> Result<(), CoreError>` — bucket `<scope>:ip:<limit_ip_hash>` (`:ip:none` without an address), outside any `Tx` (R6), `RateLimited` with `Retry-After` clamped to `1..=window`; `limits::hit` takes the window. `AuthService::plugin_ip_hash(&self, plugin: &str, ip: IpAddr) -> String` = 32 hex of HMAC(subkey `"ferrite:plugin-ip:v1"`, `plugin || 0x00 || canonical ip`).
- Plugin host: `handle_route` validates the response (`validate_route_response`): status 200..=599, ≤ 32 headers, each name/value ≤ 4 KiB and valid header text, body ≤ 1 MiB; else `CallFailed` recorded as `InvalidOutput`.
- HTTP: `/_p/{plugin}/{*path}` for every method, mounted before the public fallback in `build_site` and `build_app`. Order: unknown plugin, disabled plugin, `routes` not granted, path or method not in `[routes]` → plain `404` (`405` with `Allow` when only the method differs); non-GET/HEAD without the site origin (the API's `same_origin` rule, made `pub(crate)`) unless `cross_origin = true` → `403`; rate limit `hit_limit("route:<plugin>:<path>", ip, n, per)` from `rate_limit` (default `10/min`) → `429` + `Retry-After`; body > 1 MiB → `413`; then `RouteRequest { method, path, query (≤ 100 pairs), headers (only content-type, accept, accept-language, user-agent, referer), body, client_ip_hash: plugin_ip_hash, user: session cookie's user id (no bearer) }`. A call failure or timeout → `502` plain text. Response: the plugin's status, only its `Content-Type`, `Content-Language` and (3xx only) a root-relative `Location` (`/…`, not `//…`; else the response becomes `502`); always `Content-Security-Policy: sandbox allow-forms`, `X-Content-Type-Options: nosniff`, `Cache-Control: no-store`, `X-Robots-Tag: noindex`; never `Set-Cookie`. Never cached.
- Theme helper `plugin_route(name, path) -> String` = `/_p/<name><path>` (name `[a-z0-9-]{2,48}`, path starting with `/`, segments percent-encoded; anything else `""`).
- Example plugins: `router` (`routes = true`; `GET /hello` → JSON `{"user": <user or null>, "ip": <hash>}`, `POST /echo` → echoes the body, `GET /go` → `303 Location: /thanks/`, `GET /away` → `303 Location: https://evil.example/`, `GET /big` → a 2 MiB body); hostile `route-xss` (`GET /x` → `text/html` with `<script>document.cookie</script>` plus a `Set-Cookie` and an `Authorization` response header); hostile `route-flood` (`POST /submit`, `rate_limit = "5/min"`, `cross_origin = true`).

**Tests** (`tests/app/plugin_route.rs`, both profiles; plugin-host `hello.rs` success paths): `handle_route_success_path` (host level: `router` answers `/hello`); `route_success_and_user_id` (anonymous → `"user": null`; with a session cookie → the user id; with `Authorization: Bearer` only → null); `route_xss_is_contained` (Review Focus 2: CSP `sandbox allow-forms`, `nosniff`, no `Set-Cookie`, no `Authorization` header in the response; the request the plugin saw had no cookie header); `undeclared_path_and_method` (404, 405 with `Allow`); `post_needs_site_origin` (`router` `POST /echo` with a foreign `Origin` → 403; with the site origin → 200); `cross_site_location_refused` (`/away` → 502; `/go` → 303 `/thanks/`); `route_response_caps` (a 2 MiB body from `router`'s `GET /big` → 502); `body_over_1_mib_is_413`; cluster `route_flood_is_limited_across_nodes` (Review Focus 2, `crates/ferrite/tests/cluster/routes.rs`, Postgres: 6 POSTs alternating between two nodes from one client IP → the 6th is 429; a second IP still passes); theme `plugin_route_helper` (unit).

- [ ] **Step 1:** Write the example plugins and the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-http --test app plugin_route` — expect FAIL.
- [ ] **Step 3:** Implement core limits, host validation, the HTTP route and the helper; update 05 §4b/§12 and 06 §4.
- [ ] **Step 4:** Run `cargo test -p ferrite-http --test app`, `cargo test -p ferrite-plugin-host`, `cargo test -p ferrite --test cluster routes` with Postgres — expect PASS.
- [ ] **Step 5:** Commit `feat(http): plugin routes under /_p/ with sandboxing, origin checks and shared rate limits`.

### Task 9: `[mail]`, the SMTP mailer and `notify-admins`

**Files:**
- Create: `crates/ferrite-core/src/mail.rs`, `crates/ferrite-http/src/mail.rs`, `plugins/examples/notifier/`, `plugins/examples/hostile/mail-flood/`, `crates/ferrite-http/tests/plugins/mail.rs`
- Modify: `Cargo.toml` (lettre), `crates/ferrite-http/Cargo.toml`, `crates/ferrite-config/src/{model.rs,load.rs,tests.rs}` + init template, `crates/ferrite-store/src/repos.rs` (`UserRepo::list_active_admins`), both backends' `repos/user.rs`, `crates/ferrite-store-tests/src/user.rs`, `crates/ferrite-plugin-host/src/{services.rs,state.rs}`, `crates/ferrite-http/src/plugins/services.rs`, `crates/ferrite/src/serve.rs`, `docs/design/01-architecture.md` §4

**Interfaces — Produces:**
- Config `[mail]`: `smtp_url` / `smtp_url_file` (secret rules of 01 §4; schemes `smtp`, `smtps`; validated with lettre's parser at load, message never echoes the URL) and `from` (a mailbox, required when `smtp_url` is set) → `Config.mail: Option<MailConfig { smtp_url: Secret, from: String }>`.
- Core `mail`: `OutgoingMail { to: Vec<String>, subject: String, text: String }`; `#[async_trait] trait Mailer: Send + Sync { async fn send(&self, mail: OutgoingMail) -> Result<(), MailError>; }`; `MailError(String)` (`thiserror`); `MemoryMailer` (records sent mail; `sent() -> Vec<OutgoingMail>`; used by tests in every crate).
- `UserRepo::list_active_admins(&self, limit: u32) -> Result<Vec<User>, StoreError>` (`role = 'admin' AND status = 'active'`, by login, `limit` 1..=`MAX_AUTH_LIST`).
- `ferrite_http::mail::SmtpMailer::new(&MailConfig) -> Result<SmtpMailer, MailError>` (lettre `AsyncSmtpTransport<Tokio1Executor>::from_url`, timeout 10 s, `from` parsed once).
- Plugin host: `HostServices::notify_admins(&self, plugin: &str, subject: &str, text: &str) -> Result<(), String>` (default: `Err("mail is not configured")`); the `mail` import refuses during render filters (R20 text "mail is not allowed during render filters"), allows one mail per call, subject 1..=200 chars without control characters, body ≤ 64 KiB; `InMemoryServices` records.
- `StoreHostServices::with_mailer(self, Arc<dyn Mailer>, site_title: String)`: per-plugin limit through `RateLimitRepo::hit("mail:plugin:<name>", 1 h)` outside any `Tx`, refused once the count is over 10, recipients `list_active_admins(50)` e-mails, subject `"[<site title>] <subject>"`, body + `"\n\n-- \nSent by plugin <name>"`.
- Example `notifier` (`mail = "admins"`, action `post.published` sends one mail); hostile `mail-flood` (`post.saved` tries 50 sends in one call and is called repeatedly).

**Tests:** config `mail_section_rules` (bad scheme, missing `from`, `_file` works, URL not echoed); store `list_active_admins_filters` (both backends); http `smtp_mailer_sends_to_a_fake_server` (a ~40-line in-test SMTP sink on 127.0.0.1 accepts `EHLO/MAIL/RCPT/DATA/QUIT`; `smtp://127.0.0.1:<port>`; the received DATA has the subject); plugins `notify_admins_reaches_admins_only` (two admins, one editor, one disabled admin → two recipients; `MemoryMailer`); `mail_flood_is_capped` (Review Focus 3: across repeated calls exactly 10 mails in the hour, every further call gets an error; the second send inside one call fails); `mail_refused_on_render_path`; `mail_unconfigured_is_an_error_to_the_guest`.

- [ ] **Step 1:** Write the plugins and the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-config mail`, `cargo test -p ferrite-store-tests --test contract list_active_admins`, `cargo test -p ferrite-http --test plugins mail` — expect FAIL.
- [ ] **Step 3:** Implement config, core trait, store method, mailer and the import; wire `serve` (`SmtpMailer` when `[mail]` is set).
- [ ] **Step 4:** Re-run with Postgres — expect PASS.
- [ ] **Step 5:** Commit `feat: [mail] config, SMTP mailer and the notify-admins import`.

### Task 10: Password reset by e-mail

**Files:**
- Create: `crates/ferrite-core/src/auth/reset.rs`, `crates/ferrite-http/tests/api/reset.rs`
- Modify: `crates/ferrite-core/src/auth/{mod.rs,service.rs}`, `crates/ferrite-core/tests/auth.rs`, `crates/ferrite-http/src/api/{auth.rs,state.rs,problem.rs,dto/auth.rs,mod.rs}`, `crates/ferrite-http/tests/api/sweep.rs`, `crates/ferrite-http/openapi.json`, `docs/design/07-admin-api.md` §1–§2

**Interfaces — Produces:**
- `AuthService::request_password_reset(&self, mailer: Arc<dyn Mailer>, input: &str /* login or e-mail */, ip: Option<IpAddr>, reset_base: &str /* "<site.url>/admin/reset-password" */, site_title: &str) -> Result<(), CoreError>` — limits first (`reset:ip` 5/min, `reset:user:<keyed hash of the trimmed, ASCII-lowercased input>` 3/h, label `"ferrite:reset-bucket:v1"`); lookup as login (e-mail when it contains `@`); an active user with an e-mail gets, **in a spawned task** (the response does not wait for SMTP), a mail with `<reset_base>#token=<token>`; unknown, disabled or mail failure: nothing visible (logged). Always `Ok(())` unless limited.
- Token = `seal(key, "password-reset", {user, exp: now + 1 h, fp})`, `fp` = 32 hex of HMAC(subkey `"ferrite:reset-pw:v1"`, current `password_hash` or `""`).
- `AuthService::confirm_password_reset(&self, users: &UserService, token: &str, new: &str) -> Result<(), CoreError>` — `password::check_policy` first; token opened, unexpired, user active, `fp` equal to the current hash's (else `invalid_field("token", "the reset link is invalid or has expired")`); then the operator `set_password(login, new, must_reset: false)` (ends every session, keeps API tokens per R22, TOTP untouched); audit `user.password_reset`.
- HTTP (both `SameOrigin`, like login): `POST /auth/password-reset {login}` → `202` (empty body) or `429`; `503 mail-unavailable` when no mailer is configured; `POST /auth/password-reset/confirm {token, password}` → `204`. `ApiState` gains `mailer: Option<Arc<dyn Mailer>>` and `site_title`. Sweep `UNAUTHENTICATED` gains both paths.

**Tests:** core (both backends, controllable clock): `reset_does_not_enumerate` (Review Focus 4: known, unknown and disabled inputs all `Ok`; only the known active user gets mail); `reset_token_single_use_and_expiry` (confirm works once; the same token again → `InvalidField(token)`; clock + 61 min → refused; a password change via `change_password` before use kills it); `reset_ends_sessions_keeps_tokens`; `reset_keeps_totp` (a TOTP user still gets `TotpRequired` after resetting); `reset_rate_limits` (6th request per IP in a minute → `RateLimited`; 4th per account in an hour). HTTP: `password_reset_over_http` (202; the `MemoryMailer` link's fragment token confirms → 204; login with the new password), `reset_checks_origin`, `reset_without_mailer_is_503`.

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-core --test auth reset_` and `cargo test -p ferrite-http --test api reset` — expect FAIL.
- [ ] **Step 3:** Implement; extend the sweep exemptions; regenerate the snapshot.
- [ ] **Step 4:** Re-run plus the sweep — expect PASS on both backends.
- [ ] **Step 5:** Commit `feat(auth): password reset by e-mail with single-use sealed tokens`.

### Task 11: `plugin.action` jobs for `post.saved`, `post.published`, `user.created`

**Files:**
- Create: `crates/ferrite-core/src/actions.rs`, `crates/ferrite-core/tests/actions.rs`, `crates/ferrite/tests/node_actions.rs` (or a module in `tests/node.rs`)
- Modify: `crates/ferrite-core/src/{lib.rs,jobs.rs,content.rs,content/lifecycle.rs,users.rs}`, `crates/ferrite-plugin-host/src/lib.rs` (`subscribers`), `crates/ferrite-plugin-host/tests/hello.rs`, `crates/ferrite/src/{jobs.rs,serve.rs}`, `plugins/examples/meta-writer/ferrite-plugin.toml`, `docs/design/04-cluster.md` §6, `docs/design/05-plugin-host.md` §4

**Interfaces — Produces:**
- `ferrite_core::jobs::{PLUGIN_ACTION_KIND = "plugin.action", PLUGIN_ACTION_VERSION = 1, PluginAction { v, plugin: String, hook: String, subject: Option<String>, payload: Value }, plugin_action_job(&PluginAction) -> NewJob}` (no unique key; `max_attempts` default).
- `ferrite_core::actions::enqueue(repos: &dyn Repos, hook: &str, subject: Option<String>, payload: Value) -> Result<u32, CoreError>` — reads `repos.plugins().list()`, keeps enabled rows whose `manifest.hooks` has `hook`, enqueues one job each (name order); returns the count. Called inside the write `Tx` by: `ContentService` `after_save` (`post.saved`, subject = item id, payload `{"post": id}`), every transition to `published` (`transition_in` Publish, `publish_one`: `post.published`, same payload), `UserService::create` and `OperatorUsers::create` (`user.created`, subject = user id, payload `{"user": id}`).
- `PluginHost::subscribers(&self, hook: &str) -> Vec<String>` (enabled loaded plugins subscribed, priority order).
- Binary `PluginActionHandler { store: Arc<dyn Store>, host: Arc<PluginHost> }` (kind `plugin.action`, versions `[1]`): `dispatch_action` (one registry load per call; the loaded instance is held for the whole call, R13). Outcomes: `Ok` → done; `UnknownPlugin`/`Disabled` with the row missing or disabled → done (logged, dropped); `UnknownPlugin` with the row enabled → `Retry("not loaded on this node yet")`; `NotSubscribed` → `Permanent`; `CallFailed` → `Retry`.
- `meta-writer` fixture gains `content.read = ["post"]` (needed by Task 13's meta scoping).

**Tests:** core (both backends): `actions_enqueued_in_the_write_tx` (an enabled subscriber and a disabled one: create → one `post.saved` job; publish → one `post.published`; a rolled-back write (version conflict) enqueues nothing); `user_created_enqueued`. Plugin host: `dispatch_action_success_path` (meta-writer's `post.saved` writes meta through `InMemoryServices`). Binary (both backends): `post_saved_runs_meta_writer` (save → the job runs on the runner → `seo.seen = "action"` on the item); `action_survives_hot_swap` (a reload of the plugin while a slow action (`slow-import` fixture subscribed to `post.saved`) runs: the call completes on the old instance, R13); `action_for_removed_plugin_is_dropped` (job completes, no retry).

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-core --test actions`, `cargo test -p ferrite-plugin-host --test hello dispatch_action_success`, `cargo test -p ferrite --test node post_saved` — expect FAIL.
- [ ] **Step 3:** Implement; register the handler in `serve`.
- [ ] **Step 4:** Re-run on both backends — expect PASS.
- [ ] **Step 5:** Commit `feat: plugin.action jobs for post and user action hooks`.

### Task 12: Batched `request.served`

**Files:**
- Create: `crates/ferrite-http/src/plugins/served.rs`, `plugins/examples/counter/`, `crates/ferrite-http/tests/app/served.rs`
- Modify: `crates/ferrite-http/src/plugins.rs`, `crates/ferrite-http/src/app/{handler.rs,state.rs}`, `crates/ferrite-http/src/lib.rs`, `crates/ferrite/src/serve.rs`, `docs/design/05-plugin-host.md` §4a

**Interfaces — Produces:**
- `ServedEvent { path: String /* no query */, status: u16, ts: Timestamp, referrer_host: Option<String>, ua_class: UaClass /* browser | bot | other */, cache: CacheOutcome /* hit | miss | bypass */ }`; `ua_class(&str) -> UaClass` (contains `bot`, `crawl`, `spider` or `slurp`, ASCII case-insensitive → bot; starts with `Mozilla/` → browser; else other).
- `ServedBuffer::new(store: Arc<dyn Store>, host: Arc<PluginHost>)`; `record(&self, ServedEvent)` (non-blocking; beyond 10 000 buffered events new ones are dropped and counted); `spawn_flusher(self: Arc<Self>, every: Duration /* 10 s */, stop: CancellationToken) -> JoinHandle<()>` — flushes when 1 000 are buffered or every 10 s, and once on stop: one `plugin.action` job per `host.subscribers("request.served")` with payload `{"events": [...]}`, subject `None`; nothing is enqueued without subscribers or events.
- `AppState::with_served(self, Arc<ServedBuffer>) -> Self`; the public handler records every page response (content pages, archives, feeds, 404/410 pages; not health, assets, previews, `/_p/`), cache outcome from whether the render closure ran (`hit` when it did not, `bypass` for logged-in bypass).
- Example `counter` subscribes to `request.served` and stores the event count in `kv`.

**Tests** (both profiles): `served_events_batched_into_one_job` (5 requests, two of them cache hits → after a manual flush exactly one `plugin.action` job for `counter` with 5 events, `cache` values `miss`/`hit`, no IP, cookie or full UA anywhere in the payload); `flush_at_1000_events`; `no_subscriber_no_job`; `buffer_drops_beyond_cap`; unit `ua_classes`; end to end through the runner: `counter`'s kv count equals the requests made.

- [ ] **Step 1:** Write the plugin and the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-http --test app served` — expect FAIL.
- [ ] **Step 3:** Implement; wire the flusher into `serve`'s background tasks (final flush within the shutdown bound).
- [ ] **Step 4:** Re-run — expect PASS on both profiles.
- [ ] **Step 5:** Commit `feat(http): batched request.served events as plugin.action jobs`.

### Task 13: Plugin-host hardening — call cap, render budget, meta scoping, cancel-safety

**Files:**
- Modify: `crates/ferrite-plugin-host/src/{config.rs,lib.rs,call.rs,state.rs,services.rs,error.rs}`, `crates/ferrite-plugin-host/tests/{limits.rs,capabilities.rs}`, `crates/ferrite-http/src/plugins/{apply.rs,services.rs}`, `crates/ferrite-http/src/app/{pipeline.rs,state.rs}`, `crates/ferrite-config/src/{model.rs,load.rs,tests.rs}`, `crates/ferrite/src/serve.rs`, `docs/design/05-plugin-host.md` §6, §7a, §9

**Interfaces — Produces:**
- `HostConfig.max_concurrent_calls: usize` (config `plugins.max_concurrent_calls`, default `max(8, 4 × available_parallelism)`, > 0): one `Semaphore` per host; every filter, action and route call acquires a permit within its deadline; none in time → the call is skipped, `PluginError::Busy` for actions and routes (route → `503` + `Retry-After: 1`, action → `Retry`), a filter is skipped and marks the page volatile. A `Busy` skip is **not** counted as a plugin failure.
- `PluginHost::apply_filter_until(&self, hook, value, req, until: Instant) -> FilterOutcome` — each call's deadline is `min(hook deadline, until − now)`; once `until` has passed the remaining plugins are skipped and the outcome is volatile; `apply_filter_traced` = `until` far in the future.
- `HttpConfig.render_budget: Duration` (config `plugins.render_budget_ms`, default 1 000): `PluginRequest` carries the page's deadline (set when the render slot is taken); `run_filter` calls `apply_filter_until`.
- `meta` scoping (ROADMAP follow-up): `meta.get` and `meta.set` act only on items whose type `content.read` grants (`meta.set` also loads the item's type through a new `HostServices::post_kind(&self, id: &str) -> Option<String>` that sees every status); a `"*"` meta pattern never matches keys `plugin.<other>.*`.
- `HostServices` docs: futures are dropped at the hard timeout (cancel-safety: implementations must not hold a `Tx` across an await they cannot finish, and writes are single statements or short `Tx`s).

**Tests:** `concurrency_cap_skips_without_failure` (cap 2, three concurrent `slow-import` actions → one `Busy`, failure counter unchanged); `render_budget_bounds_a_page` (budget 300 ms, three `slow-import` filters on one page → the page answers within ~500 ms, volatile, later filters skipped); `meta_set_scoped_to_read_types` (meta-writer on a `page` item → refused; on a `post` → ok); `star_meta_grant_excludes_other_plugins_namespace`; config `plugins_limits_rules`.

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-plugin-host --test limits` and `cargo test -p ferrite-http --test plugins` — expect FAIL.
- [ ] **Step 3:** Implement; wire config.
- [ ] **Step 4:** Run `cargo test --workspace` with Postgres — expect PASS (all hostile tests included).
- [ ] **Step 5:** Commit `feat(plugin-host): call concurrency cap, per-render budget, type-scoped meta`.

### Task 14: Docs, ROADMAP and exit check

**Files:**
- Create: `docs/superpowers/m2a2a-exit.md`
- Modify: `docs/design/07-admin-api.md` (§1 table: M2a-2a rows built, core checks column; §2 password reset as built, the e-mail item), `docs/design/05-plugin-host.md` (§2 install/approval/update/uninstall as built, §3 limits ceilings and bare-IP rule, §4a–§4c, §6 cap and budget, §8, §12 new hostile plugins), `docs/design/06-themes-rendering.md` §1, §4, §7, `docs/design/04-cluster.md` §6 (`plugin.action`, `plugin.purge`), `docs/design/01-architecture.md` §4 (`[mail]`, new `[plugins]` keys), §8, `docs/design/09-operations.md` §8, `docs/ROADMAP.md` (split M2a-2 into M2a-2a (ticked) and M2a-2b; tick R11, the R21 item, plugin CLI, themes API/CLI, routes, settings, mail, reset, `plugin.action`, `request.served`, bare IPs, meta scoping, limits approval, cap, budget, success-path tests, cancel-safety docs, `install` off the runtime; move the option cache to M4), `CLAUDE.md` (status and crate map)

**Deliverables:** `m2a2a-exit.md` with transcripts: workspace tests on both backends; a manual session against `ferrite serve` (solo): `plugin inspect` → install with the fingerprint → enable → `/_p/router/hello` → settings PATCH → theme upload and activation → password reset with a local SMTP sink.

- [ ] **Step 1:** Update the docs listed above.
- [ ] **Step 2:** Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `CI=true FERRITE_TEST_PG_URL=… cargo test --workspace` — expect all green.
- [ ] **Step 3:** Run the manual session and write `m2a2a-exit.md`.
- [ ] **Step 4:** Commit `docs: M2a-2a as built, roadmap and exit check`.

---

## Plan rulings

- **P1 — M2a-2 is split in two.** M2a-2a (this plan, 14 tasks: extensions and the plugin platform) and M2a-2b (outlined below: media, comments, system API, hardening, the remaining deferred and parked items). One plan would be ~26 tasks; the extension and plugin items share the install flow, grants and host changes, while media/comments/system share uploads and the new migration. Cost: one more plan/review cycle.
- **P2 — No migration in M2a-2a.** Grants with approved limits live in `plugins.granted` (versioned JSON, the bare M1c-2 form still read), settings in `options`, limits in `rate_limits`, actions and purges in `jobs`. `0006` (same name on both backends, `.down.sql`, expand-only) is M2a-2b's (idempotency keys, the sanitised-SVG marker). Cost: none.
- **P3 — Approval is a fingerprint of what the admin saw.** `POST /plugins/inspect` returns the report and `fingerprint` = SHA-256 over plugin, version, artifact hash and the grant; `POST /plugins` must carry it. The stored `granted` is always an object (closes M1c-2 R21's open item). A grant cannot be approved for different bytes. Cost: the upload is sent twice (inspect, install).
- **P4 — Widening updates need the new approval in the same request; nothing is stored unapproved.** Design 05 §2's "stored but not activated" needed a pending-version column and a second approval endpoint; with P3 every install shows the diff (`report.added`) and an update without the matching fingerprint is refused while the old version keeps running. Cost: no "approve later" queue.
- **P5 — Limits above host defaults are approved per plugin, up to ceilings.** Memory ≤ 256 MiB, fuel ≤ 10 × default; above the default needs approval (high-risk report line), stored in `granted.limits`, applied by `prepare_with`; above a ceiling is refused at inspection (closes M2a P28). Cost: worst-case memory per call grows for approved plugins (still capped by P22's call count).
- **P6 — The permission report lives in `ferrite-plugin-host`** next to the manifest it describes; plain-language lines and high-risk flags as listed in Task 2. Cost: report wording changes need a host release.
- **P7 — Bare IP addresses and `localhost` are refused in `capabilities.http`.** The post-resolution SSRF check comes with the real `http` import (M2a-2b). Cost: a plugin cannot target an IP-only API.
- **P8 — The install pipeline moves to `ferrite_http::extensions::Installer`** (library, `thiserror`), used by the API and the CLI; `ExtensionService` stays the system path and `ExtensionAdmin` is its actor-checked facade with audit rows (closes M1.5 R11). Compiles run on the blocking pool, two at a time per node, on one scratch host that never commits. Cost: none.
- **P9 — Plugin delete keeps KV and settings 30 days.** The row goes in one `Tx` with `plugins.changed` and a `plugin.purge` job due in 30 days (keyed by deletion time; a reinstall in between keeps the data); `purge=true` deletes at once. No retention table. Cost: a reinstalled plugin sees its old data (WordPress-like).
- **P10 — Extension reads need the role capability and `extensions:read`** (`require_reader`), since only admins may see installed plugins and their grants. Cost: editors see no plugin list.
- **P11 — Upload formats.** Themes: the raw deterministic ustar bundle (`application/x-tar`, P17 of M1.5) as the request body. Plugins: `multipart/form-data` (`manifest`, `component`, `approve`) through axum's `multipart` feature (multer 3.1.0, already maintained by the axum project); per-route `DefaultBodyLimit` (probe-verified). `Idempotency-Key` stays with media uploads (M2a-2b), where retries create rows. Cost: none.
- **P12 — Activation refuses a theme that does not load** (fetched, hash-checked, unpacked and compiled before the row changes); an embedded theme without a row is installed from the embedded files when activated; a startup fallback is flagged as `nodes.meta.theme.fallback` and shown by `GET /themes` (the system status of M2a-2b reads the same key). Cost: activation takes as long as one theme load.
- **P13 — One settings schema for themes and plugins** (`ferrite_core::ext_settings`, 05 §4c), validated in core against the schema the caller derives from the installed manifest (theme rows now store it; embedded themes from their files). Theme settings stay one object option, plugin settings one option per key (the layouts the readers already use). `secret` is plugin-only, sealed with the site key per plugin and key, write-only in the API, opened only by the owning plugin's `settings.get`; `settings.get` answers declared keys only and falls back to the declared default. Cost: a lost site key makes stored plugin secrets unreadable (admins re-enter them).
- **P14 — The per-node option cache is not reintroduced (to M4).** Theme settings and `site.*` are read on page-cache misses only and plugin settings per plugin call, each one primary-key read; a cache adds cross-node staleness (R13's refresh) for no measured gain. Cost: one indexed read per setting on misses.
- **P15 — Plugin routes.** `/_p/{plugin}/{*path}` is mounted before the public fallback for every method; exact path and method from `[routes]`; checks in the order of Task 8; request headers allowlisted, body ≤ 1 MiB; the response keeps only status, `Content-Type`, `Content-Language` and a root-relative `Location` on 3xx, and always gets `Content-Security-Policy: sandbox allow-forms`, `nosniff`, `Cache-Control: no-store`, `X-Robots-Tag: noindex`; never cached; never `Set-Cookie`. The visitor's user id comes from the session cookie only; `client-ip-hash` is per plugin (HMAC with the plugin name), so plugins cannot join visitors across each other. Cost: plugins cannot set caching headers or cookies (by design).
- **P16 — Route rate limits are DB-backed fixed windows** through `AuthService::hit_limit` (bucket per plugin, route and client /64), so they hold across nodes (05 §4b); the API's origin rule guards non-GET/HEAD unless `cross_origin`. Cost: one upsert per route request.
- **P17 — Mail.** `[mail] smtp_url[_file]` + `from`, lettre 0.11.23 (rustls/ring/webpki-roots, matching the binary's ring provider); a `Mailer` trait in core so services and tests use `MemoryMailer`; without `[mail]` the import errors and the reset endpoint is `503 mail-unavailable`. Cost: no other transports (sendmail, API providers) yet.
- **P18 — `notify-admins` bounds.** Active admins only (≤ 50), one mail per call, 10 per plugin per hour (shared bucket), subject ≤ 200 chars without control characters, body ≤ 64 KiB, a site-title prefix and a "sent by plugin" footer; refused on the render path (R20). Cost: chatty plugins hit the cap.
- **P19 — Password reset tokens are stateless and single-use.** Sealed `{user, exp 1 h, fingerprint of the current password hash}`, so any password change (including the reset) kills them; the request always answers `202` and mails from a spawned task so timing does not reveal accounts; limits 5/min per IP and 3/h per account input; the link puts the token in the URL fragment (`/admin/reset-password#token=…`) so it stays out of logs and `Referer`; the reset ends sessions, keeps API tokens (R22) and does not bypass TOTP. Cost: a mail failure is only logged (the user retries).
- **P20 — Action hooks are enqueued by core in the write transaction.** Subscribers are read from the enabled `plugins` rows' manifests inside the `Tx`, so an event is never lost or sent for a rolled-back write; one job per (plugin, event); the binary's handler dispatches through the node's host with one registry snapshot per call (R13 holds by construction). Cost: one `plugins` list per content save.
- **P21 — `request.served` is buffered per node** and flushed every 10 s or 1 000 events into one job per subscribed loaded plugin (05 §4a), including cache hits; no IP, cookie or full user agent; at most 10 000 events buffered; lost on a crash. Cost: analytics can lose up to 10 s per crash.
- **P22 — Plugin calls get a per-host concurrency cap and a per-render time budget** (closes the M0 review items). A call that cannot get a permit within its deadline is skipped without counting as a plugin failure (it is load, not misbehaviour); a page's filters share `plugins.render_budget_ms`. Cost: under load, filters are skipped (pages volatile) rather than queued.
- **P23 — `meta` is type-scoped** to the plugin's `content.read` types, and `"*"` meta grants never reach other plugins' `plugin.<other>.*` keys (closes the M0 follow-up). The `meta-writer` fixture declares `content.read = ["post"]`. Cost: a meta-only plugin must also declare the types it annotates.
- **P24 — New test plugins:** `router`, `notifier`, `counter`, `hello-v2`; hostile `route-xss`, `route-flood` (05 §12), `mail-flood`. Cost: longer plugin builds in tests.
- **P25 — Deferred to M2a-2b:** media service and uploads (`media.variants`, semaphore, `put_stream` limits, `Idempotency-Key`, SVG sanitising), comments, the system API (status, nodes, jobs list/retry, `GET /plugins/{name}/metrics`, metrics summary, audit list), the ammonia URL allowlist, the real `http` import, the `__Host-` cookie prefix, key-material zeroization, term archives for custom types, and the "Deferred from M1c-2" backend items. To M4: the option cache (P14). Cost: listed in the roadmap.
- **P26 — The extension CLI is an operator path** (M2a P17): no actor; `plugin install` prints the report and needs `--yes` or an interactive `y`, and approves exactly what it printed; SQLite commands fail while `serve` holds the data-folder lock. Cost: as `ferrite user`.
- **P27 — One new problem slug, `mail-unavailable` (503).** Approval mismatches are `422 validation` with `errors[0].field = "approve"`; `/_p/` errors are plain text because they are the plugin's surface, not the API's. Cost: none.
- **P28 — The sweep's unauthenticated list grows by the two password-reset endpoints only**; both check the origin like login (`SameOrigin`). Cost: none.
- **P29 — Actions on removed or disabled plugins are dropped, not retried**; a plugin not yet loaded on the claiming node (an enable racing the reload) is retried with the usual backoff. Cost: an action enqueued just before a disable is lost (intended).
- **P30 — Theme deletion is not built** (not in 07 §1's table); an installed theme row stays until M4's `artifacts.gc`/cleanup work. Cost: unused theme artifacts stay stored.

## Self-review notes

- Spec coverage (ROADMAP M2a-2 list): R11 → T1/T4/T5; install flow, grants, widening, limits approval, R21 → T2–T4; themes API + CLI, broken-theme refusal, theme settings, fallback flag → T3, T5–T7; plugin CLI → T7; routes and hostile `route-xss`/`route-flood` → T8; plugin settings and secrets → T6; `mail` + reset → T9–T10; `plugin.action` + R13 + success-path tests → T8 (routes), T11 (actions); `request.served` → T12; plugin-host follow-ups (bare IPs T2, meta scoping, semaphore, budget, cancel-safety T13, route caps T8, `install` off the runtime T3); option cache → P14. Everything else → M2a-2b (P25).
- Interfaces used across tasks are defined once: `ExtensionAdmin` (T1, extended in T6), `require_reader` (T1), `Grant`/`ApprovedLimits`/`PermissionReport`/`prepare_with` (T2), `Installer`/`ExtensionError` (T3), `SettingsSchema` and `CoreError::InvalidKey` (T6; T3's `stage_theme` writes `"settings": {}` until T6), `AuthService::hit_limit`/`plugin_ip_hash` (T8, reused by T9's per-plugin bucket through `RateLimitRepo::hit`), `Mailer`/`MemoryMailer` (T9, used by T10), `PLUGIN_ACTION_KIND`/`PluginHost::subscribers` (T11, used by T12).

---

## Next plan: M2a-2b — media, comments, system, hardening (outline)

Written as its own plan after M2a-2a merges (`docs/superpowers/plans/<date>-m2a2b-media-system.md`). Planned tasks (≈12):

1. **Migration `0006_uploads`** (both backends, same name, `.down.sql`, expand-only, `lossy-down`): `idempotency_keys (key_hash PK, user_id FK cascade, method, path, status, response json, created_at)` + prune in `sessions.prune` (24 h); `media.sanitized boolean NOT NULL DEFAULT false`; comment indexes for the moderation queue (`comments_status_created_idx`). Contract tests.
2. **Media service in core.** `MediaService` (`upload_media`; rows written with `post:<media id>` in the same `Tx`, M1c-2 R11), alt text / title updates, delete (trash then force), admin reads with `can_read`.
3. **`POST /media` streaming uploads** (multipart from M2a-2a, `put_stream` with `media.max_upload` (config, default 256 MiB, below the 5 GB copy ceiling), sniffed type, `Idempotency-Key`), `GET /media`, `GET/PATCH/DELETE /media/{id}`.
4. **`media.variants` job** with the image-processing semaphore (M1b R4; `process_image` on the blocking pool), core + active theme sizes, artifacts + `media_variants` rows.
5. **SVG sanitising on upload** over `quick-xml` 0.38 (workspace pin; no SVG-sanitiser crate on crates.io is maintained, `usvg` 0.48 re-renders and is heavy): strict element/attribute allowlist; no `script`, `foreignObject`, event handlers, `style` with `url(`, external `href`s, DOCTYPE or entities; sanitised SVG served inline with `Content-Security-Policy: sandbox` (03 §5), others stay attachments.
6. **Comments.** `CommentRepo` (table from 0001), `CommentService`, `GET /comments`, `PATCH /comments/{id}` (moderate), public `POST /content/{type}/{id}/comments` (origin-checked, honeypot field, per-IP rate limit, `pending` by default), comment settings in `settings::SETTINGS` (`comments.enabled`, `comments.moderation`); the sweep's unauthenticated list grows by that one path.
7. **System API (1).** `GET /system/status` (versions, schema, backends, nodes with `nodes.meta`, the theme fallback flag), `GET /system/audit` (`AuditRepo::list`).
8. **System API (2).** `JobRepo::list` (kind/status filters, cursor) + `retry` (dead → queued) on both backends, `GET /system/jobs`, `POST /system/jobs/{id}/retry` (queued kinds no node knows stay visible, M1.5 P5), `GET /plugins/{name}/metrics` (shared failure buckets + this node's counts), `GET /system/metrics/summary`.
9. **Real `http` import.** `reqwest = { version = "=0.13.5", default-features = false, features = ["rustls"] }` (already in the tree via object_store; probe-verified `ClientBuilder::dns_resolver(Arc<dyn Resolve>)`, `redirect::Policy::custom`, `https_only`): HTTPS only, exact allowlisted hosts, a resolver refusing non-global addresses (SSRF and DNS rebinding: connect to the checked IP), ≤ 3 redirects to allowlisted hosts, 5 s and 2 MiB per request, `User-Agent: ferrite/<ver> plugin/<name>`, no cookies.
10. **Hardening.** Ammonia URL allowlist for `html` blocks and plugin `html` (`http`, `https`, `mailto`, root-relative, `#`; drop scheme-relative `//x`, `ftp:`, `tel:`); `__Host-ferrite_session` on `https` sites (read both names during a transition); `zeroize` (1.9.1, already in the tree) for `SiteKey`, subkeys, HMAC key blocks and password copies.
11. **Deferred from M1c-2 + term archives.** Term archives for custom types (`SiteReader::term_archive_page`, R18); term rename and `permalink.*` changes record redirects; `seed demo` atomicity; bounded redirect hit counting; `date` filter timezone (`site.timezone`); theme `query(term=…)` limit reader; reject a child slug `page`; the reserved-slug legacy rename and the extra `types().list()`; `render.footer` wired; `cache.search_ttl` and `plugins.volatile_ttl`.
12. **Docs, ROADMAP and `m2a2b-exit.md`.**
