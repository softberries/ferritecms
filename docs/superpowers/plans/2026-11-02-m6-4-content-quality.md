# M6-4 — Accessibility and content health Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accessibility and link checks of the item being written (inline in the editor, in the side panel, as an MCP tool, and as an optional per-type publish gate), and a daily content health report of the whole site (broken internal and opt-in external links, orphans, missing alt text, redirect chains and loops, stale items) with an admin screen, a dashboard card and an MCP tool.

**Architecture:** One rule engine in `ferrite-core::quality` (pure document checks plus store-backed facts), called by the REST check endpoint the editor uses, by `ContentService`'s publish/schedule/update paths (the gate), by the MCP tools and by a `HealthRunner` that the `content.health` job drives. Internal paths are probed in `ferrite-http` through the router and the page pipeline's lookup step (no rendering); external links through an SSRF-checked `LinkChecker` on the egress client, only from the job. Results are rows (`0019_content_health`), so every node serves them.

**Tech Stack:** Rust 2024, axum 0.8, sqlx (SQLite + Postgres), reqwest (already in `ferrite-http`), `rmcp` 3.8, React/TanStack + TipTap/ProseMirror (already in `admin/`), Vitest, Playwright. No new dependencies (robots.txt is parsed in-house; the vague-phrase lists are data).

**Spec:** `docs/design/16-content-quality.md` (read it first; it wins over this plan on behaviour). Wave B: assumes M6-1 is merged (`ContentItem.locale`, `translation_group`, menu and term `locale`, `site.locales`) and M6-6 (outgoing webhooks off outbox events).

## Global Constraints

