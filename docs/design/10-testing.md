# 10 — Testing Strategy

## 1. Layers

| Layer | Where | What | Runs |
|---|---|---|---|
| Unit | next to code (`#[cfg(test)]`) | pure logic: hierarchy resolution, permalink matching, manifest validation, block rendering, WXR fragments | every `cargo test` |
| Store contract | `crates/ferrite-store-tests` | every repository method on **SQLite and Postgres** | every CI run (PG via service container) |
| Migration | `ferrite-store-tests` | up/down/up roundtrip per migration with seeded data; schema snapshot equality; same version sets on both backends; expand-lint | every CI run |
| Plugin host | `crates/ferrite-plugin-host/tests` | capability linking, limits, hostile plugins ([05 §12](05-plugin-host.md#12-hostile-test-plugins-required-by-claudemd)) | every CI run (test plugins built first) |
| Cluster | `tests/cluster_*.rs` | 2–3 in-process nodes against one Postgres: invalidation propagation, job exclusivity, lease expiry after a "crashed" node, periodic job single-run, rolling version coexistence | every CI run |
| HTTP integration | as built (M1c-2): `crates/ferrite-http/tests/app` (in-process app; `app_tests!` runs each test on solo = SQLite + local media and cluster = Postgres + in-process S3), `crates/ferrite-http/tests/plugins`, `crates/ferrite-http/tests/api` (M2a-1: the REST API in process; `api_tests!` runs each test on the same two profiles, `openapi_snapshot_is_current` checks `crates/ferrite-http/openapi.json`, and `sweep.rs` walks the generated spec: every unsafe operation but the five without credentials (the two logins, the two password-reset calls and, since M2a-2b, the comment submit) is `401` without credentials and `403 csrf` for a cookie without the token; M2a-2b added `media.rs`, `idempotency.rs`, `comments.rs`, `system.rs` and `jobs.rs`), `crates/ferrite/tests/serve/` (one test binary, `--test serve`, with one harness: the real binary on a random port with temp data, both profiles; `site.rs` covers `seed demo` + `serve`, `api.rs` has `api_end_to_end_on_real_binary`, which drives login, CSRF, content, tokens, users and revocation over raw HTTP); theme renderer and `blog` theme in `crates/ferrite-theme/tests`, cache review-focus tests in `crates/ferrite-cache/tests` | public pages, routing, cache invalidation, media, feeds, plugins, shutdown, previews and logged-in requests; API CRUD, auth, CSRF/origin, `If-Match`, scopes, rate limits | every CI run, both backends |
| Media | `ferrite-media` tests; uploads in `crates/ferrite-http/tests/api/media.rs`, the `media.variants` job in `crates/ferrite/tests/node/media.rs` | local folder + in-process S3 server (`s3s-fs`), no Docker; `tests/svg.rs` runs the sanitiser over the hostile fixtures in `tests/svg/` (script, `foreignObject`, event handlers, entity tricks, DOCTYPEs, UTF-16, `<use>` fan-out) | every `cargo test` |
| Import | `ferrite-import-wp` tests | fixture WXRs → golden JSON of resulting content | CI |
| Admin | `admin/` | as built (M2b-1): Vitest (`pnpm test`: the `app` project in jsdom, the `scripts` project in Node) and Playwright (`pnpm e2e`, `admin/playwright.config.ts`) against the real release binary. Global setup (`admin/e2e/support/`) starts one site per selected project (M2b-3, P55; until then one per backend): a SQLite site (`init`, `seed demo`, the harness accounts `admin`, `editor`, `author`, `author2`, `contributor`, `resetuser`, `totp` (TOTP enrolled) and the `must_reset` `resetme`, an in-process SMTP sink over `node:net`, then `serve` trusting `127.0.0.1` as a proxy so each login context sends its own `X-Forwarded-For`) and, for `chromium-pg`, a Postgres site in a throwaway schema `e2e_<random>` of `FERRITE_TEST_PG_URL` (with `CI` set the project is always defined, and global setup fails when it is selected without the URL); it logs the accounts in once and saves storage states (`e2e/.auth/<project>/<login>.json`). Projects (M2b-3, `admin/e2e/support/projects.ts`): `chromium-sqlite`, `chromium-pg` (with the URL), `firefox-sqlite` (`Desktop Firefox`) and `webkit-sqlite` (`Desktop Safari`), every spec in each, all at 1440×900. Each is two Playwright projects: `<name>` runs every spec but the site-wide ones (`settings`, `themes`, `system`: settings, the active theme, a dead job) in parallel, and its teardown `<name>-site-wide` runs those after it, one worker, pass or fail. `pnpm e2e` runs `chromium-sqlite` (and `chromium-pg` with the URL), `pnpm e2e:all` all of them (three without the URL, four with it); the wrapper `scripts/e2e.mjs` turns a filtered run (a file, `-g`, `--last-failed`) into both halves of each project with `--no-deps`, so it runs only what it names (and skips the ordering). 2 workers per selected project, 1 retry on CI. `snap` and the contact sheet are taken in `chromium-sqlite` only. Fixtures: `as(role)`, `freshSession` (for tests that log out or reset; shared sessions are never ended), `snap` (light/dark × 1440×900/390×844 under `test-results/screens/` plus a contact sheet; it waits for fonts and finite CSS/WAAPI animations, not for Motion's JS animations, which a spec waits for itself, and fails on horizontal overflow), `expectAccessible` (axe, WCAG 2.2 AA tags, both schemes) and an automatic guard that fails a test on a `securitypolicyviolation`, a page error or a `console.error`. Tests create their own data (`uid()`); tests that compare counts with lists create their own content type. M2b-2 added the specs `media`, `editor-media`, `editor-fields`, `editor-followups`, `types`, `taxonomies`, `menus`, `comments` and `redirects` (SQLite project only until M2b-3; `chromium-pg` ran the smoke file), `styleguide_ordering` (the sortable list and tree) in `styleguide.spec.ts`, and support: `files.ts` (in-memory PNG, SVG, PDF and executable-header fixtures), `comments.ts` (guest comments through the public submit, each from its own `X-Forwarded-For`), `editor-snap.ts` (`showSection`: a panel section or the fields card in view, on phones inside the settings sheet), `snap` options (`prepare(viewport)` after each viewport and scheme change, `fullPage: false`), and `ContentApi` helpers for types, taxonomies, terms, menus, comments, redirects and media; most drags go through the keyboard or the row menus (every drag has that path, P29; `menus` `drag_reorder_with_pointer` and `taxonomies` `cycle_is_refused_and_rolled_back` use the pointer), and file drops go through a real `DataTransfer` | every CI run (`admin` and the `admin-e2e` matrix) |
| Benchmarks | `benches/` (criterion) | filter call overhead, cached/uncached render, cache invalidation fan-out | manual + nightly, results tracked |
| Backward compat | CI job | previous release's integration tests against current migrations | from first tag |

## 2. Test infrastructure

- `FERRITE_TEST_PG_URL` env var. Missing locally → those
  tests are skipped with a printed notice; in CI (`CI=true`) missing → fail.
- Locally, point `FERRITE_TEST_PG_URL` at any Postgres >= 15. CI uses a `postgres:18` service container (`.github/workflows/ci.yml`).
- **S3 tests need no MinIO or Docker.** `ferrite-media` tests start an `s3s-fs` S3 server
  in-process on `127.0.0.1:0` (bucket = a temp directory) and point the real `AmazonS3`
  client at it (`crates/ferrite-media/tests/common/mod.rs`). The `backend_tests!` macro
  generates, for each test body, a `local` and an `s3` test (`mod <name> { local(); s3(); }`),
  so every storage behaviour is asserted on both backends. Caveat: s3s-fs may not implement
  every S3 feature (for example listing orphan multipart parts), so the orphan-multipart
  case is not covered by tests. The S3 lifecycle rule that backstops it is a planned M4 recommendation.
- Postgres isolation: schema-per-test; SQLite: temp dir per test.
- Test plugins are built by the tests themselves: `crates/ferrite-plugin-host/src/test_support.rs`
  (included by path from `tests/common/mod.rs`) runs
  `cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml` once per
  test binary (`OnceLock`) before reading the `.wasm`, and panics with cargo's stderr if it fails.
  This needs the `wasm32-wasip2` target installed. CI also builds the plugins in an explicit step.
- **Test seams** sit behind one dev-only cargo feature, `test-seams` (M2a-2a R22 M11):
  `ferrite-core` has `test-seams = []`, `ferrite-http` has
  `test-seams = ["ferrite-core/test-seams"]` and `ferrite` has
  `test-seams = ["ferrite-http/test-seams"]`. Each crate enables its own feature through a
  dev-dependency on itself (`ferrite-http = { path = ".", features = ["test-seams"] }`). With
  the workspace's resolver 3, dev-dependency features apply only when a test, example or bench
  target is built (`cargo test`, and `cargo build`/`clippy --all-targets`), so tests compile
  the seams and `cargo build -p ferrite --release` (the Docker image) does not.
  `crates/ferrite/tests/seams.rs` (`release_build_has_no_seams`) checks this with
  `cargo tree --locked --offline -e features,normal`. The seams are
  `AuthService::with_clock`, `SiteReader::with_sitemap_file_len`,
  `Installer::{with_theme_loaded, with_prepare}`, `PluginReloader::with_prepare`, `ExtensionService::with_lock_wait`,
  `ExtensionAdmin::with_lock_wait`,
  `SharedFailures::with_before_disable`, `Egress::{for_tests, for_tests_webpki}` with
  `EgressOverrides`, every `NodeOptions` field (`version`, `extra_handlers`, `runner` with
  `RunnerOverrides`, `periodics`, `scheduler_tick`, `served_flush`, `listen`, `boot_barrier`
  with `BootBarrier`/`BootGate`, `image_processor`; a production node uses
  `NodeOptions::default()`), `MediaVariantsHandler::with_processor` with the
  `ferrite::jobs::ImageProcessor` re-export, `AppState::with_admin_assets` (an in-memory
  admin build instead of the embedded `admin/dist`, M2b-1), and `RunningNode::abort` (crash simulation). A
  seam needed only by a crate's own unit tests stays `#[cfg(test)]` (for example
  `PluginHost::extra_wait`). A new seam goes behind the feature, never in a normal build as
  `#[doc(hidden)] pub`.

## 3. CI (GitHub Actions)

Exists today (`.github/workflows/ci.yml`):

1. `cargo fmt --check` for the workspace and for `plugins/`
2. `cargo clippy --all-targets -- -D warnings` (workspace) and clippy for `plugins/` on `wasm32-wasip2`
3. build test plugins (`wasm32-wasip2`)
4. `cargo test --workspace` with a Postgres 18 service

The workflow runs on pushes to and pull requests into `main` (work lands on `develop` first) and on demand (`workflow_dispatch`).

As built in M2b-1 and split in M2b-3 (P55), job `admin` (Ubuntu, the same free-disk step) and the matrix job `admin-e2e`:

5. admin: `pnpm/action-setup@v4` (pnpm from `admin/package.json`'s `packageManager`), Node 24 with the pnpm cache keyed on `admin/pnpm-lock.yaml`, `pnpm -C admin install --frozen-lockfile`, then `lint`, `typecheck`, `test`, `api:check`, `build`, then `bundle:check` (P56: gzip -9 sizes of the entry, the entry with its modulepreloads, the main CSS, the editor chunk, the chart chunk and the largest other lazy chunk, each against its limit in `admin/bundle-budget.json`; over a limit the step fails naming the group; raising one is a reviewed edit of that file); then `cargo build --release --locked -p ferrite` (embedding the fresh `admin/dist`) and the example plugin `admin-e2e` for `wasm32-wasip2`. `admin/dist`, the release binary and the plugin's `.wasm` are uploaded as artifacts (`admin-dist`, `ferrite-release`, `admin-e2e-plugin`)
6. E2E, `admin-e2e` (`needs: admin`, `fail-fast: false`, `project: [chromium-sqlite, chromium-pg, firefox-sqlite, webkit-sqlite]`; only `chromium-pg` starts a `postgres:18` service, the others get an empty `image`, which starts none): downloads the three artifacts (and marks the binary executable), `playwright install --with-deps <engine>`, then `pnpm -C admin e2e --project <project>` (only `chromium-pg` gets `FERRITE_TEST_PG_URL`). On failure it uploads its report, traces and server logs as `admin-e2e-<project>`; `chromium-sqlite` always uploads `admin/test-results/screens/` as `admin-screens`. CI runs the engines on Linux; WebKit there is not Safari, so a manual Safari pass stays in the exit (P55)
7. OpenAPI drift check: `pnpm -C admin api:check` regenerates `src/api/schema.d.ts` from `crates/ferrite-http/openapi.json` and diffs it with the committed file (the spec itself is checked by `openapi_snapshot_is_current` in `check`)

Engine-specific steps in the admin E2E specs (P55). No spec or step is skipped in any engine (no `test.skip(browserName === …)`); where an engine needs another way to the same check, the spec takes it:

| Spec | Engine | What differs | Why |
|---|---|---|---|
| `themes` (folder uploads) | WebKit | the theme goes through the `.tar` input (blog-min's fixture, or the folder packed with the dialog's own `packTheme`) | Playwright's WebKit has no directory upload |
| `design` `dark_os_first_paint_is_dark_before_js` | Firefox | the colour scheme is emulated again on the loaded page | Playwright's Firefox drops a context's `colorScheme` when `Cross-Origin-Opener-Policy: same-origin` moves the page to another process |
| `shell` `keyboard_focus_rings_in_the_shell` | WebKit on macOS | Option+Tab instead of Tab | Tab moves through form controls only there, as Safari does by default |
| guard (`fixtures.ts`) | Firefox, WebKit | accepts each engine's report of a load that a navigation cancelled (Firefox: an aborted web font, `status=2152398850`; WebKit: "Fetch API cannot load <own origin>/… due to access control checks." when that page saw that URL's request cancelled within 2 s, and "Importing a module script failed." when it saw a script request cancelled within 2 s) | Chromium cancels these silently |
| `as`/guard pages | WebKit | `page.reload()` is tried again once after "Frame load interrupted" | a reload while the page still fetches its preloaded modules can be interrupted |

Engine-neutral changes made for the matrix: `account_screens` loads the page again with `goto` instead of `reload()` then `goto` (the pair raced in Firefox); `editor-media` puts the title's caret at the end with `setSelectionRange` (End does not move it in a text field in WebKit and Firefox on macOS); `styleguide_screens` opens the dialog from the keyboard before checking the ring on the trigger focus returns to (after a mouse open, Firefox's `:focus-visible` heuristics show none); `upload_retry_after_network_drop_creates_one_item` stores the first upload with its own request using the page's key (WebKit gives an intercepted request no file body for `route.fetch()`); `reset_token_is_stripped_and_never_sent` falls back to a request's sending-time headers after 5 s (WebKit never reports raw headers for a request the reload cancelled); and `design` accepts the font family with or without quotes (WebKit drops them). Run side by side with more load, the matrix also showed specs leaning on other tests' data or timing: the guard now also accepts a 404 for a custom taxonomy's terms and, outside `plugins.spec.ts`, a plugin's metrics (what another test may delete; before only a custom type's list), `expectAccessible` leaves out a toast that is fading in or out (`TOAST_IN_TRANSITION`), the editor's `screens` uploads its own image, the palette's search check allows the custom types other tests create and its theme switch types the whole command name, the bubble-menu steps select again until the menu shows, and the waits after a save use the response instead of a toast that may still be up from the save before.

The `check` job builds without `admin/dist`, so it covers the placeholder path; the `docker` job builds the image, whose `admin` stage builds the SPA.

Planned:

8. migration set parity + expand-lint

## 4. Definition of done (per checklist item)

- Tests for the behaviour, on both backends where storage is touched.
- fmt + clippy clean.
- Docs updated if behaviour or config changed.
- One reviewable commit (or a small series) per checklist item.

**Known harness quirk.** A Postgres-backed test that ends right after dropping an uncommitted `Tx` stalls about 5 s at schema teardown (the dropped transaction's connection is released late). Commit or roll back explicitly to avoid it.
