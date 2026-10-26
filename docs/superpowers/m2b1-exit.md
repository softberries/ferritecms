# M2b-1 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M2b-1; plan Task 18): *a fresh `init` + `seed demo` +
`serve` on a release build: sign in as each role, write and publish a post, schedule one,
resolve a conflict between two browser windows, restore a revision, reset a password through
a real SMTP catcher, in Chrome and Safari; the workspace tests on both backends with and
without `admin/dist`; the admin gates and `pnpm -C admin e2e` on SQLite and Postgres (smoke);
the contact sheet reviewed against 11 §8; bundle sizes recorded as the M2b-3 baseline;
`docker build` with the admin stage serves `/admin/`.*

**Result (2026-10-26, branch `m2b-admin` from `24f0c02`, plus the fix commits listed below
and this docs commit):** met in Chrome, with the limits below. Two parts are **not met**
here: Safari was not checked (no human hands in this run; ruling R19), and `docker build` could
not run because the local Docker engine did not answer (see "Not verified").

| Exit check | Verified by | Result |
|---|---|---|
| 1. Workspace tests, both backends, with and without `admin/dist` | §1: two full runs with `CI=true` and PostgreSQL 17 | yes |
| 2. Admin gates and `pnpm -C admin e2e` (SQLite + Postgres smoke) | §2 | yes (one flaky unit test and one flaky E2E test found and fixed) |
| 3. Manual flows on a release build | §3: a Playwright script driving Google Chrome against a fresh site | yes in Chrome; Safari **not verified** |
| 4. Contact sheet against 11 §8 | §4: every screen of the final E2E run | yes, two shot defects fixed, one ruling, findings recorded |
| 5. Bundle sizes | §5 | recorded; one fix (TipTap left the entry graph) |
| 6. `docker build` serves `/admin/` | — | **not verified** (Docker engine unresponsive) |

## Commits made in this task

| Commit | What |
|---|---|
| `54070a8` test(admin): stub ResizeObserver for every unit test | `conflict.test.ts` `take_theirs_keeps_mine_in_the_slot` failed 2 of 5 full Vitest runs with "ResizeObserver is not defined" (a Radix tooltip opening on a slow run); the stub lived only in later `describe` blocks. 5 of 5 green after. |
| `e4cfb3a` fix(admin): a new item's title does not take the focus from an open dialog | The parked flake `palette.spec` `create_new_post_from_palette` was a real race: the palette opened while the new-post screen loaded lost its focus to the title's `autoFocus`, so typing went into the title. Failed 7 of 20 alone before, 60 of 60 after (20 + 40 repeats). |
| `f5afa00` perf(admin): keep TipTap out of the entry chunk | The `/$id` route's loader imported the new-item hand-over from `editor-state.ts`, which put the whole editor chunk (TipTap) in `index.html`'s modulepreloads, so every admin page, sign-in included, loaded it. The hand-over moved to `editor/handoff.ts`. Entry + preloads: 1 203 828 → 642 779 bytes, 384 998 → 208 013 with `gzip -9`. |
| `a716181`, `f2ee8a2` test(admin): list status-tab shots wait for … | `content-list-pending` and `content-list-scheduled` (light, desktop) caught rows mid Motion animation (an empty row; a leaving row and a shifted entering row). `snap`'s settle sees CSS/WAAPI animations only; the spec now waits until every row stands still at opacity 1. |
| this commit | `docs/superpowers/m2b1-exit.md`, `docs/superpowers/rulings/m2b.md` (new: P1–P26, F1–F41, R1–R19; the file the plan said the controller would create did not exist), ROADMAP, CLAUDE.md, README, 01 §1, 07 §3, 10 §1 |

## 1. Workspace tests

Darwin 25.6.0 arm64, PostgreSQL 17 at `localhost:54329` (CI uses `postgres:18`).