- Migration `0019_content_health` only, on **both** backends, `-- ferrite:phase expand` and `-- ferrite:lossy-down`, with `.down.sql`; contract tests on both (`FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test`, any Postgres ≥ 15).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`; `thiserror` in libraries, `anyhow` only in `crates/ferrite`; no `allow(dead_code)`; no `unwrap`/`expect` in non-test code.
- Capabilities are checked in core (`QualityService`, `ContentService`), never in handlers or tools.
- REST changes: DTOs with `utoipa` annotations; regenerate `crates/ferrite-http/openapi.json` with `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`, then `pnpm -C admin api:gen` (checked by `pnpm -C admin api:check`).
- Untrusted input everywhere (bodies, fields, Markdown, robots.txt, HTTP responses): iterative walks, the caps of design §4.3 and §6, hostile-input tests in every task that parses them.
- Constants (design §6): health daily at 04:00 UTC, job timeout 1 h, lock `content.health`; 200 items per page; caps 500 findings/item, 1 000 links/item, depth 64, 50 000 paths, 20 000 URLs, 100 000 findings/run; check endpoint ≤ 200 probed paths; external: 4 concurrent, 1 per host, 2 s per host, 10 s per URL, 1 000 URLs/run, 30 min/phase, `ok` cached 7 days, rows unseen 30 days pruned; runs kept 30 days.
- No public request runs a rule; the only render change is `alt=""` for decorative images.
- Admin: i18n keys in `admin/src/i18n/en/*.json` (new namespace `quality.json`); design rules of `docs/design/11-admin-ui.md` (tokens only, 44 px touch targets, axe clean, never colour alone).
- Keep to the files listed per task (other M6 parts merge in parallel); shared files touched: `crates/ferrite-store/src/repos.rs` (one trait + one accessor), both stores' `repos/mod.rs`, `catalogue.rs`, `ferrite-core/src/{lib.rs,error.rs,settings.rs,types.rs,content.rs,content/lifecycle.rs}`, `ferrite-http/src/api/{mod.rs,state.rs,problem.rs,types.rs}`, `mcp/server.rs`, `crates/ferrite/src/{serve.rs,jobs.rs}`, `admin/src/shell/nav.ts`.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- The gate cannot be bypassed by a person: REST publish/schedule, the admin's bulk publish, MCP `content_publish`/`content_schedule`, a live edit that adds an error (REST, MCP `content_update`, `restore_revision`) are all refused with `422 content-quality`; only the due-time scheduler, `publish_in` (seed, media) and imports skip it.
- An author cannot suppress an error on a gated type (needs `edit_others`); a suppressed finding never counts in the gate or the totals.
- The path probe never renders, never writes the page cache, never counts a page view, `request.served` or a redirect hit, and never calls a plugin.
- The link checker refuses private, loopback, link-local and unique-local addresses at resolve and connect time, on every redirect hop; non-default ports are not contacted; it is reachable only from the job (no `check` path holds one).
- A hostile body (depth 100 000, a million text nodes, wrong JSON types everywhere) is checked in bounded time without a panic; a 10 MiB robots.txt is cut at 64 KiB.
- Two nodes: one run per period; a manual run during a periodic one waits (`Busy`), never runs twice at once.

---

### Task 1: Rule catalogue and document checks (pure)

**Files:**
- Create: `crates/ferrite-core/src/quality/{mod.rs, rules.rs, walk.rs, check.rs, vague.rs}`
- Modify: `crates/ferrite-core/src/lib.rs` (`pub mod quality;`), `crates/ferrite-core/src/blocks/{mod.rs, anchor.rs}` (`pub fn heading_ids(doc: &Value) -> Vec<String>`, the ids `render_html_toc` gives, without rendering)

**Interfaces:**
- `pub enum Severity { Error, Warning, Notice }`, `pub enum Category { Accessibility, Links, Media, Site, Freshness }`, `pub enum RuleScope { Item, Site }` (each with `as_str`).
- `pub struct Rule { pub id: &'static str, pub severity: Severity, pub category: Category, pub wcag: &'static [&'static str], pub scope: RuleScope }`; `pub static RULES: &[Rule]` (design §3 table plus `check-truncated`, `findings-truncated`); `pub fn rule(id: &str) -> Option<&'static Rule>`.
- `pub enum Location { Title, Body { path: Vec<u32> }, Field { name: String, path: Vec<u32> }, FeaturedImage, Item, Redirect { id: RedirectId } }`.
- `pub struct Finding { pub rule: &'static Rule, pub location: Location, pub key: String, pub context: Option<String>, pub detail: Value }`.
- `pub struct DocInput<'a> { pub title: &'a str, pub body: &'a Value, pub fields: &'a Value, pub field_defs: &'a [FieldDef], pub locale: &'a str }`.
- `pub struct Links { pub internal: Vec<FoundLink>, pub external: Vec<FoundLink> }`, `pub struct FoundLink { pub href: String, pub text: String, pub location: Location }` (internal = local path or `site_origin`).
- `pub fn check_document(input: &DocInput, site_origin: &str) -> (Vec<Finding>, Links, Vec<MediaRef>)` — every item-scope rule that needs no store or probe; returns the media references (`ferrite:media/<id>` images and `featured_image`) for the facts step of Task 3.

- [ ] Write unit tests first: one positive and one negative case per rule of design §3 with scope `item` (except `media-missing`, `featured-alt-missing`, `link-internal-*`: Task 3/5), `heading-level-skipped` with the title as level 1, `decorative: true` skips `image-alt-missing`, alt of zero-width characters only is blank, link text joined across marks (`[bold "read", "more"]` with one href → "read more" vague), vague lists by locale (`de-AT` uses `de` + `en`; `ja` uses `en` only), `table-header-missing` for header row and header column, `link-anchor-missing` against `heading_ids`, rich-text fields inside repeaters checked with `Location::Field`, stable `key` formats of design §7.2, `context` cut at 120 characters on a char boundary.
- [ ] Hostile tests: `deep_nesting_is_truncated_not_overflowed` (depth 100 000 → `check-truncated`, no stack overflow), `wrong_types_are_skipped` (attrs as arrays, `content` as a string, numbers as text), `findings_capped_at_500`, `links_capped_at_1000`, `million_text_nodes_bounded` (finishes under 2 s in release, 10 s in debug).
- [ ] `heading_ids_match_render_toc` (property over the existing render fixtures).
- [ ] Implement; `cargo test -p ferrite-core quality blocks` passes; clippy clean.
- [ ] Commit `feat(core): accessibility rule catalogue and document checks`.

### Task 2: Decorative images in render and Markdown

**Files:**
- Modify: `crates/ferrite-core/src/blocks/render.rs` (`render_image`: `decorative: true` → `alt=""`), `crates/ferrite-core/src/blocks/markdown.rs` (`image`: `None` for decorative, so it becomes a `ferrite-block` fence), `crates/ferrite-plugin-host` sanitiser only if it strips the attribute from block JSON (check `grep -rn '"alt"' crates/ferrite-plugin-host/src`; leave it if it never sees block JSON).

**Interfaces:** none new (attribute `decorative` on `image` nodes, design §4.1).

- [ ] Tests: `decorative_image_renders_empty_alt` (alt text present but ignored), `decorative_image_round_trips_through_markdown` (to_markdown → fence → `ferrite_import_md::markdown_to_body` → same node), `non_decorative_unchanged` (existing image tests still pass byte for byte).
- [ ] Implement; `cargo test -p ferrite-core blocks` and `cargo test -p ferrite-import-md` pass.
- [ ] Commit `feat(blocks): decorative images render alt="" and keep their flag through Markdown`.

### Task 3: Storage (0019) and the quality repo

**Files:**
- Create: `crates/ferrite-store-{sqlite,postgres}/migrations/0019_content_health.{up,down}.sql`, `crates/ferrite-store-{sqlite,postgres}/src/repos/quality.rs`, `crates/ferrite-store-tests/src/quality.rs`, `crates/ferrite-domain/src/quality.rs`
- Modify: `crates/ferrite-domain/src/{lib.rs, content.rs}` (`ContentType.quality_gate: QualityGate`, `string_enum!(QualityGate { Off => "off", Errors => "errors" })`), `crates/ferrite-store/src/repos.rs` (`QualityRepo`, `Repos::quality()`), both stores' `repos/mod.rs` and `codec.rs` (content type column), the content type insert/update SQL of both stores, `crates/ferrite-core/src/types.rs` (the one `ContentType { … }` literal: `quality_gate: QualityGate::Off`), `crates/ferrite-store/src/catalogue.rs` (column; `quality_suppressions` exported; `health_runs`, `health_findings`, `link_checks` `Export::Skip` with a reason, and `skipped_tables_are_the_p2_list` updated), `crates/ferrite-store-tests/src/{lib.rs, migrations.rs}`, every test literal of `ContentType` (`rg -n "ContentType \{" crates`).

**Interfaces:**
- Domain: `Suppression { item_id, rule: String, key: String, reason: String, user_id: Option<UserId>, created_at }`, `HealthRunId(Uuid)`, `HealthRun { id, status: HealthRunStatus{Running,Done,Failed,Abandoned}, trigger: HealthTrigger{Schedule,Manual}, requested_by, node, started_at, finished_at, items_checked, paths_checked, urls_checked, totals: Value, truncated, error }`, `NewFinding { rule, severity, category, item_id, redirect_id, item_type, locale, key, context, detail }`, `StoredFinding { run_id, seq, ..NewFinding }`, `FindingQuery { run, category, rule, severity, item_type, locale, limit, cursor }`, `LinkCheck { url, host, status: LinkStatus{Ok,Failing,Broken,Skipped}, http_status, detail, failures, first_failed_at, checked_at, last_seen_at }`.
- `QualityRepo`: `suppressions_for(&[ContentId]) -> Vec<Suppression>`, `suppress(&Suppression)` (upsert), `unsuppress(item, rule, key) -> bool`, `published_page(after: Option<ContentId>, limit) -> Vec<ContentItem>` (statuses published + scheduled, type ≠ media, by id), `last_revision_at(&[ContentId]) -> Vec<(ContentId, Timestamp)>` (one grouped query), `insert_run`, `finish_run(id, status, counters, totals, truncated, error)`, `abandon_running(now) -> u64`, `latest_run(status) -> Option<HealthRun>`, `running_run()`, `prune_runs(before, keep: HealthRunId) -> u64`, `insert_findings(run, first_seq, &[NewFinding])`, `delete_findings_except(run)`, `findings(&FindingQuery) -> Page<StoredFinding>`, `link_get(&[url_hash]) -> Vec<LinkCheck>`, `link_put(&LinkCheck)`, `link_prune(unseen_before) -> u64`.

- [ ] Write `migration_0019_roundtrip` (up, down, up on both) and contract tests: `quality_gate_defaults_off_and_round_trips`, `suppression_upsert_and_delete`, `suppressions_cascade_with_item`, `published_page_skips_drafts_media_and_trash`, `last_revision_at_groups_per_item`, `findings_page_filters_and_cursor`, `findings_cascade_with_run_and_item`, `delete_findings_except_keeps_one_run`, `abandon_running_marks_old_runs`, `link_cache_put_get_prune`, `catalogue_matches_live_schema` (existing, must pass with the new table list).
- [ ] Run on both backends, see them fail; implement; `cargo test -p ferrite-store-sqlite -p ferrite-store-postgres -p ferrite-store` with `FERRITE_TEST_PG_URL` passes.
- [ ] Commit `feat(store): content health runs, findings, link cache, suppressions and the type gate (0019_content_health)`.

### Task 4: QualityService, the gate and the settings

**Files:**
- Create: `crates/ferrite-core/src/quality/{service.rs, facts.rs, gate.rs}`
- Modify: `crates/ferrite-core/src/error.rs` (`CoreError::QualityBlocked(Box<Vec<Finding>>)`, `#[error("accessibility errors block publishing")]`), every exhaustive `match` on `CoreError` (`rg -n "CoreError::Store\(s\) =>" crates`), `crates/ferrite-core/src/content/lifecycle.rs` (`transition_item` runs `gate::check` for `Action::Publish` and `Action::Schedule` when called through `transition`; `publish_in` passes `Gate::Skip`), `crates/ferrite-core/src/content.rs` (`update_in`: "adds errors" rule for published/scheduled items of gated types), `crates/ferrite-core/src/types.rs` (`NewType.quality_gate`, `TypePatch.quality_gate: Option<QualityGate>`, `media` refuses `errors`), `crates/ferrite-core/src/settings.rs` (`health.external_links` Bool default false, `health.stale_days` Integer 30..=3650 default 365; module doc table), `crates/ferrite-core/src/lib.rs` (re-exports).

**Interfaces:**
- `pub trait PathProbe: Send + Sync { async fn probe(&self, path_and_query: &str) -> PathStatus; }` and `pub enum PathStatus { Ok { item: Option<ContentId> }, Redirected { to: String, item: Option<ContentId> }, NotFound, Gone, External(String), Unchecked }` (in `quality/mod.rs`; implemented in Task 6).
- `facts::load(repos, &[MediaRef]) -> MediaFacts` (exists, public, image, alt blank — nothing else) and `facts::apply(&MediaFacts, &mut Vec<Finding>, refs)` → `media-missing`, `featured-alt-missing`.
- `gate::check(repos: &dyn Repos, item: &ContentItem, ty: &ContentType) -> Result<(), CoreError>` (unsuppressed accessibility errors → `QualityBlocked`, at most 50), `gate::error_count(repos, title, body, fields, ty, item_id) -> usize`.
- `QualityService::new(store, site_origin: String)`; `check(&self, actor, kind: &str, draft: CheckDraft, probe: Option<&dyn PathProbe>) -> Result<CheckReport, CoreError>` where `CheckDraft { id: Option<ContentId>, title, body, fields, locale: Option<String> }` and `CheckReport { findings: Vec<CheckedFinding /* Finding + Option<Suppression> */>, counts: Counts, gate: GateState { mode: QualityGate, blocked: bool } }` (probes at most 200 distinct internal paths → `link-internal-broken`/`link-internal-redirected`); `suppressions(actor, id)`, `suppress(actor, id, rule, key, reason)`, `unsuppress(actor, id, rule, key)` (design §7.2 rights; audit `quality.suppress` / `quality.unsuppress`; unknown rule or a site-scope rule → `InvalidField rule`; reason 1..=500 characters without control characters).

- [ ] Tests (in-memory SQLite store, a fake `PathProbe`): `check_reports_media_missing_and_trashed`, `check_hides_media_details_from_unreadable` (only facts), `check_marks_suppressed_and_excludes_from_counts`, `check_probe_capped_at_200_paths`, `publish_refused_on_gated_type_with_errors`, `publish_allowed_with_warnings_only`, `publish_allowed_when_errors_suppressed`, `schedule_refused_on_gated_type`, `publish_due_and_publish_scheduled_never_gated`, `publish_in_not_gated` (seed path), `update_of_live_item_refused_only_when_errors_increase`, `restore_revision_of_live_item_gated`, `submit_not_gated`, `author_cannot_suppress_error_on_gated_type`, `editor_can_suppress_error`, `author_can_suppress_warning`, `suppress_audited`, `media_type_refuses_errors_gate`, `health_settings_validate`.
- [ ] Implement; `cargo test -p ferrite-core` passes; clippy clean.
- [ ] Commit `feat(core): quality service, suppressions and the per-type publish gate`.

### Task 5: REST: check, suppressions, type gate, 422

**Files:**
- Create: `crates/ferrite-http/src/api/quality.rs`, `crates/ferrite-http/src/api/dto/quality.rs`, `crates/ferrite-http/tests/api/quality.rs`
- Modify: `crates/ferrite-http/src/api/{mod.rs (routes), state.rs (ApiParts.quality: Arc<QualityService>, ApiState.probe: Option<Arc<dyn PathProbe>>), problem.rs (CoreError::QualityBlocked → 422 type `content-quality`, `findings` member; the problem table test), types.rs + dto for types (quality_gate), dto.rs (module)}`, `crates/ferrite-http/tests/api/{main.rs, harness.rs}`, `crates/ferrite/src/serve.rs` (build `QualityService`), `crates/ferrite-http/openapi.json`, `admin/src/api/schema.d.ts`.

**Interfaces:**
- `POST /api/v1/content/{type}/check` (`CheckRequestDto { id?, title, body, fields?, locale? }` → `CheckResponseDto { findings: [FindingDto], counts, gate }`), `GET|POST /api/v1/content/{type}/{id}/suppressions`, `DELETE /api/v1/content/{type}/{id}/suppressions/{rule}?key=`; `FindingDto { rule (enum of every id), severity, category, wcag, location: LocationDto, key, context, detail, suppressed: Option<SuppressionDto> }`.
- `TypeDto.quality_gate`, type create/patch `quality_gate`.

- [ ] Tests: `check_draft_returns_findings_with_paths`, `check_existing_item_applies_suppressions`, `check_needs_content_read_scope`, `check_static_segment_does_not_shadow_items`, `publish_on_gated_type_is_422_with_findings`, `bulk_publish_path_gated` (the same endpoint the admin's bulk bar uses), `suppress_and_unsuppress`, `type_patch_quality_gate`, `rule_enum_matches_core_catalogue` (OpenAPI `FindingDto.rule` enum == `quality::RULES` ids), `openapi_snapshot_is_current`.
- [ ] Implement (the probe stays `None` until Task 6); regenerate the spec and `pnpm -C admin api:gen`; `cargo test -p ferrite-http --test api quality types content` passes.
- [ ] Commit `feat(api): content checks, suppressions and the type gate setting`.

### Task 6: The internal path probe

**Files:**
- Create: `crates/ferrite-http/src/quality_probe.rs`, `crates/ferrite-http/tests/app/quality_probe.rs` (+ registration in that test crate's `main.rs`)
- Modify: `crates/ferrite-http/src/app/pipeline.rs` (`load` → `pub(crate)`; no behaviour change), `crates/ferrite-http/src/lib.rs` (`pub use quality_probe::SiteProbe`), `crates/ferrite/src/serve.rs` (`ApiState.probe = Some(SiteProbe::new(state.clone()))`)

**Interfaces:**
- `pub struct SiteProbe { state: Arc<AppState> }`, `impl PathProbe for SiteProbe` (design §6.2: redirects up to 10 hops, `route::resolve`, `Canonical` once, `pipeline::load` for listings and content, artifact row for `/_media/`, `Unchecked` for `/_p/`, `/admin/`, `/api/`, `/mcp`, `/oauth/`, `/.well-known/`, absolute URLs on `site.url`'s origin made relative; path at most `route::MAX_PATH_BYTES`).

- [ ] Tests: `published_page_ok_with_item_id`, `draft_page_not_found`, `trashed_media_not_found`, `stored_redirect_followed_and_reported`, `redirect_410_is_gone`, `redirect_to_external_is_external`, `canonical_trailing_slash_redirected`, `term_and_archive_routes_ok`, `plugin_and_admin_paths_unchecked`, `absolute_site_url_is_internal`, `probe_records_no_hit_view_or_cache_entry` (redirect hit counter, metrics registry, `request.served` buffer and page cache unchanged after 100 probes), `hostile_paths` (`//evil`, `%2F`, 10 KiB path, control characters → `NotFound` or `Unchecked`, never a panic). Plus in `tests/api/quality.rs`: `check_flags_broken_internal_link`.
- [ ] Implement; `cargo test -p ferrite-http` passes.
- [ ] Commit `feat(http): internal link probe over the router and the page lookup`.

