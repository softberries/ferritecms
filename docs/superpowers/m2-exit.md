# M2 exit — API, auth and admin

**Criterion** (plan Task 16, "M2 exit"): *the M2 checklist from ROADMAP §M2 and 07 (every
endpoint and screen in its as-built state), the open follow-ups carried to M3, M4 or Later,
and what was never verified (Safari unless done by hand, real S3 clusters if not run).*

**Result (2026-10-27, branch `m2b3-admin` at `4e695da`, plus this docs commit; updated
2026-10-28 for the M2b-3 exit's part B):** every M2
checklist item in the ROADMAP is ticked (76 of 76), every REST resource but Import (M3) is
built (106 operations on 73 paths in `crates/ferrite-http/openapi.json`), and every admin
screen but Import (M3) is built, System without backups (M4). The last sub-milestone's exit
([`m2b3-exit.md`](m2b3-exit.md)) is verified in two parts (M2b R76): part A on 2026-10-27,
part B on 2026-10-28 (the four E2E projects twice in a row on the final code, the contact
sheet, `docker build` serving `/admin/`, a Linux Chromium run and actionlint; it found and
fixed two product bugs and needed ten test-harness fixes). Safari and real S3 were never verified
in M2 (§5).

## 1. Sub-milestones and their exits

| Sub-milestone | Exit document | Date | Result as recorded there |
|---|---|---|---|
| M2a-1 REST API core and auth | [`m2a1-exit.md`](m2a1-exit.md) | 2026-10-23 | met; the manual session on SQLite only |
| M2a-2a extensions and the plugin platform | [`m2a2a-exit.md`](m2a2a-exit.md) | 2026-10-24 | met; the manual session on SQLite only |
| M2a-2b media, comments, system, hardening | [`m2a2b-exit.md`](m2a2b-exit.md) | 2026-10-25 | met; one flaky CLI test parked |
| M2b-1 admin foundation and writing | [`m2b1-exit.md`](m2b1-exit.md) | 2026-10-26 | met in Chrome; Safari and `docker build` not verified |
| M2b-2 media and content modelling | [`m2b2-exit.md`](m2b2-exit.md) | 2026-10-26 | met in Chrome; Safari not verified; one E2E race recorded (fixed in M2b-3, R71) |
| M2b-3 administration and extensions | [`m2b3-exit.md`](m2b3-exit.md) | 2026-10-27/28 | met: part A (flows over the API, workspace tests, gates, bundle budget), part B (four E2E projects twice, contact sheet, `docker build`, Linux Chromium, actionlint); Safari and CI not verified |

ROADMAP §M2 checkboxes (counted with a script over `docs/ROADMAP.md` from "## M2" to
"## M3", nested items included):

| Section | Ticked | Open |
|---|---|---|
| M2a-1 | 10 | 0 |
| M2a-2a | 10 | 0 |
| M2a-2b | 20 | 0 |
| M2b-1 | 4 | 0 |
| M2b-2 | 7 | 0 |
| M2b-3 | 21 | 0 |
| Done from the earlier M2 deferred lists | 4 | 0 |

## 2. REST API as built

From `crates/ferrite-http/openapi.json` (OpenAPI 3.1.0), grouped by the first path segment
under `/api/v1`; the "Since" column is 07 §1's status column. 07 §1 has each resource's
rules and its core capability check.

| Resource | Operations | Since |
|---|---|---|
| `auth` (login, TOTP login, logout, me, password, password reset and confirm, sessions list / revoke / revoke others, TOTP setup / confirm / disable) | 13 | M2a-1; reset by e-mail M2a-2a; sessions M2b-3 |
| `content` (refs, list, create, counts, get, delete, patch, autosave get / put / delete, comment submit, publish, restore, revisions list / get / restore, schedule, submit, unpublish) | 19 | M2a-1; `author`, `allowed`, counts, autosave M2b-1; `seo`, body root check, `refs` M2b-2 |
| `types` | 5 | M2a-1; relation `types` M2b-3 |
| `taxonomies` and `terms` | 7 + 3 | M2a-1 |
| `menus` | 4 | M2a-1; `locations` M2b-2 |
| `users` (list, create, get, delete, patch, set password, tokens list / create / revoke, TOTP disable) | 10 | M2a-1; filters, `current_password`, `totp/disable` M2b-3 |
| `redirects` (list, create, export, import, get, delete, patch) | 7 | M2a-1; `q` M2b-2 |
| `settings` | 2 | M2a-1 |
| `media` (list, upload, get, delete, patch, restore) | 6 | M2a-2b; `q`, `kind`, `trash`, restore M2b-2; restore keeps the status M2b-3 |
| `comments` (list, counts, delete, patch) | 4 | M2a-2b; `item`, counts M2b-2 |
| `themes` (list, upload, activate, settings get / patch) | 5 | M2a-2a; manifest facts M2b-3 |
| `plugins` (list, install, inspect, retained, get, delete, disable, enable, metrics, purge, settings get / patch) | 12 | M2a-2a; metrics M2a-2b; facts, retained, purge M2b-3 |
| `system` (audit, jobs, job retry, test mail, metrics summary, nodes, status) | 7 | M2a-2b; audit filters, `mail`, typed node `meta`, test mail M2b-3 |
| `meta`, `openapi.json` | 1 + 1 | M2a-1 |
| **Total** | **106** | |
| Import | — | M3 (07 §1 "Import \| M3") |

The spec is checked against the code by `openapi_snapshot_is_current` and against the
admin's generated types by `pnpm -C admin api:check` (both pass, `m2b3-exit.md` §2–§3). The
exit flows of `m2b3-exit.md` §1 called the M2b-3 additions on a release build (sessions
aside: the TOTP login and logout were called, the session list was not).

## 3. Admin screens as built

From 07 §3 "Screens (M2)" (each row there holds the screen as built, task by task):

| Screen | Status |
|---|---|
| Sign-in and account recovery | Built (M2b-1) |
| Shell, palette, shortcuts | Built (M2b-1; M2b-3 additions: the Administration section, the body-match snippet, the theme cookie) |
| Dashboard | Built (M2b-1) |
| Posts / Pages / custom types | Built (M2b-1) |
| Editor | Built (M2b-1); media, SEO and custom fields (M2b-2); relation target types (M2b-3) |
| Media library | Built (M2b-2) |
| Content types | Built (M2b-2; relation "Can point at" M2b-3) |
| Taxonomies | Built (M2b-2) |
| Menus | Built (M2b-2) |
| Comments | Built (M2b-2) |
| Redirects | Built (M2b-2) |
| Account | Built (M2b-3) |
| Users | Built (M2b-3) |
| Settings | Built (M2b-3) |
| Themes | Built (M2b-3) |
| Plugins | Built (M2b-3) |
| System | Built (M2b-3); backups M4 (M2b P25) |
| Import | Planned (M3, with the import API; M2b P25) |

The screens run under the admin CSP (`script-src 'self'`, no `'unsafe-eval'`), with every
user-visible string through `t()`, axe (WCAG 2.2 AA) in both schemes on every E2E visit and
the guard against CSP violations, page errors and `console.error` (10 §1). Their E2E specs in
all four projects are built (Task 14) and passed twice in a row on the final code
(`m2b3-exit.md` §5.1).

## 4. Open follow-ups, carried

Each is in the ROADMAP under the milestone named.

- **M3:** redirects for `permalink.*` structure changes (a permalink change moves every URL
  without a redirect; the Settings screen says so, M2b P58) and `Idempotency-Key` on the
  import upload (M2a-2b P26); the import job's timeout, lock and event batching (M1.5).
- **M4, new from M2b-3:** plugin hook latency across the cluster as a time series (M2b P48);
  an index for the redirect `q` search if needed (M2b R43); an E2E of the Users screen's
  "Send a reset e-mail" (M2b R56); the M2b-3 final review's six minors (theme `homepage`
  through `web_url`, plugin `mail_prefix` vs core `subject_title`, site-title read failure
  in reset/test mail vs plugin mail, blank `q` on `/users` vs `/redirects`, the first run
  of the rebuilt `ci.yml`, `check_relations` revealing the type of an unreadable item); and
  Task 16's harness minors (the WebKit fetch rule counting SPA navigations, `MEDIA_FILE`
  letting any `/_media/` 404 pass in two specs, the cmdk workaround's nested `useCallback`
  refs, a Vitest test near its 5 s timeout under load); and the contact sheet's open items
  (`m2b3-exit.md` §5.3): `icon-sm`/`icon-xs` buttons stay under 44 px on touch screens
  (11 §8 asks 44 px; WCAG 2.2 AA's 24 px is met), dark dropdown menus have a faint edge,
  and the palette's stale results, dimmed to 60 % while a slow search runs, fall below
  4.5:1.
- **M4, from M2a:** the per-node option cache, a GC grace period for staged artifacts, a
  compile bound for uploaded components, fairness in the per-render plugin budget, signed
  plugins and publisher identity (M2a-2a); `date` filter time zones and `seed demo`
  atomicity (M2a-2b P27); `ferrite media regenerate` and optional location-metadata
  stripping (M2a-2b P8, P27, R11); the metrics registry and dashboard page views (M2a-2b P20,
  M2b P16).
- **Later:** multi-host sites (M2a P27); passkeys (M2a P3); TOTP recovery codes and a user
  agent per session (M2a P9, M2b P43, P45); comments on pages and custom types, per-item
  closing, comment hooks and admin notification mails (M2a-2b P29); more upload types and
  `tel:` links (M2a-2b P29, P21); an atomic KV increment (M2a-2a R19).
- **Owed before merging M2b-3** (not a ROADMAP item): the first CI run (M2b R72).

The accepted limits of each sub-milestone are listed under it in the ROADMAP ("Known
limits") and in the rulings files.

## 5. Never verified in M2

1. **Safari.** No M2 exit ran Safari (M2b-1 R19; M2b-2; M2b-3). Linux WebKit in CI is not
   Safari (P55). For the user, by hand, on a release build: the flows of the three M2b exits,
   including `⌘⌥R` (revisions) on a real Mac (R18), drag and drop of files and folders, and
   the keyboard drags.
2. **Real S3 and real S3 clusters.** Every S3 run in M1–M2 used an `s3s` server
   (in-process in the tests, standalone `s3s-fs` in the M1 exit); MinIO was used nowhere
   (`m15-exit.md`), and no run used AWS S3 or another real provider. Direct S3 mode
   (`public_base_url`) was never run (`m2a2b-exit.md`). The cluster example
   (`examples/cluster/`, `docker compose` with MinIO and Caddy) was validated by config
   checks only, never run end to end (ROADMAP M4, M1.5 P24).
3. **`docker build` serving `/admin/`** never ran on a local engine (M2b-1: engine
   unresponsive; M2b-2: not run) until M2b-3's part B, where it built and served the SPA
   at `/admin/` (`m2b3-exit.md` §5.4). CI's `docker` job builds the image; its results
   could not be read from this host (no connection to `api.github.com`), so they are not
   checked here.
4. **An `https` site.** Every exit ran on a plain `http://` loopback address (`127.0.0.1` or
   `localhost`), so the `Secure` cookie
   attributes (session, and the admin theme cookie on https) were seen only in tests
   (`m2a1-exit.md`).
5. **Two `ferrite serve` processes driven by hand** in M2: the manual and scripted
   sessions all ran one node on SQLite; multi-node behaviour is covered by the in-process
   and two-process cluster tests (`FERRITE_TEST_PG_URL`).
6. **CI on the M2b-3 branch**, and the E2E matrix on Linux beyond `chromium-sqlite` on
   arm64 (`m2b3-exit.md` §5.5): CI's x86_64 runners, Linux Firefox and Linux WebKit.

## Verified (claims in this document)

| Claim | How verified | Result |
|---|---|---|
| 76 of 76 M2 checkboxes ticked; per-section counts | a Python count of `- [x]` / `- [ ]` lines between "## M2" and "## M3" in `docs/ROADMAP.md` after this task's edits | true |
| 106 operations on 73 paths; per-resource counts | a Python count over `paths` in `crates/ferrite-http/openapi.json` | true |
| The "Since" column | 07 §1 "Resources" status column | true (copied) |
| Screen statuses | the status column of 07 §3 "Screens (M2)", printed with a script | true |
| Sub-milestone dates and results | the head of each exit document | true (copied) |
| MinIO used nowhere; S3 runs on `s3s` | `m1-exit.md` §"MinIO → s3s-fs", `m15-exit.md` limit 5 | true as recorded there |
| Direct S3 mode never run | `m2a2b-exit.md` limit 2 | true as recorded there |
| No `https` site | `m2a1-exit.md` limit 2; `grep -- "--url"` over the six M2 exit documents: only `http://127.0.0.1:…` and `http://localhost:…` | true |
| CI results not readable here | `gh run list` → "error connecting to api.github.com" | true |
| Follow-ups carried | the ROADMAP's M3, M4 and Later lists after this task's edits | true |
