# M6-4 exit: accessibility checks and content health

Branch `m6-4-content-quality` (from `m6` at 6945d2a4; `m6` merged in again at 6d676d5a,
after M6-5). Design: [16](../design/16-content-quality.md) (§15 "As built"). Plan:
[`plans/2026-11-02-m6-4-content-quality.md`](plans/2026-11-02-m6-4-content-quality.md).
Rulings P3, P4 and P6 ([`rulings/m6.md`](rulings/m6.md)) applied; migration
`0019_content_health` only.

## 1. What was built

- **Rules** (`ferrite_core::quality`): the catalogue `RULES` (24 ids: design 16 §3 plus
  P3's `image-alt-empty` and the engine's `check-truncated`, `findings-truncated`), the
  iterative capped document walk (`check_document`: body and rich-text fields inside
  repeaters, depth 64, 500 findings, 1 000 links per item), vague phrases for en, de, fr,
  es, pl, `blocks::heading_ids` (the renderer's ids without rendering), media facts.
- **Decorative images**: `decorative: true` renders `alt=""`; Markdown keeps the flag as
  a `ferrite-block` fence.
- **Storage** `0019_content_health` (both backends, expand, lossy down):
  `content_types.quality_gate`, `quality_suppressions` (exported), `health_runs`,
  `health_findings`, `link_checks` (skipped); `QualityRepo`; contract tests on both.
- **Gate** (`quality::gate::check`, P4): publish and schedule (REST, admin, bulk, MCP) of a
  gated type are refused with `CoreError::QualityBlocked` → `422 content-quality` with
  `findings`; a live edit only when it adds errors; scheduler, `publish_in` (seed,
  uploads) and imports are never gated. `media` refuses `errors`.
- **QualityService**: checks with at most 200 probed paths, suppressions (reason
  required; an error on a gated type needs `edit_others`; audited), the health summary,
  findings and "Run now" (`409` while queued or running, audited `health.run`).
- **HTTP**: `SiteProbe` (redirects ≤ 10 hops, the route table, the pipeline's lookup;
  no render, cache, counters or plugins), `LinkChecker` (egress checked resolver, peer
  re-check, IP literals, default ports, no credentials, HEAD then GET without reading
  the body, ≤ 5 checked hops, 10 s, robots.txt ≤ 64 KiB in 5 s), the REST endpoints with
  OpenAPI, `quality_gate` on `/types`, the settings `health.external_links`,
  `health.stale_days`.
- **Job** `content.health` (daily 04:00 UTC, manual runs, lock `content.health`, 1 h):
  `HealthRunner` (pages of 200, internal links per referrer, orphans, redirect
  chains/loops/broken targets, stale from the newest revision, external links paced 4 /
  1 per host / 2 s / 1 000 / 30 min with the 7-day cache and the 2-failures-20-h rule),
  totals without suppressed findings, older runs' findings deleted, outbox event and
  webhook `health.completed`.
- **MCP**: `content_check`, `health_report` (read-only); refused publishes list the
  findings; the writing guide has an Accessibility section. 60 tools after the merge with
  M6-5 (58 + 2).
- **Admin**: inline marks with labelled buttons, the side panel's Checks, Suppress…, the
  publish dialog's list and disabled confirm, the Decorative image switch; Content health
  screen (`g h`), dashboard card, the type form's gate switch, Settings → Content health;
  `?check=<rule>:<key>` on the editor route.
- **Docs**: guide `02-guides/11-content-health.md`, the MCP reference rows, designs 02,
  04, 06, 07, 11, 12, `VERIFIED.md` rows.

## 2. Verified

See §4 for the commands and their results.

## 3. Decisions (R-4-n)

- **R-4-1** P3: `image-alt-missing` = no `alt` attribute (error); blank `alt` =
  `image-alt-empty` (warning); `decorative: true` clears both.
- **R-4-2** The gate is called in `ContentService::transition` (the public wrapper) after
  `transition_in` and before the webhook emit, not in `transition_item`; `publish_in` stays
  ungated. M6-3 was told the signature: `gate::check(repos, &item, &ty)`, all-or-nothing.
- **R-4-3** `HealthRun.counters: HealthCounters`; `QualityRepo::finish_run(id, &RunEnd)`.
- **R-4-4** The robots.txt parser lives in core (`quality::robots`, pure);
  `LinkProbe::robots` returns `RobotsFetch` (the body, allow all, skip host).
- **R-4-5** The link-checker tests are in `tests/plugins/link_check.rs` (beside the egress
  tests); no tracing-capture test (no subscriber in the dev-dependencies): hosts only by
  construction (`failure` logs `host` and a class).
- **R-4-6** Suppressible: every item-scope rule plus `orphan`, `stale`,
  `link-external-broken` (design §7.2 over the plan's "no site rule"); never `redirect-*`
  or the truncation notes.
- **R-4-7** `heading-level-skipped` in the body only (fields render anywhere).
- **R-4-8** Health findings keep the location in `detail.location` (no column).
- **R-4-9** Keys: `src`; `href|text`; `h<level>:<text>`; `table:<n>` (1-based, both table
  rules); `html:<n>`.
- **R-4-10** `health_findings` is indexed on `item_id` and `redirect_id` (FK indexes)
  instead of `(run_id, item_id)`.
- **R-4-11** `Problem` keeps its size: the plugin report and the findings share one boxed
  `attached`.
- **R-4-12** The type form has a switch ("Block publishing on accessibility errors"),
  not a two-option select.
- **R-4-13** Local `url` menu items are probed too, so pages they lead to are not orphans.
- **R-4-14** A run past its deadline (1 h less a minute) skips the remaining phases and
  ends `done` with `truncated`.
- **R-4-15** `health.completed` is an outbox event (`run:<id>`) and a webhook event in the
  group `health` (admin label "Content health").
- **R-4-16** After the M6-5 merge: a change-set revert of a type keeps its current gate
  (the snapshot has none, `quality_gate: None`).
- **R-4-17** A bad findings cursor or limit is `400` (the API's extractor convention), a bad
  category/rule/severity `422`.
- **R-4-18** A decorative image inline in a paragraph has no fence to fall back to in
  Markdown and loses the flag (a block image keeps it).

## 4. Gates

Run on the branch after the merge of `m6` (M6-5); `PG` =
`FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test`.

| Gate | Command | Result |
|---|---|---|
| Format | `cargo fmt --all -- --check` | pass |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| Rustdoc | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | pass |
| Workspace tests | `PG cargo test --workspace --no-fail-fast` | pass except `import_cli::import_site_a_end_to_end_sqlite` (environment, §5); the tests fixed after the merge were rerun on their own |
| OpenAPI | `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`, then without the variable | current |
| Admin | `pnpm -C admin typecheck`, `lint`, `test` | pass (907 tests) |
| New E2E specs | `PG pnpm -C admin e2e e2e/editor-quality.spec.ts e2e/health.spec.ts` (Chromium, SQLite and Postgres) | pass, also with `--repeat-each=2` |
| Chromium E2E | `PG pnpm -C admin e2e --project chromium-sqlite --project chromium-pg` | 413 passed, 3 failed: `shortcut_sheet_lists_bindings` (the sheet's list lacked Webhooks and Content health; fixed, passes), `failed_backup_restore_says_so_and_keeps_the_backup` on Postgres once (passes on rerun; not touched here) |

Test fixes outside the new code: `editor-media.spec.ts` `edit_image_alt` (a blank alt now reads
"No alt text", P3), the webhook sink closes idle connections on stop and
`webhook_end_to_end` polls for the scheduled attempt (both raced under load). The health
periodic runs once at node start, so older tests that wait for "no event" ignore
`health.completed` (`tests/node.rs`).

## 5. Not verified

- Firefox and WebKit runs of the new E2E specs (only Chromium; the full cross-browser run is
  at the end of M6).
- External link checks against real hosts (only the local test server and fake probes).
- Screen readers beyond the axe checks of the E2E specs.
- A log-capture test that the link checker never logs URLs (R-4-5).
- `import_cli::import_site_a_end_to_end_sqlite` on this machine: it fails because the
  user's own `ferrite` holds 127.0.0.1:8080, an environment issue, not a code bug.