### Task 7: The external link checker

**Files:**
- Create: `crates/ferrite-http/src/link_check.rs` (`LinkChecker`, robots parser in a `robots` submodule), `crates/ferrite-http/tests/app/link_check.rs`
- Modify: `crates/ferrite-http/src/plugins/egress.rs` (a `pub(crate) fn link_client()` like `metadata_client`, http and https, and its `test-seams` twin), `crates/ferrite-core/src/quality/mod.rs` (`pub trait LinkProbe: Send + Sync { async fn check(&self, url: &Url) -> LinkOutcome; async fn robots_allows(&self, url: &Url) -> RobotsVerdict; }`, `pub enum LinkOutcome { Ok(u16), Restricted(u16), NotFound(u16), Gone, NoSuchHost, RateLimited, Failed(String /* class */), Refused(&'static str), Unchecked(&'static str) }`), `crates/ferrite-http/src/lib.rs`

**Interfaces:**
- `LinkChecker::new(version: &str, site_url: &str) -> Result<Self, String>`; `#[cfg(feature = "test-seams")] LinkChecker::for_tests(egress::EgressOverrides, …)`.
- Robots: `robots::parse(bytes) -> Rules` (groups `ferrite-linkcheck` and `*`, `Allow`/`Disallow`, `*`, `$`, longest match wins as RFC 9309 §2.2.2), `Rules::allows(path) -> bool`.