```
$ CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace --no-fail-fast
  with admin/dist:    79 test result lines, 2298 passed, 0 failed, 1 ignored; no SKIP
$ mv admin/dist <scratch>/dist-aside   # then the same command
  without admin/dist: 79 test result lines, 2298 passed, 0 failed, 1 ignored; no SKIP
  (dist restored with mv afterwards)
$ cargo fmt --all --check                          # ok
$ cargo clippy --all-targets -- -D warnings        # ok
$ cargo clippy -p ferrite -- -D warnings           # ok
```

The ignored test is the `ferrite-sdk` `export_plugin` doc test, as in M2a-2b. Both runs pass
`site::admin_index_on_real_binary` and `site::admin_placeholder_on_real_binary`; each returns
early in the run where it does not apply (dist present or absent at build time), so the pair
covers both builds, and `admin::placeholder_without_dist` (solo, cluster) ran in both. The
first "with dist" run overlapped a `pnpm build` (the `f5afa00` change), so which dist it
embedded is not certain; it was rerun on the final tree (addendum at the end).

No Rust code changed in this task (the fix commits touch `admin/` only), so the Rust gates
above cover the final tree.

## 2. Admin gates and E2E

On the final tree (`f2ee8a2`), Node 24.18.0, pnpm 10.28.2:

```
$ pnpm -C admin lint && pnpm -C admin typecheck   # ok
$ pnpm -C admin test        # 35 files, 258 passed
$ pnpm -C admin api:check   # src/api/schema.d.ts is current
$ pnpm -C admin build       # check-dist: ok
$ cargo build --release -p ferrite
$ FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test pnpm -C admin e2e
  79 passed (3.0 min): 76 chromium-sqlite, 3 chromium-pg (shell.smoke: admin_loads_under_csp,
  assets_are_immutable, smtp_sink_receives_a_reset_mail)
```

Flakes found on the way (all fixed above):

- Vitest: the first two runs failed 4 and 1 tests on 5 s timeouts while `cargo build --release`
  ran beside them (load); the third and two of the next five runs failed
  `take_theirs_keeps_mine_in_the_slot` ("ResizeObserver is not defined") → `54070a8`.
- E2E: `create_new_post_from_palette` failed in the first full run (no retries locally) and
  7 of 20 alone → `e4cfb3a`.

## 3. Manual flows (automated)

There are no human hands in this run (ruling R19), so the flows were driven by a Playwright
script (`flows.mjs`, kept in the session scratchpad, not committed) in **Google Chrome
156.0.8078.48** (Playwright `channel: "chrome"`, headless, 1440×900), not Chromium, against
the release binary (`ferrite 0.1.0`, built from `f5afa00`, which embeds the final admin
build). Each run used a fresh data folder:

1. `ferrite init --data <dir>/site --url http://localhost:18431`, `seed demo` ("12 posts,
   3 pages, 4 terms, 2 menus, 1 image (admin user: admin)"), `user reset-password --login
   admin --password-stdin`, `user create` for `editor`, `author`, `contributor`
   (`--password-stdin`).
2. The repo's SMTP sink (`admin/e2e/support/smtp-sink.ts`, imported by the script) on a free
   port; `serve` with `FERRITE_SERVER__LISTEN=127.0.0.1:18431`,
   `FERRITE_MAIL__SMTP_URL=smtp://127.0.0.1:<sink>`, `FERRITE_MAIL__FROM`.

The third run passed every step (the first two found script errors and a limit, below):

| Step (as scripted) | Seen |
|---|---|
| Sign in through the form as admin, editor, author, contributor; read the main navigation and `/auth/me`; sign out from the account menu | each reached the Dashboard heading; `role` matched; navigation "Dashboard, Posts, Pages" for all four (the only screens M2b-1 has); back on Sign in |
| Author: new post, title + a typed paragraph, `⌘S`, then Publish… → Publish now | URL moved to `/admin/content/post/<id>`; "Published" toast; API `status: published`, version 2; `GET /exit-check-post-…/` on the public site 200 with the title |
| Editor: new post, `⌘S`, `⌘⇧P`, Later, time 10:15, Schedule for… | "Scheduled for" toast; API `status: scheduled`, `scheduled_at` 2026-10-27T09:15:00Z (10:15 in the machine's UTC+1); the panel shows Unschedule |
| Conflict: the same editor signed in in two browser contexts (two windows), A creates and saves a title, B (opened before A's save) types a paragraph and saves | B got the conflict dialog showing A's title; Keep mine → saved, API version 3 with B's paragraph in the body |
| Author: on the published post, add a sentence, `⌘S`, open Revisions, Restore this version (the default: the second-newest) | "Version restored" toast; the stored body no longer has the sentence; version 4; newest revision note `restored from <id>` |
| Password reset: Forgot password? → login `contributor` → Send reset link | "Check your e-mail"; the sink received "[My Site] Password reset" for `contributor@example.test`; the link's page showed "Choose a new password" with no `token` left in the URL; Set password → "Password changed"; signing in with the new password reached the Dashboard |

Also seen:

- Browser console errors in the runs were only the expected ones: `401` from the session
  probe on each sign-in page load and the `412` of the conflict save (Chrome logs failed
  loads as errors).
- Run 2 failed the final sign-in with `429`: it was the 11th sign-in from one address within
  a minute (`login:ip` is 10/min, 07 §2). Run 3 waits 61 s before the reset step. A real user
  does not hit this; it is the documented limit.
- **Finding (M2b-2 follow-up):** a probe step pressed `⌘⇧P` right after the first save moved
  a new item from `/new` to `/$id`; 1.5 s later no publish dialog was open (the dialog, or the
  key press, went with the unmounting `/new` screen). Waiting for "Saved" first works (the
  schedule step). Nothing is lost but the key press.
- `serve` stopped on SIGTERM with exit code 0.
- The scratch data folders under the session scratchpad could not be deleted (`rm -rf` is
  blocked in this environment); they hold only the test sites.

## 4. Contact sheet against 11 §8

Reviewed: every screen of the final E2E run (`admin/test-results/screens/`: 107 screens, 429
PNGs; `shell-focus-nav-dark` has only its dark variants by design), as 4-up composites
(light/dark desktop, light/dark mobile; first viewport), with full-size views where a
composite looked wrong. The per-task folders `shots/task-7` … `task-17` are older captures of
the same screens; the final run supersedes them.

| 11 §8 item | Result |
|---|---|
| 4 px grid | no misalignment seen |
| One primary action per view | holds per view, with one ruling: the top bar's "New" split button (filled until the final fix wave) appears with a screen's own primary (for example "New entry" on a list, "Publish…" in the editor). Ruling R20 (Task 18, confirmed by the final review): the top-bar New is global chrome, not the view's primary action — 11 §7's shell has it on every screen — and since the final fix wave it is an outline control, so each view has one filled primary |
| Consistent page header | yes (title with count, description, actions, tabs) on dashboard and lists; the editor uses its own bar by design (11 §7) |
| Loading, empty, error, permission-denied states | present: list skeletons, dashboard empty and error cards, search empty, trash empty, forbidden, not-found, error boundary, editor read-only and content-error |
| Dark mode at parity | yes on every screen; `no_raw_colours_outside_tokens` passes in the unit suite |
| Mobile without horizontal scroll, ≥ 44 px targets | `snap` asserts no horizontal overflow on every shot (passed); targets looked right, measured only by the earlier tasks' tests |
| Text truncated only with a tooltip or `title` | truncated titles and slugs seen in lists, palette and drawer; their `title` attributes were checked in the earlier task reviews, not again here |
| Numbers tabular | counts, tiles and the diff chips look tabular; not measured |
| Focus visible | `shell-focus-nav`, `shell-focus-top-bar` show the 2 px ring |
| Motion within §5 | not judged from stills |
| `expectAccessible` and the guard on every visit | every spec passed (axe WCAG 2.2 AA tags in both schemes, no CSP violation, page error or `console.error`) |

Fixed here: the two list shots that caught rows mid-animation (`a716181`, `f2ee8a2`).
Recorded, not fixed: on phones the status tabs scroll so that a tab next to the active one
shows only its count ("1") at the edge under the scroll fade (`content-list-trash` mobile) —
the fade signals more; acceptable, polish for M2b-3.

## 5. Bundle sizes (baseline for the M2b-3 budget)

`pnpm -C admin build` on the final tree (Vite 8.3.4, `assetsInlineLimit: 0`, no source maps).
Raw bytes and `gzip -9` bytes (Vite's own report uses its default level, so its gzip figures
differ slightly):

| What | Raw | gzip -9 |
|---|---|---|
| Entry `index-*.js` | 296 697 | 92 323 |
| Entry + the 15 modulepreloaded chunks `index.html` names (what the first page loads, sign-in included) | 642 779 | 208 013 |
| (same, before `f5afa00`: 18 preloads incl. the editor) | 1 203 828 | 384 998 |
| Main stylesheet `index-*.css` | 166 083 | 25 603 |
| Editor chunk `editor-*.js` (TipTap; lazy, editor routes only) | 619 303 | 187 232 |
| Editor stylesheet `editor-*.css` | 6 697 | 1 941 |
| Chart chunk `chart-*.js` (Recharts; lazy, dashboard) | 361 961 | 104 240 |
| Styleguide chunk (lazy, `/admin/_styleguide`) | 127 792 | 34 769 |
| Fonts: 20 woff2 (Inter 14: 710 640; Geist Mono 6: 70 448), each a unicode-range subset, so a page loads only the subsets it uses | 781 088 | — |
| Whole `admin/dist` (93 files, embedded in the binary) | 3 344 933 | — |

P3's "about 0.8 MB of fonts" holds (781 088 bytes).

## Verified (claims in this task's doc changes)

| Claim (file) | How verified | Result |
|---|---|---|
| README: the server, the REST API and the first admin screens are built; the importer is not (M3) | `ferrite --help` lists no import command; the screens above | true |
| README / CLAUDE.md: `pnpm -C admin install && pnpm -C admin build` before `cargo build`; without it a placeholder page | `admin/README.md`, `crates/ferrite-http/build.rs` watch rule (07 §3), `admin::placeholder_without_dist` and `site::admin_placeholder_on_real_binary` passing in §1 | true |
| CLAUDE.md: Node >= 24.18, pnpm 10.28.2 | `admin/package.json` `engines`, `packageManager` | true |
| CLAUDE.md: `pnpm -C admin dev` is Vite on `:5173` under `/admin/`, proxying `/api` and `/_preview` to `FERRITE_DEV_URL` (default `http://localhost:8080`) | `admin/vite.config.ts` (`process.env.FERRITE_DEV_URL ?? "http://localhost:8080"`, `port: 5173`, `proxy: { "/api", "/_preview" }`), `admin/README.md` | true |
| CLAUDE.md: the admin is at `/admin/` on the site | §3 | true |
| 07 §3: palette search debounced 150 ms, up to 8 hits per type; nav from `me.capabilities` | `palette/sources.ts` (`SEARCH_DEBOUNCE_MS = 150`, `SEARCH_LIMIT = 8`), `shell/nav.ts` | true |
| 07 §3: revisions drawer lists up to 50, `mod+alt+r`, Undo rules | Task 17 report and `api/content.ts` `MAX_REVISIONS`; E2E `revisions.spec.ts` passing | true |
| 10 §1: harness accounts include `totp`; workers 2, 1 retry on CI, `chromium-pg` runs `*.smoke.spec.ts` | `admin/e2e/support/users.ts`, `admin/playwright.config.ts` | true |
| 01 §1: `admin/` is one pnpm package, not a workspace | no `pnpm-workspace.yaml` in the repo or `admin/` | true |
| rulings P3 and P7 numbers (fonts 781 088 bytes; chart 361 961 / 104 240) | §5 | true |

## Not verified (limits)

1. **Safari.** Nothing was run in Safari or WebKit. For the user to do by hand on the
   release build: the six flows of §3 in Safari (sign-in per role, write and publish,
   schedule, a conflict between two windows, restore a revision, reset by e-mail), plus the
   reset page's token stripping and the `⌘K`/`⌘⌥R` shortcuts (R18: Option on macOS).
2. **`docker build`.** The Docker Desktop engine on this machine did not answer
   (`docker info` gave nothing within 20 s; earlier `docker version` calls from previous
   days were still hanging; they were killed). The image's `admin` stage
   (`node:24-bookworm-slim`, `pnpm install --frozen-lockfile`, `pnpm build`, copied into the
   Rust stage) and `.dockerignore`'s `admin/` entries were read, not built. The M2b-1 ledger
   records no `docker build` since Task 6 added the admin stage (ledger, Task 6: "Docker build and actionlint
   not run"), so **the admin stage has not been built as far as recorded**; CI's
   `docker` job may be its first run. To check by hand: `docker build -t ferrite-exit . && docker run --rm -p
   8080:8080 …` then `curl -i http://localhost:8080/admin/` (expect 200 with the admin CSP),
   then remove the image.
3. **The flows ran headless**, scripted, one machine, SQLite only, in Chrome on macOS. The
   Postgres site is covered by the smoke project only (P22); Firefox and WebKit are M2b-3.
4. **Contact sheet**: judged from screenshots; motion, touch-target sizes, tabular figures
   and truncation `title`s were not measured again here (§4).
5. **Parked review items stay open** (ROADMAP M2b-2/M2b-3 follow-ups): the restore banner's
   Discard Undo toast not dismissed on unmount; `storeKept`'s extra PUT; the lingering
   "Restoring…" toast; AltGraph for Option on
   macOS (R18); M5's `null`-valued custom field; M6/M7 untested; edits after Take theirs
   guarded only by `beforeunload` (R13; a local backup is the follow-up); core validating
   `body`'s root is `doc` on write; a palette snippet for body-only matches; a server theme
   hint against the one-frame flash (11 §4).
6. **The `/new` → `/$id` publish-shortcut race** (§3) is recorded, not fixed.
7. **CI has not run on these commits** at the time of writing.

## Addendum: workspace rerun on the final tree

After the last admin change (`f2ee8a2`) and a fresh `pnpm -C admin build`, the "with dist"
run was repeated so the embedded build is known to be the final one:

```
$ CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace --no-fail-fast
  79 test result lines, 2298 passed, 0 failed, 1 ignored; no SKIP
```

## Final fix wave (after the whole-branch review)

The final review (`.superpowers/sdd/2026-10-25-m2b1-foundation-and-writing/final-review.md`)
asked for one fix wave before merging. Commits on `m2b-admin` after `4eb4874`:

| Commit | What |
|---|---|
| `616571d` fix(admin): clear the query cache when another account signs in on the tab | I1: `startSession` (`auth/session.ts`) records whose data the cache holds and clears it when login, TOTP login or the guard's `me()` brings a different user id; the same user keeps it, and `pendingDrafts` (per user) is untouched, so a re-login still returns to the draft. Unit tests plus the E2E `another_account_on_the_same_tab_sees_none_of_the_first` (fails without the fix: B saw A's draft count). |
| `889e1b0` fix(admin): a revision restore locks editing before waiting for a save in flight | `restoreRevisionNow` makes body and title read-only before awaiting the in-flight save or transition (writes stay open so that transition still runs); unit test. |
| `f8d458a`, `5c4cefe` fix(admin): mod+s … | `⌘S` does nothing under a modal dialog (the conflict dialog: no second PATCH) but still fires there, so the browser's Save page never opens; popovers and the settings sheet still save. Unit test. |
| `ebdf3fd` chore(admin): drop the unused kit.table.keyboardHint key | — |
| `ffab788` style(admin): top-bar New is an outline control | R20. |
| `203d87a` style(admin): Recent drafts card shrinks to its list | No empty block beside the chart with one or two drafts. |

Gates after the wave: admin lint, typecheck, 264 unit tests, build; E2E 77/77 on SQLite and
the Postgres smoke 3/3 against a fresh release build. No Rust changed, so the workspace
run above stands. Still not verified: Safari, the Docker admin stage, and CI on the branch
(items 1, 2 and 7 above).