- [ ] Tests (local http server; the `allow_ip` seam permits loopback only where a test needs it): `head_then_get_on_405`, `redirect_hops_checked_and_capped_at_5`, `redirect_to_private_address_refused` (resolver returns 10.0.0.1 / ::1 / 169.254.x on a hop), `ip_literal_private_refused`, `non_default_port_unchecked`, `credentials_in_url_refused`, `timeout_10s_is_failed` (a server that never answers, paused clock), `404_and_410_classified`, `401_403_restricted`, `429_rate_limited`, `body_never_read` (a 1 GiB streaming body ends after headers), `robots_disallow_unchecked`, `robots_4xx_allows_all`, `robots_5xx_skips_host`, `robots_capped_at_64k`, `robots_parser_hostile` (no newline 1 MiB, invalid UTF-8, 10 000 groups), `logs_name_host_only` (tracing capture).
- [ ] Implement; `cargo test -p ferrite-http --test app link_check` passes (the dev-dependency on itself enables `test-seams`).
- [ ] Commit `feat(http): SSRF-checked external link checker with robots.txt`.

### Task 8: The health runner

**Files:**
- Create: `crates/ferrite-core/src/quality/health.rs`, `crates/ferrite-core/src/quality/external.rs` (pacing: global 4, per host 1 with 2 s spacing, budget 1 000 / 30 min, cache rules of design §6.3)
- Modify: `crates/ferrite-core/src/quality/mod.rs`, `crates/ferrite-core/src/events.rs` (`HEALTH_COMPLETED = "health.completed"`, `health_completed(run) -> NewEvent` with key `run:<id>`; key grammar table)

**Interfaces:**
- `HealthRunner::new(store, site_origin, path_probe: Arc<dyn PathProbe>, link_probe: Option<Arc<dyn LinkProbe>>, node: String)`; `run(&self, trigger: HealthTrigger, user: Option<UserId>, deadline: Instant) -> Result<HealthRun, CoreError>` (design §6.1 steps 1–8; reads `health.external_links` and `health.stale_days` at start).

- [ ] Tests (in-memory SQLite, fake probes, `tokio::time::pause` for pacing): `run_writes_item_findings_per_page`, `internal_broken_link_reported_per_referrer`, `orphan_rules` (menu target, front page, linked, child of linked, type with archive → not orphan; unreachable page → orphan; menu of another locale does not count), `redirect_chain_loop_and_broken_target`, `stale_uses_newest_revision`, `external_off_by_default_contacts_nothing`, `external_cache_reuses_ok_for_7_days`, `external_broken_after_two_failures_20h_apart`, `external_404_broken_at_once`, `external_budget_and_per_host_spacing`, `findings_cap_sets_truncated`, `old_findings_deleted_on_done`, `failed_run_marked_failed`, `completed_event_appended`, `suppressed_findings_excluded_from_totals`.
- [ ] Implement; `cargo test -p ferrite-core quality` passes.
- [ ] Commit `feat(core): the content health runner`.

### Task 9: The job, the schedule and the health API

**Files:**
- Create: `crates/ferrite/src/jobs/health.rs` (`ContentHealthHandler`, `content_health_periodic()`), `crates/ferrite-http/tests/api/health.rs`, `crates/ferrite/tests/cluster/health.rs` (+ registration)
- Modify: `crates/ferrite-core/src/jobs.rs` (`CONTENT_HEALTH_KIND = "content.health"`, payload `{v:1, trigger, user}`, `content_health_job(trigger, user)` with key `content.health:manual:<uuid v7>`), `crates/ferrite-core/src/quality/service.rs` (`summary(actor)`, `findings(actor, query)`, `request_run(actor) -> JobId` with the queued-or-running check and audit `health.run`), `crates/ferrite-store/src/lock.rs` (`LOCK_HEALTH = "content.health"`), `crates/ferrite/src/jobs.rs` (module, re-exports; `core_periodics` gains the health periodic), `crates/ferrite/src/serve.rs` (handler with `SiteProbe` + `LinkChecker`), `crates/ferrite-http/src/api/{quality.rs, dto/quality.rs, mod.rs}` (`GET /health/summary`, `GET /health/findings`, `POST /health/runs`), `crates/ferrite-http/openapi.json`, `admin/src/api/schema.d.ts`.

**Interfaces:**
- `ContentHealthHandler { runner: Arc<HealthRunner>, store }`: kind `content.health`, timeout 1 h, `try_lock(LOCK_HEALTH)` → `Busy`; `CoreError` store errors → `Retry`, payload errors → `Permanent`.
- DTOs `HealthSummaryDto { last: Option<HealthRunDto>, running: Option<HealthRunDto>, next_at, totals: [RuleTotalDto], external_links: bool }`, `HealthFindingDto { …FindingDto, item: Option<HealthItemRefDto { id, type, title, status, locale, admin_url }>, redirect: Option<RedirectRefDto> }`.

- [ ] Tests: `periodic_is_daily_at_0400_and_fits`, `manual_run_202_then_409_while_queued`, `summary_and_findings_need_edit_others`, `findings_filters_type_locale_severity`, `run_end_to_end_on_seeded_site` (serve with test seams, enqueue, wait for `done`, findings visible), `busy_lock_requeues_without_attempt`, cluster (`FERRITE_TEST_PG_URL`): `two_nodes_one_run_per_period`, `manual_and_periodic_never_overlap`, `any_node_serves_the_report`, `openapi_snapshot_is_current`.
- [ ] Implement; `cargo test -p ferrite --test cluster health` (with Postgres) and `cargo test -p ferrite-http --test api health` pass.
- [ ] Commit `feat: content.health job, daily schedule and the health API`.

### Task 10: MCP tools

**Files:**
- Create: `crates/ferrite-http/src/mcp/tools/quality.rs`, `crates/ferrite-http/tests/api/mcp_quality.rs`
- Modify: `crates/ferrite-http/src/mcp/tools/mod.rs`, `crates/ferrite-http/src/mcp/server.rs` (`+ Self::quality_tools()`), `crates/ferrite-http/src/mcp/results.rs` (`QualityBlocked` → error result listing the findings, structured `findings`), `crates/ferrite-http/src/mcp/tools/types.rs` (`quality_gate` in type get/create/update), `crates/ferrite-http/src/mcp/resources.rs` (`ferrite://guide/writing`: alt text, headings, link text, "run content_check before content_publish"), `site/content/05-reference/02-mcp-tools.md` (two rows)

**Interfaces:**
- `content_check` input `{ id?: uuid } | { type, title?, body_markdown?, body_blocks?, fields?, locale? }` (exactly one of `id` or `type`; Markdown converted with `ferrite_import_md::markdown_to_body` as in `content_create`), output `{ findings: [{rule, severity, category, wcag, message, context, fix, suppressed}], counts, gate }`; `health_report` input `{ category?, rule?, severity?, type?, locale?, per_page?, cursor? }`, output `{ summary, findings, next_cursor }`. Both `read_only_hint = true`, area `content` (read scope).

- [ ] Tests: `content_check_markdown_draft_finds_missing_alt`, `content_check_by_id_applies_suppressions`, `content_publish_blocked_lists_findings`, `health_report_needs_edit_others`, `health_report_pages`, `tools_list_has_both_with_annotations`, `reference_page_lists_every_tool` (existing test, must pass), `writing_guide_mentions_content_check`.
- [ ] Implement; `cargo test -p ferrite-http --test api mcp` passes.
- [ ] Commit `feat(mcp): content_check and health_report tools`.

### Task 11: Admin: checks in the editor

**Files:**
- Create: `admin/src/api/quality.ts`, `admin/src/editor/quality/{use-quality-check.ts, use-quality-check.test.ts, decorations.ts, decorations.test.ts, findings.ts, findings.test.ts, suppress-dialog.tsx}`, `admin/src/editor/panel/quality-section.tsx`, `admin/src/editor/panel/quality-section.test.tsx`, `admin/src/i18n/en/quality.json`, `admin/e2e/editor-quality.spec.ts`
- Modify: `admin/src/editor/editor.tsx` (register the decoration plugin, run the hook), `admin/src/editor/panel/side-panel.tsx` (the "Checks" section), `admin/src/editor/panel/publish-dialog.tsx` (counts, gate-disabled confirm, 422 list), `admin/src/editor/image-node.tsx` (Decorative switch in `AltEditor`), `admin/src/editor/schema.ts` (image `decorative` attribute), `admin/src/api/keys.ts`, `admin/src/i18n` index (namespace)

**Interfaces:**
- `checkContent(type, draft, signal) -> CheckResponse`, `suppress(type, id, {rule, key, reason})`, `unsuppress(type, id, rule, key)`.
- `findingPos(doc: PMNode, path: number[]) -> number | null`; `messageFor(t, finding) -> string` (one i18n key per rule id; a Vitest test asserts every `FindingDto.rule` enum value has a key).

- [ ] Vitest: debounce 1.5 s and one request in flight; stale results dropped by revision; path → position mapping incl. nested lists and tables; widget has an `aria-label`; panel groups by severity and collapses suppressed; suppress dialog requires a reason; publish dialog disables confirm when `gate.blocked`; decorative switch sets `decorative` and clears `alt`.
- [ ] E2E `editor-quality.spec.ts` (Chromium, both backends as the suite runs): image without alt flagged inline and in the panel → fixed via the alt popover → finding gone; decorative switch clears it; vague link flagged; type set to "Block publishing on errors" (via API in the setup) → publish disabled with the reason, a forced API publish 422; suppression by an editor unblocks; axe clean on the editor with findings shown.
- [ ] `pnpm -C admin typecheck && pnpm -C admin lint && pnpm -C admin test && pnpm -C admin e2e editor-quality` pass; `pnpm -C admin bundle:check` within budget.
- [ ] Commit `feat(admin): accessibility checks in the editor and the publish dialog`.

### Task 12: Admin: health screen, dashboard card, type gate, settings

**Files:**
- Create: `admin/src/routes/_app/health.index.tsx`, `admin/src/health/{health-screen.tsx, health-screen.test.tsx, summary.tsx, findings-table.tsx, health-model.ts, health-model.test.ts}`, `admin/src/dashboard/health-card.tsx`, `admin/src/dashboard/health-card.test.tsx`, `admin/src/settings/health.tsx`, `admin/src/settings/health.test.tsx`, `admin/e2e/health.spec.ts`
- Modify: `admin/src/shell/nav.ts` (`health` screen, section `content`, `can("edit_others")`, shortcut `g h`; `SCREEN_ENTRIES`), `admin/src/shell/nav.test.ts`, `admin/src/routeTree.gen.ts` (generated), `admin/src/routes/_app/index.tsx` (the dashboard grid), `admin/src/structure/types/type-form.tsx` (+ test: the gate select, hidden for `media`), `admin/src/settings/groups.ts`, `admin/src/i18n/en/{quality.json, shell.json, dashboard.json, admin.json}`, `admin/src/api/quality.ts`

**Interfaces:** `healthSummaryQuery`, `healthFindingsQuery(filters)`, `requestHealthRun()`; `?check=<rule>:<key>` on the editor route opens the panel at that finding (read by Task 11's section).

- [ ] Vitest: tabs filter by category; never-run, running and failed states; "Run now" 409 shows "already running"; external-off note links to Settings; card hidden without `edit_others`; type form sends `quality_gate`; settings validate stale days 30–3650.
- [ ] E2E `health.spec.ts`: seed a site with a broken internal link, an orphan page, a redirect chain and an image without alt; "Run now"; the four findings appear in their tabs; a row opens the editor at the finding; `g h` navigates; axe clean (light and dark); 44 px targets.
- [ ] `pnpm -C admin typecheck && pnpm -C admin lint && pnpm -C admin test && pnpm -C admin e2e health` pass; bundle budget holds (the route is lazy).
- [ ] Commit `feat(admin): content health screen, dashboard card, type gate and settings`.

### Task 13: Documentation

**Files:**
- Create: `site/content/02-guides/<next free number>-content-health.md` (the editor checks, decorative images, the gate, the report, external links and their privacy note, suppressions; screenshots in `site/content/images/`)
- Modify: `docs/design/07-admin-api.md` (§1 resources and status lines for the new endpoints; §3 screens), `docs/design/11-admin-ui.md` (§6 `g h`), `docs/design/12-mcp.md` (§5.2 table: `content_check`, `health_report`), `docs/design/02-storage.md` (the four tables and the column), `docs/design/04-cluster.md` (the `content.health` job and lock), `docs/design/06-themes-rendering.md` (decorative images render `alt=""`), `site/content/02-guides/08-ai-assistants.md` (the two tools), `site/content/05-reference/02-mcp-tools.md` (prose for the two rows), `site/VERIFIED.md` (the new claims, each verified against the code)

- [ ] `cargo test -p ferrite --test docs_site` passes (link check of the docs site).
- [ ] Commit `docs: accessibility checks and content health`.

### Task 14: Exit

**Files:**
- Create: `docs/superpowers/m6-4-exit.md`
- Modify: `docs/ROADMAP.md` (tick both M6-4 items; follow-ups found on the way under "Later"), `CLAUDE.md` (status paragraph: M6-4 done, migration `0019_content_health`, the crate map entries `quality/`, `quality_probe`, `link_check`, `jobs/health.rs`, the run instructions for the check endpoint and the health API)

- [ ] Run and record: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `FERRITE_TEST_PG_URL=… cargo test --workspace`, `FERRITE_TEST_PG_URL=… cargo test -p ferrite --test cluster`, `pnpm -C admin lint typecheck test api:check bundle:check`, `pnpm -C admin e2e:all` (Chromium on both backends, Firefox, WebKit), `pnpm -C admin e2e --project public-themes-chromium` (decorative images keep the themes axe-clean).
- [ ] A manual pass on a local site (`seed demo`, `import wordpress` of a real export if one is at hand): the editor's inline marks and panel, the gate, "Run now", the report; external links on against a handful of real sites (one with a robots.txt `Disallow`), noting what was and was not exercised.
- [ ] Write `m6-4-exit.md`: what was verified (commands, counts), what was not (e.g. external checks against many real hosts, screen readers other than the axe checks, Safari), decisions taken during implementation, open follow-ups.
- [ ] Commit `docs: M6-4 exit`.
