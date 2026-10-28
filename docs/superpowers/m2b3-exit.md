# M2b-3 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M2b-3; plan Task 16): *a fresh `init` + `seed demo` +
`serve` on a release build: sign in, enrol TOTP with a real authenticator code (computed),
sign out and back in with it; create a token and use it with `curl`; create an editor and
sign in as them; change settings and see them on the site (title, front page, permalinks);
pack and upload a theme, activate it, customize it; install the example plugin through the
permission report, enable it, set a secret, see its metrics, delete it with and without
erasing data; retry a dead job; filter the audit log; send a test mail through a real SMTP
catcher; the workspace tests on both backends with and without `admin/dist`; the admin gates
and the four E2E projects; the contact sheet against 11 §8; the bundle budget; `docker build`
serving `/admin/`, if a local engine runs.*

**Result, part A (2026-10-27, branch `m2b3-admin` at `4e695da`, plus this docs commit):** every
flow passed over the REST API on a fresh release build, the workspace tests passed on both
backends with and without `admin/dist`, and the admin gates without E2E, the bundle budget,
`fmt` and `clippy` passed. No flow turned up a product bug. Part B waited for a host reboot
(ruling R76: no browser engine or Docker daemon started on the host, the fault recorded in
Task 14).

**Result, part B (2026-10-28, after the reboot; §5):** the four E2E projects passed twice in a
row on the final code (`e2e:all` runs 20 and 21 at `22d3272`: 764 passed each, 0 failed),
after twenty-one runs in all. They found two product bugs (fixed with unit tests: a command
list's `aria-activedescendant` naming a removed option, `1807f2f`; a token scope label
truncated without a title, `d767108`) and needed ten test-harness fix commits (`8c8b493`,
`5fd046d`, `2d2d78e`, `b33bb1a`, `816bbb5`, `7fb1f77`, `43f27a6`, `11d9ba9`, `107b2f9`,
`22d3272`); two failures from outside the repository were not fixed (a Playwright tracing
race; WebKit ending its own Networking process under host load). The contact sheet was checked against
11 §8 (one deviation fixed, four listed); `docker build` of the repo Dockerfile serves the
admin SPA at `/admin/`; `chromium-sqlite` passed on Linux in the Playwright 1.64.0 image
(191 of 191); actionlint 1.7.12 reports nothing. Task 14's two assumptions about WebKit's
cancelled fetches turned out false and the guard was changed (§5.2).

| Exit check | Verified by | Result |
|---|---|---|
| 1. The flows on a release build | §1: `curl` against the REST API of a fresh site, a computed RFC 6238 code, a Python SMTP catcher | yes, over the API (the screens themselves: E2E, row 4) |
| 2. Workspace tests, both backends, with and without `admin/dist` | §2: two full runs with `CI=true` and PostgreSQL 17 | yes: 2 458 passed, 0 failed, each run |
| 3. Admin gates (`lint`, `typecheck`, `test`, `api:check`, `build`), `fmt`, `clippy` | §3 | yes |
| 4. The four E2E projects (`pnpm -C admin e2e:all`), twice in a row, with flake counts | §5.1 | yes: runs 20 and 21 at `22d3272`, 764 tests each (191 per project), 764 passed and 764 passed, 0 failed; every earlier failure is listed with its cause |
| 5. Contact sheet against 11 §8 (with the account screens' snaps re-checked) | §5.3 | yes: all 228 snaps (909 images) looked at; one deviation fixed, four listed |
| 6. Bundle budget | §4 | yes: every group under its limit |
| 7. `docker build` serving `/admin/` | §5.4 | yes: Docker 29.8.1, the SPA (not the placeholder) at `/admin/` |
| 8. A Linux Chromium run (the CI engine's platform) | §5.5 | yes: `chromium-sqlite` 191 of 191 in `mcr.microsoft.com/playwright:v1.64.0-noble` (arm64, not CI's x86_64) |
| 9. actionlint on `.github/workflows/ci.yml` after Task 14's fix round | §5.6 | yes: `rhysd/actionlint` 1.7.12, exit 0, no output |
| 10. Safari, with `⌘⌥R` on a real Mac | — | **not verified**: the user's manual check (P55, R18) |
| 11. CI on GitHub | — | **not run**: no push in M2b-3 (R72); it runs on the final develop→main pull request |

Part A changed no code. Part B's code changes are the fix commits in §5 (admin and its E2E
harness only; nothing under `crates/` or `plugins/`). Docs changed: this file, [`m2-exit.md`](m2-exit.md),
`docs/superpowers/rulings/m2b.md` (P57's cookie path as built, R18 as updated, the M2b-3
pre-flight Rulings A–C and controller rulings R37–R76), ROADMAP (M2b-3 ticked, the M2
status, follow-ups carried to M4 and Later), CLAUDE.md (status, `theme pack`), README
(status line), 01 §1 (crates, admin layout, example plugins) and §2 (HTTP and Mail rows),
07 §3 (the Shell, Dashboard, Editor, Media and Content types rows as built in M2b-3), 10 §1
(the M2b-3 specs and support files) and 11 §6 (key sequences, Option and AltGraph) and §8
(snapshots in `chromium-sqlite` only); in part B, 10 §1 (the support modules and unit files)
and §3 (the WebKit guard rules, the reload retry and the part B test changes).

## 1. The flows (over the REST API)

macOS 26.6 (Darwin 25.6.0) arm64, `rustc` 1.99.1, Node 24.18.0, pnpm 10.28.2, Python 3.14.0.
`target/` had been cleaned before the task. Build, from `4e695da`:

```
$ pnpm -C admin install --frozen-lockfile && pnpm -C admin build     # check-dist: ok
$ cargo build --release -p ferrite                                   # Finished `release` profile in 2m 01s
$ ./target/release/ferrite --version                                 # ferrite 0.1.0
$ cargo build --release --target wasm32-wasip2 -p admin-e2e          # in plugins/; admin_e2e.wasm, 91 562 bytes
```

Site (all paths under the session scratchpad, `$SP`; left in place):

```
$ ferrite init --data $SP/site --url http://127.0.0.1:18080
  applied migrations [1, 2, 3, 4, 5, 6, 7, 8] from=0 to=8; created a new site key
$ ferrite seed demo --data $SP/site
  Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
$ printf '[mail]\nsmtp_url = "smtp://127.0.0.1:2525"\nfrom = "Exit Site <noreply@exit.test>"\n' >> $SP/site/ferrite.toml
$ echo '<password>' | ferrite user reset-password --data $SP/site --login admin --password-stdin
  password of admin reset; its sessions were ended
$ ferrite config check --data $SP/site
  ok: mode=solo database=sqlite media=local workers=14 mail=configured
$ python3 -I smtp_catcher.py 2525 $SP/mail &          # a 43-line asyncio SMTP catcher (scratchpad)
$ FERRITE_SERVER__LISTEN=127.0.0.1:18080 ferrite serve --data $SP/site &
  Listening on http://127.0.0.1:18080
```

`GET /admin/` answered `200 text/html` with the admin CSP (`script-src 'self'`, no
`'unsafe-eval'`), `Referrer-Policy: no-referrer`, `Cache-Control: no-cache` and `Vary:
accept-encoding, cookie`. With `Cookie: ferrite_admin_theme=dark` the page began `<html
class="dark" lang="en">`, with `light` `<html class="light" lang="en">`, and with `system` or
`bogus` `<html lang="en">` (P57).

Every API call below sent `Origin: http://127.0.0.1:18080`, the session cookie and, on
writes, `X-CSRF-Token` (a helper `call <who> <METHOD> <path>` in the scratchpad).

| Flow | Commands (abridged) | Seen |
|---|---|---|
| Sign in | `POST /auth/login {"login":"admin",…}` | `200`, `status: "session"`, `totp_enabled: false`, `mail_configured: true`; `Set-Cookie: ferrite_session=…; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000` |
| TOTP enrolment | `POST /auth/totp/setup`; a code computed from `secret_base32` with RFC 6238 in Python (`totp.py`; the RFC's SHA-1 test vector, T=59 → `94287082`, reproduced first); `POST /auth/totp/confirm {enrollment, code, password}` | setup `200`, a 32-character base32 secret, `otpauth://totp/FerriteCMS%20Demo:admin?secret=…&issuer=FerriteCMS%20Demo` (the issuer is the seeded `site.title` option, not `ferrite.toml`'s "My Site": P51); confirm `204` |
| Sign out, back in with TOTP | `POST /auth/logout`; `GET /auth/me`; `POST /auth/login`; wait for the next 30 s step (a step is accepted once); `POST /auth/login/totp {challenge, code}` | logout `204`, then `/auth/me` `401`; the login answered `200` `{"status":"totp_required","challenge":"v1.…"}`; the code login `200`, `status: "session"`, `totp_enabled: true`; `/auth/me` `200` |
| Token with `curl` | `POST /users/{admin}/tokens {"name":"exit curl","scopes":["content:read","system:read"],"expires_at":"2026-11-26T00:00:00Z"}`; then `curl -H "Authorization: Bearer fer_…"` | `201` with the value (`fer_…`, 47 characters) once; `GET /content/post?limit=2` `200` (2 items, "Hello world", "Release notes 0.1"); `GET /system/status` `200`; `POST /content/post` `403`; `GET /users` `403`; `GET /users/{admin}/tokens` lists it with `last_used_at` set and no value |
| An editor, signed in | `POST /users {"login":"edna","role":"editor","password":…,"must_reset":true}`; `POST /auth/login` as edna; `POST /auth/password {current, new}`; log in again | create `201`; first login `200` `must_reset: true`, and `GET /content/post` `403` in that session; the password change `200` with a new `csrf_token`; the new login `200`, `role: editor`, `must_reset: false`, nine capabilities (`read_admin` … `moderate_comments`, no `manage_*` beyond terms and menus); `GET /content/post` `200`, `GET /users` `403`, `PATCH /settings` `403`, `POST /content/post` `201` |
| Settings on the site | `PATCH /settings {"site.title":"Harbour Exit Site","reading.front_page":"<About's id>","permalink.post":"/%year%/%month%/%slug%/"}` | `200` with the three values; 1.5 s later `/` had `<title>About – Harbour Exit Site</title>` and `<h1 class="entry-title">About</h1>`; `/hello-world/` `404`; `/2026/07/hello-world/` `200` with `<title>Hello world – Harbour Exit Site</title>` (published 2026-07-20) |
| Theme: pack, upload, activate | a copy of `themes/blog` renamed `harbour`, version 0.2.0; `ferrite theme pack --dir $SP/harbour --out $SP/harbour.tar`; `POST /themes?activate=false` (`Content-Type: application/x-tar`, the file as the body); `POST /themes/harbour/activate`; 2 s later `GET /themes` | pack: `packed 21 files, 52224 bytes, sha256 873e38d4…4691a5` (`shasum -a 256` agrees); upload `201`: name, version, author, licence, homepage, `has_settings: true`, two menus, two image sizes, `hash` = the file's SHA-256, `origin: installed`, `active: false`; activate `200`, `active: true`; the node then reported `harbour`, `origin: artifact`, `fallback: false` |
| Theme: customize | `GET /themes/harbour/settings`; `PATCH … {"accent_color":"#0f766e","show_author":false}` | defaults `#c2410c`, `true`, `true`; the `PATCH` `200` with the new values; the post page then carried `<style>:root{--accent:#0f766e}</style>` and lost its author byline (lines matching `byline|author` went from 3 to 2, the comment form's two) |
| Plugin: inspect | `POST /plugins/inspect` (multipart `manifest` = `plugins/examples/admin-e2e/ferrite-plugin.toml`, `component` = the `.wasm`) | `200`, fingerprint `63b8801c…f821c842`, six items: low "read published posts and pages", **high** "read every meta field of published content", low "store its own data on the site", **high** "send HTTPS requests to api.example.com", **high** "add tags to the head of every page, including scripts …", **high** "use up to 48 MiB of memory per call (site default 32 MiB)"; limit `memory` 50 331 648, `above_default: true`; `GET /plugins` still empty |
| Plugin: install, enable | `POST /plugins` with `approve` = 64 zeros; then with the fingerprint; `POST /plugins/admin-e2e/enable` | wrong approval `422`, `errors: [{field: "approve", detail: "the approval does not match this upload; inspect it again"}]` with the server's report (same fingerprint); install `201`, `enabled: false` (a new plugin arrives disabled); enable `200`, `enabled: true` |
| Plugin: secret | `PATCH /plugins/admin-e2e/settings {"api_key":"sk-exit-secret-123","greeting":"Ahoy"}`; `GET` again | before: `secrets: {api_key: false}`; after: `values.greeting: "Ahoy"`, `secrets: {api_key: true}`; the secret's text appears in neither response (`grep -c` 0 and 0); the public pages carry `<meta name="admin-e2e" content="Ahoy">` |
| Plugin: metrics | `GET /plugins/admin-e2e/metrics` | `200`: `window_secs: 86400`, `failures: []`, this node `since` its start, hook `render.head` with `calls: 1`, `failures: 0`, `buckets: [1,0,0,0,0,0]` (one call: the page cache served the rest) |
| Plugin: delete, keep data | `DELETE /plugins/admin-e2e`; `GET /plugins/retained`; inspect and install again; `GET …/settings` | `204`; retained lists `admin-e2e` with `deleted_at` and `purge_after` 30 days later; reinstall `201`; retained empty; the settings came back (`greeting: "Ahoy"`, `api_key` set) |
| Plugin: delete and erase | `DELETE /plugins/admin-e2e?purge=true`; retained; install again; settings; `DELETE …?purge=true` | `204`; retained empty (not listed, R67); reinstall `201`; settings at their defaults (`greeting: "Hello"`, `api_key` not set); final delete `204` |
| Dead job, retried | `POST /media` with a file that has the PNG signature and then text (`Idempotency-Key: exit-broken-1`); poll `GET /system/jobs?kind=media.variants&status=dead`; trash and delete the item (`DELETE /media/{id}`, then `?force=true`, each with `If-Match`); `POST /system/jobs/{id}/retry`; poll | upload `201` typed `image/png`; within 1 s the job was `dead`, `attempts: 1`, `max_attempts: 5`, `last_error: "image could not be processed: unsupported image: Format error decoding Png: … chunk appeared before IHDR chunk"`, `handled: true`; both deletes `204`; retry `200` `status: queued`, `handled: true`; then `done`, `attempts=1`; `GET /system/jobs?status=dead` empty |
| Audit filter | `GET /system/audit?…` | 18 entries in all at that point; `action=plugin.` 8 (`plugin.install` ×3, `plugin.enable`, `plugin.settings`, `plugin.delete` ×3); `action=user.` 3 (`user.password` ×2, `user.create`); `action=theme.activate` 1; `action=job.retry` 1; `actor=<edna>` 1 (`user.password` by edna); `target_kind=api_token` 1 (`token.create`); `action=plugin` (no dot: exact) 0; `target_id=x` alone `422` "target_id needs target_kind"; `actor=not-a-uuid` `400`. The `plugin.install` details hold the granted capabilities; `token.create` holds name, scopes and expiry, not the value |
| Test mail | `GET /system/status`; `POST /system/mail/test` four times | `mail: {configured: true, from: "Exit Site <noreply@exit.test>"}` (no SMTP URL); `202 {"to":"admin@example.com"}`; the catcher received `0001.eml` from `<noreply@exit.test>` to `<admin@example.com>`, subject "Test e-mail from Harbour Exit Site" (the title set above, P51), body "This is a test e-mail from Harbour Exit Site (http://127.0.0.1:18080). It was sent from the admin by "admin" at 2026-10-27 22:20:52 UTC …"; calls 2 and 3 `202`, call 4 `429` with `Retry-After: 540` (3 per 10 minutes); three `mail.test` audit rows; three mails in the catcher |

Also seen:

- One request in my script failed on my side: the editor's `POST /auth/logout` right after
  her password change sent the old CSRF token and got `403`; the change rotates it (07 §2).
- `serve` stopped on SIGTERM and logged "node stopped" and "shut down". Its log has one
  `WARN`, the dead job's failed attempt above, and no `ERROR`.
- These flows check the API the screens use, not the screens. The screens are exercised by
  their E2E specs (row 4, §5.1).

## 2. Workspace tests

macOS 26.6 (Darwin 25.6.0) arm64, `rustc` 1.99.1, PostgreSQL 17.11 (Homebrew) at
`localhost:54329` (CI uses `postgres:18`).

```
$ CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace
  with admin/dist:    exit 0; 82 test result lines, 2458 passed, 0 failed, 1 ignored; no SKIP
$ mv admin/dist $SP/dist-aside && touch crates/ferrite-http/build.rs     # then the same command
  without admin/dist: exit 0; 82 test result lines, 2458 passed, 0 failed, 1 ignored; no SKIP
  (ferrite-http recompiled; dist moved back and build.rs touched again afterwards)
```

The ignored test is the `ferrite-sdk` `export_plugin` doc test, as before. Both runs passed
`site::admin_index_on_real_binary`, `site::admin_placeholder_on_real_binary` and
`admin::placeholder_without_dist` (solo, cluster). M2b-2 ended at 2 377 passed
(`m2b2-exit.md` §1); M2b-3 adds 81. The schema version stays 8: both migration folders end
at `0008_autosave_seo`, and `git diff --stat e1cd3a5..HEAD` over them is empty.

## 3. Admin gates, `fmt`, `clippy`

On `4e695da`, Node 24.18.0, pnpm 10.28.2:

```
$ pnpm -C admin lint            # exit 0
$ pnpm -C admin typecheck       # exit 0
$ pnpm -C admin test            # 111 files, 652 passed
$ pnpm -C admin api:check       # src/api/schema.d.ts is current
$ pnpm -C admin build           # check-dist: ok (run at the start and again at the end)
$ pnpm -C admin bundle:check    # bundle:check: ok (§4; the same numbers both times)
$ cargo fmt --all --check                                       # exit 0
$ cargo fmt --all --check --manifest-path plugins/Cargo.toml    # exit 0
$ cargo clippy --all-targets -- -D warnings                     # exit 0
$ cargo clippy --target wasm32-wasip2 --all-targets -- -D warnings   # in plugins/, exit 0
```

`pnpm -C admin e2e` and `e2e:all` were not run in part A: no browser engine launched on the
host. They ran in part B (§5.1), where the gates above ran again at the end (§5.7).

## 4. Bundle budget

`pnpm -C admin bundle:check` after the build of §1 (gzip -9 bytes through zlib; limits from
`admin/bundle-budget.json`, measured at `3209291` plus 10 %, rounded up to KiB, P56):

| Group | Size | Limit | Headroom |
|---|---|---|---|
| `entry` (`index-*.js`) | 98 478 | 108 544 | 10 066 |
| `entry_preloads` (the entry and its 19 modulepreloads) | 238 098 | 262 144 | 24 046 |
| `css` (every stylesheet, main and lazy) | 29 881 | 33 792 | 3 911 |
| `editor` (`editor-*.js`, TipTap, lazy) | 197 383 | 218 112 | 20 729 |
| `chart` (`chart-*.js`, Recharts, lazy) | 98 625 | 108 544 | 9 919 |
| `largest_lazy` (the largest other chunk) | 28 098 | 31 744 | 3 646 |

`bundle:check: ok`. Against the M2b-2 numbers (`m2b2-exit.md` §5, `gzip -9` on the command
line): the entry `index-DujhI9p3.js` is 316 418 bytes raw, 98 496 with `gzip -9` (M2b-2:
306 918 / 95 474, +3.2 %); the main stylesheet 184 207 / 27 900 (M2b-2: 175 735 / 26 870);
the editor chunk 656 093 / 197 402 (M2b-2: 656 406 / 197 276). `index.html` names 19
modulepreloads (M2b-2: 17). `admin/dist` holds 217 files, 4 053 856 bytes (M2b-2: 172 files,
3 727 396).

## 5. Part B: browsers and Docker (2026-10-28)

The same host after a reboot: macOS 26.6 (Darwin 25.6.0) arm64, 14 cores, Node 24.18.0,
pnpm 10.28.2, `@playwright/test` 1.64.0 (Chromium headless shell 1248, Firefox 1555, WebKit
2370), PostgreSQL 17.11 at `localhost:54329`, Docker 29.8.1. Part A was committed first
(`46322c4`), then `cargo clean` (repo: 13.3 GiB, plugins: 517.1 MiB), then `pnpm -C admin
build`, `cargo build --release -p ferrite` and the `admin-e2e` wasm. Each code change below
was followed by a fresh admin build and release build before the next full run.

### 5.1 The four E2E projects

`FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test pnpm -C admin e2e:all`
(8 workers, no retries locally, so a test fails or passes; "flaky" cannot occur). Counts
per project are summed from the list reporter's lines (`[project]` and `[project-site-wide]`
together); each project has 191 tests.

| Run | Code | chromium-sqlite | chromium-pg | firefox-sqlite | webkit-sqlite | Time |
|---|---|---|---|---|---|---|
| 1 | `46322c4` | 191 | 191 | 190, 1 failed (a) | 191 | 9.7 min |
| 2 | `8c8b493` | 191 | 191 | 191 | 190, 1 failed (b) | 9.0 min |
| 3 | `8c8b493` (b's fix landed mid-run) | 191 | 190, 1 failed (c) | 191 | 191 | 8.8 min |
| 4 | `b33bb1a` | 191 | 191 | 191 | 191 | 9.4 min |
| 5 | `b33bb1a` | 191 | 191 | 191 | 191 | 9.4 min |
| 6 | `d767108` | 191 | 191 | 190, 1 failed (e) | 177, 14 failed (d) | 14.2 min |
| 7 | `d767108` | 191 | 191 | 190, 1 failed (e) | 191 | 8.6 min |
| 8 | `1807f2f` | 191 | 191 | 191 | 190, 1 failed (f) | 10.2 min |
| 9 | `1807f2f` | 175 so far | 93 so far, 1 failed (g) | — | — | stopped after 269 tests, f being fixed |
| 10 | `816bbb5` | 191 | 191 | 191 | 191 | 9.0 min |
| 11 | `816bbb5` | 190, 1 failed (h) | 191 | 191 | 191 | 8.9 min |
| 12 | `7fb1f77` | 191 | 191 | 191 | 191 | 8.8 min |
| 13 | `7fb1f77` | 191 | 191 | 191 | 191 | 8.9 min |
| 14 | `43f27a6` | 191 | 191 | 191 | 190, 1 failed (j) | 10.0 min |
| 15 | `43f27a6` | 175 so far | 103 so far | — | — | stopped after 278 tests, j being fixed |
| 16 | `11d9ba9` | 191 | 191 | 191 | 191 | 10.6 min |
| 17 | `11d9ba9` | 191 | 191 | 190, 1 failed (k) | 191 | 11.3 min |
| 18 | `107b2f9` | 191 | 191 | 191 | 190, 1 failed (l) | 9.2 min |
| 19 | `107b2f9` | 175 so far | 169 so far | 7 so far | — | stopped after 374 tests, l being fixed (the 19 failures in its log are all in the last 30 lines, written as it was stopped) |
| 20 | `22d3272` | 191 | 191 | 191 | 191 | 8.8 min |
| 21 | `22d3272` | 191 | 191 | 191 | 191 | 8.7 min |

Runs 4–5 and 12–13 were each two clean runs in a row, but code changed after them (the
contact-sheet fix, then i), and runs 16 and 18 were each followed or met by a failure (k,
l), so the pair that counts is 20–21 on the final code. Failures
and their causes:

- **(a)** `keyboard_focus_rings_in_the_shell`: "focus never reached the target". `tabTo`
  gave up after 20 Tab presses; the page snapshot shows two custom types from parallel
  tests in the sidebar, which put the top-bar search button 22nd. Test fix `8c8b493`: at
  most one press per focusable element.
- **(b)** palette `screens`, WebKit: two page errors "/127.0.0.1:59314/api/v1/content/post?q=…
  due to access control checks." (§5.2). Test fixes `5fd046d`, `2d2d78e` and `b33bb1a`.
- **(c)** `fill_every_field_kind_and_reload`, chromium-pg: the test body passed, then
  `context.close()` failed with "file data stream has unexpected number of bytes" and the
  trace zip was truncated. A race in Playwright's tracing (in `coreBundle.js`,
  `onContentBlobAppend` appends to a `resources/` file while the context's zip is written);
  not in this repository; not fixed.
- **(d)** run 6, WebKit: 14 tests timed out or saw "Target page, context or browser has
  been closed", in four bursts across workers. `~/Library/Logs/DiagnosticReports` has
  `ExcUserFault_com.apple.WebKit.Networking.Development` reports at 06:44 and 06:55 (also one
  at 05:59), each `is_simulated: 1`, `EXC_GUARD`, termination namespace `WEBKIT`: WebKit
  ended its own Networking process. `uptime` showed load averages 76 / 172 / 117 on 14
  cores (ESET's `scand` at 61 % CPU next to the browsers). Read as the host starved, not a
  test or product fault; not fixed. It did not recur in runs 7–21.
- **(e)** `relation_searches_its_types_and_picks_an_item`, Firefox, runs 6 and 7: axe
  `aria-valid-attr-value` on `#radix-_r_29_` and `#radix-_r_27_` (the relation picker's
  combobox and listbox). Logged in an instrumented run: `aria-activedescendant` named
  `radix-_r_2c_` while the selected option was `_r_2f_`, and after the options remounted as
  `_r_2i_`/`_r_2l_` with the same values it still named `_r_2c_`, an element that was gone.
  cmdk 1.1.1 (the latest release) reads that id only when the selected value changes, from
  the DOM, in a scheduled step. **Product fix `1807f2f`**: `Command` keeps both attributes on
  the option cmdk marks `aria-selected` (a MutationObserver on its root); test
  `active_descendant_follows_options_remounted_with_the_same_values` fails without it (the
  combobox had no matching attribute). The cmdk specs (palette, editor fields, content
  list, editor media, menus, taxonomies, users) passed in three engines after it: 162 of 162.
- **(f)** palette `screens`, WebKit, run 8: `page.goto("/admin/")` "interrupted by another
  navigation" to the chosen post, then "Importing a module script failed.". The spec went
  to the dashboard right after choosing a result; the editor's route loads its chunk first.
  Test fix `816bbb5`: wait for the item's address. 18 of 18 over 6 repeats in three engines.
- **(g)** `set_featured_image_from_picker`, chromium-pg, run 9: not diagnosed (run stopped,
  artifacts overwritten); 40 of 40 when `editor-media.spec.ts` was repeated 5 times on
  chromium-pg.
- **(h)** `forbidden_keeps_running_uploads`, chromium-sqlite, run 11: "Failed to load
  resource: … 404" for `/_media/<sha256>/tour-mvrrfi0j-d9c675e6.png`, the editor tour's own
  image, purged by that test while the library grid showed it. Test fix `7fb1f77`: in
  `media.spec.ts` and `editor-media.spec.ts` only, a stored media file's 404 is accepted.
  Both specs in four projects: 84 of 84.
- **(j)** palette `screens`, WebKit, run 14, on f's fix: `page.goto("/admin/")` "Frame load
  interrupted", and "console.error: TypeError: Importing a module script failed. " with
  React's component stack. The address had changed but the editor's route chunks were
  still loading. Test fix `11d9ba9`: both palette tests that open a result wait for the
  editor's body to be editable; and the guard trims that line, whose trailing space kept
  the import rule from matching (test
  `an_import_error_logged_with_its_component_stack_is_recognised`; the rule still needs a
  cancelled script on the page within 2 s). `palette.spec.ts` in three engines: 90 of 90
  over 5 repeats.
- **(k)** palette `screens`, Firefox, run 17 (`uptime` load 89 / 56 / 38 at its start): axe
  `color-contrast` on every row of `palette-results`. The palette keeps the previous
  query's rows at 60 % opacity while a slow round runs (`palette.tsx`, `slow &&
  search.isPlaceholderData && "opacity-60"`), and the spec's wait (the seeded post
  visible, no skeleton) is also met by those rows. Test fix `107b2f9`: also wait for the
  status to read the result count (shown only when no round is pending) and for the group
  to be fully opaque. Not reproduced on the idle host; 90 of 90 over 5 repeats in three
  engines. The dimmed rows themselves stay below 4.5:1 while a slow search runs; they are
  stale, still selectable, and listed under §5.3.
- **(l)** `set_featured_image_from_picker`, WebKit, run 18: axe `color-contrast` on the
  editor's save-state label. It crossfades with Motion's `AnimatePresence mode="wait"`
  (exit, then enter), and `settle` awaited only the animations running when it looked.
  Test fix `22d3272`: `settle` waits until no finite animation runs, within the same 5 s.
  Not reproduced on the idle host: the cause rests on code reading. A temporary probe spec
  saw the crossfade as a WAAPI `Animation` in all three engines.
  `editor-media.spec.ts` and `design.spec.ts` in four projects: 120 of 120 over 2 repeats.
  This may also have been run 9's undiagnosed failure (g), same test, not shown.
- **(i)** not an `e2e:all` failure: `FERRITE_BIN` broke the Linux run (§5.5). Test fix
  `43f27a6`; it changes nothing when `FERRITE_BIN` is unset, as in every local run.

Also seen once: `renders_the_shell_with_translated_title` (Vitest) timed out at 5.3 s (limit
5 s) in a full `pnpm test` right after a release build; 3 of 3 alone and 659 of 659 in two
full runs after.

### 5.2 Task 14's WebKit guard assumptions

Checked with temporary logging of the guard's deferred entries and of the page's `request`,
`requestfinished`, `requestfailed` and `framenavigated` events (not committed):

| Assumption (Task 14) | Seen | Result |
|---|---|---|
| A page error's `stack` starts with "Fetch API cannot load" | Playwright 1.64 (`wkPage` `_onConsoleMessage`) sets `stack` to `""` when WebKit sends no frames, and builds `name`/`message` with `splitErrorMessage`, cutting at the first `:` and dropping one character: name "Fetch API cannot load http", message "/127.0.0.1:…" | **false**: `5fd046d` reads the stack or rebuilds the text from name and message (`webkit-cancelled.ts`, unit-tested) |
| "Importing a module script failed." arrives as a page error's whole `message` | yes in one shape (whole text as `message`); Playwright's split gives `name` "TypeError" in the other; React also logs it as a `console.error` whose first line ends in a space | partly: `2d2d78e` accepts both page-error shapes (the refactor in `5fd046d` had dropped the first; caught by design `theme_follows_the_system_unless_chosen` in a WebKit-only run), `11d9ba9` the console line |
| Playwright's `request.url()` matches the URL in WebKit's message | for every logged "Fetch API cannot load …/revisions?limit=50", the page had **no** `request` or `requestfailed` event for that URL; the error came about 10 ms before the main frame's navigation committed (e.g. error at …65417, `framenavigated` at …65426) | **false**: `b33bb1a` also accepts it when the page's main frame navigated within 2 s |
| The reload retry (one retry after "Frame load interrupted") holds | a trace showed three document requests for one `reload()`; the immediate retry was interrupted 3 ms later | **false**: `b33bb1a` waits 250 ms and retries at most twice |

After `b33bb1a`, `design.spec.ts` and `palette.spec.ts` in webkit-sqlite passed 156 of 156
(12 repeats); the import rule was logged as explained (a cancelled script within 2 s).

### 5.3 Contact sheet (11 §8)

The `chromium-sqlite` snaps of run 4 (228 snaps, 909 images: light and dark at 1440×900
and 390×844, plus `shell-focus-nav-dark`) were tiled one snap per row, light/dark desktop then
light/dark phone, on sheets rendered with Playwright's Chromium (a scratchpad script): the
first 128 snaps four to a sheet (32 sheets), the other 100 two to a sheet (50 sheets) so
the long pages stay legible. All 82 sheets were looked at, the account screens included;
single images were opened where a sheet was too small to judge. Run 20's snaps were taken
by the final code; the UI changes after run 4 (`d767108`, a `title`; `1807f2f`, ARIA
attributes) should change no pixels, which was reasoned, not checked: the sheets were not
taken again (see Not verified 6).

| 11 §8 item | Finding | Action |
|---|---|---|
| Text never truncated without a tooltip or `title` | the new-token dialog's scope labels (`token-dialog.tsx`, `truncate`, no `title`); every other truncation found has one (`grep truncate` in `src/account`) | **fixed** `d767108`, with a check in `token_scopes_offer_only_held_scopes` that fails without it |
| ≥ 44 px touch targets | `icon-sm` and `icon-xs` buttons (`button.tsx`: `size-7`, `size-6`) do not grow under `pointer: coarse`; only `--control-h` does (`tokens.css`). They meet WCAG 2.2 AA 2.5.8 (24 px) but not 11 §8's 44 px; seen in the phone bulk bars and row menus | **listed**: a fix changes dense rows on many screens, and the phone snaps do not emulate a coarse pointer, so no snap would check it |
| Dark mode at parity | dropdown menus in dark mode have a faint edge against the rows they overlap (`ring-foreground/10` on `bg-popover`, which is `--card`), e.g. `styleguide-ordering-menu` | **listed** (shadcn's default; a judgement, not a rule breach) |
| Contrast | the palette's stale rows at 60 % opacity during a slow search (§5.1 k) fall below 4.5:1 | **listed**: a passing state, the rows stay selectable; WCAG's contrast rule has no exception for it |
| Capture | full-page phone snaps show `position: fixed` bars (the editor's word count in `editor-focus-mode`) mid-page, where the viewport ended | **listed** (a property of full-page screenshots, not of the screen) |

Everything else looked consistent with 11 §8: one primary action per view, consistent page
headers, loading, empty, error and permission states, tabular numbers, focus rings in both
schemes, no horizontal overflow at 390 px (the snap fixture also fails on overflow), and
the account screens (`account`, `account-email-change`, `account-totp-scan`,
`account-totp-code`, `account-token-new`, `account-token-created`) as Task 6 left them, except
the long harness login, which is cut with an ellipsis and has a `title`.

### 5.4 `docker build` serving `/admin/`

```
$ docker build -t ferrite:m2b3-exit .            # at 7fb1f77; 2 min 13 s; exit 0
  #17 [admin 7/7] RUN pnpm build … check-dist: ok (/src/admin/dist)
  #21 Finished `release` profile [optimized] target(s) in 1m 32s
  writing image sha256:ed63b955474a…; 115 229 721 bytes, arm64, ENTRYPOINT [/ferrite] CMD [serve --data /data]
$ docker volume create ferrite-m2b3-exit
$ docker run --rm -v ferrite-m2b3-exit:/data ferrite:m2b3-exit init --data /data --url http://localhost:18091
  database migrated applied=[1, 2, 3, 4, 5, 6, 7, 8]
$ docker run -d --name ferrite-m2b3-exit -p 127.0.0.1:18091:8080 \
    -e FERRITE_SERVER__LISTEN=0.0.0.0:8080 -v ferrite-m2b3-exit:/data ferrite:m2b3-exit
  Listening on http://0.0.0.0:8080
$ curl -s -D - http://127.0.0.1:18091/admin/
  HTTP/1.1 200 OK; content-type: text/html; charset=utf-8; cache-control: no-cache;
  content-security-policy: default-src 'self'; script-src 'self'; …
  2 136 bytes; "FerriteCMS admin not built" (the placeholder's text) 0 times;
  <script type="module" crossorigin src="/admin/assets/index-BJEBqctT.js">; 19 modulepreloads
$ curl …/admin/assets/index-BJEBqctT.js     # 200 text/javascript; charset=utf-8, 316 418 bytes
$ curl …/admin/content/post                 # 200 (the SPA's fallback)
$ docker stop ferrite-m2b3-exit             # "node stopped", "shut down"; no ERROR in its log
```

The local build's `admin/dist/index.html` names the same entry, `index-BJEBqctT.js`.

### 5.5 Linux Chromium

The binary from the image above (`docker cp` of `/ferrite`: ELF 64-bit aarch64, GNU/Linux),
in the image matching `@playwright/test`:

```
$ docker run --rm --ipc=host -v "$PWD:/repo" -v /repo/admin/node_modules \
    -v "$SP/linux-bin:/linux-bin:ro" -e FERRITE_BIN=/linux-bin/ferrite -e FERRITE_TEST_PG_URL= \
    -w /repo/admin mcr.microsoft.com/playwright:v1.64.0-noble \
    bash -c 'npm install -g pnpm@10.28.2 && pnpm install --frozen-lockfile && pnpm e2e --project chromium-sqlite'
```

Ubuntu 24.04.5 LTS, Linux aarch64, Node 24.21.0. The first attempt failed at global setup:
"ferrite init --data failed … invalid configuration … env:FERRITE_BIN: unknown key"; the
harness passed its own `FERRITE_BIN` to `ferrite`, which refuses unknown `FERRITE_*` keys
(fixed `43f27a6`, unit test `harness_variables_never_reach_ferrite`). The second attempt:
**191 passed (8.1 min)**, 0 failed. The container's `pnpm install` left a `.pnpm-store/`
(432 MB) in the repository root; it was moved to the scratchpad.

### 5.6 actionlint

```
$ docker run --rm -v "$PWD:/repo" -w /repo rhysd/actionlint:latest -no-color .github/workflows/ci.yml
  exit 0, no output                       # rhysd/actionlint 1.7.12 (image created 2026-03-30)
```

### 5.7 Gates at the end (final code)

```
$ pnpm -C admin lint / typecheck          # exit 0 / exit 0
$ pnpm -C admin test                      # 114 files, 661 passed (part A: 111 files, 652)
$ pnpm -C admin api:check / build / bundle:check   # current / check-dist: ok / bundle:check: ok
$ cargo fmt --all --check (and plugins)   # exit 0, exit 0
$ cargo clippy --all-targets -- -D warnings; plugins: --target wasm32-wasip2   # exit 0, exit 0
$ git diff --stat 4e695da..HEAD -- crates plugins   # empty: no Rust changed in part B
```

The admin gates ran last at `22d3272`; `fmt` and `clippy` ran at `43f27a6`, and no Rust changed
after. The Docker image (§5.4) and the Linux run's binary (§5.5) were built at `7fb1f77`;
`git diff --stat 7fb1f77..HEAD -- admin/src crates plugins Dockerfile admin/package.json
admin/pnpm-lock.yaml` is empty, so the product they contain is the final one. The Linux run
used the E2E harness of `43f27a6`; the three test commits after it (`11d9ba9`, `107b2f9`,
`22d3272`) ran on macOS only.

The workspace tests were not run again in part B: no Rust code changed.

## Verified (claims in this task's doc changes)

| Claim (file) | How verified | Result |
|---|---|---|
| Every flow result in §1 | the command output recorded there (the scratchpad keeps the responses, the mails and `serve.log`) | true |
| §2 counts, §3 gate results, §4 sizes | the command output above (`grep "^test result"` summed with `awk`) | true |
| The bundle limits (§4, ROADMAP) | `cat admin/bundle-budget.json`: `measured_at` `3209291`; 108 544, 262 144, 33 792, 218 112, 108 544, 31 744 | true |
| Rulings P57: the cookie is `Path=/admin` | `admin/src/design/preferences.ts` `writeThemeCookie`; §1 (`class` per cookie value) | true |
| Rulings R18 and 11 §6: the physical key with ⌘ whatever AltGraph reports | `admin/src/lib/shortcuts.ts` `eventKey`; the test `mac_option_reported_as_altgraph_still_runs_cmd_option_r` in `shortcuts.test.ts` | true |
| 11 §6: a sequence completes across re-registration | `shortcuts.test.ts` `chord_completes_with_a_binding_registered_between_steps`; Task 14 report | true |
| Rulings R67: `erased_at` written after the erase, cleared on reinstall and by the final sweep | `crates/ferrite-core/src/extensions/mod.rs` (`erase_locked`, `delete_options`, the install's marker loop); §1 (a `purge=true` delete is not listed) | true |
| Rulings R56: the reset mail reached a real sink in the M2b-1 exit | `m2b1-exit.md` §3 | true |
| Rulings R37–R76, Rulings A–C | the M2b-3 ledger (`progress.md`, not committed) and the task reports | copied |
| ROADMAP: the dead comment keys left before M2b-2 merged | `git log -S` finds their removal in `e72fc7b`, an ancestor of `e1cd3a5` (main) | true |
| ROADMAP / CLAUDE.md: no migration in M2b-3 | `ls crates/ferrite-store-*/migrations` ends at `0008`; `git diff --stat e1cd3a5..HEAD` over both folders is empty | true |
| CLAUDE.md / 01 §1: `ferrite theme pack`, the CLI commands | `ferrite --help`, `ferrite theme --help` on the release build | true |
| 01 §1: the admin's `src/` folders, the example plugins | `ls admin/src`, `ls plugins/examples` | true |
| 07 §3 Shell: 120-character snippet, "Matches in text" | `palette/sources.ts` `SNIPPET_CHARS = 120`; `i18n/en/palette.json` | true |
| 07 §3 Content types: "Can point at"; Editor: "Search {{types}}…" | `i18n/en/structure.json`, `i18n/en/editor.json` | true |
| 07 §3 Media: the forbidden state alone, uploads kept in the tray | Task 13 report (fix-round item 2, E2E `forbidden_keeps_running_uploads`) | from the report; its E2E passed in all four projects in runs 20 and 21 |
| 10 §1: the M2b-3 spec and support files | `git diff --name-status e1cd3a5..HEAD -- admin/e2e` (added files); `app-state.ts` reads both caches (`getQueryCache`, `getMutationCache`) | true |
| §"Task 14 notes": the `fixtures.ts` names | `grep` for `CANCELLED_LOAD`, `GONE_ELSEWHERE`, `TOAST_IN_TRANSITION`, `deferCancelled` | true |
| Not verified 3: the paths CI downloads to | Task 14 report, Concerns 3 | from the report |
| §5.1 run table, timings, failures | each run's log in the scratchpad (`run<N>.log`, failing runs kept as `run<N>.keep.log`), counted per project with a script over the list reporter's lines; `git rev-parse --short HEAD` before each pair | true |
| §5.1 (l): the save-state crossfade is a WAAPI `Animation` | a temporary probe spec (deleted afterwards), run in Chromium, Firefox and WebKit; Task 16 report, Concerns 1 | true as observed; that it caused run 18's failure is code reading, not reproduced |
| §5.1 (d): WebKit Networking ended by WebKit, host load | the three `.ips` reports (`is_simulated`, `exception`, `termination` fields), `uptime`, `ps -Ao pcpu,comm -r` at 07:03 | true as observed; the cause (load) is an inference |
| §5.1 (e), §5.2: attribute values, missing events, timings | the temporary logging's output (`ad.log`, `failed.log` in the scratchpad), Playwright 1.64's `coreBundle.js` (`splitErrorMessage`, `_onConsoleMessage`), cmdk 1.1.1's `dist/index.mjs` (`selectedItemId`) | true |
| §5.1: cmdk 1.1.1 is the latest release | `npm view cmdk version` → 1.1.1 | true on 2026-10-28 |
| §5.3: the contact-sheet findings | the sheets and images; `button.tsx` sizes, `tokens.css` `pointer: coarse` block, `dropdown-menu.tsx` classes, `grep truncate` | true |
| §5.4–§5.6: Docker, Linux, actionlint | the command output recorded there (`docker-build.log`, `linux-e2e.log` in the scratchpad) | true |
| §5.7 gates | the command output recorded there | true |
| `10-testing.md` §1 and §3 (part B edits) | the code in `webkit-cancelled.ts`, `child-env.ts`, `fixtures.ts` (`goneElsewhere`, `RELOAD_SETTLE_MS`), `shell.spec.ts` (`tabTo`), `palette.spec.ts`, `vitest.config.ts` | true |

## Not verified (limits)

1. **Part B's limits** (§5): the Linux run was `chromium-sqlite` only, on arm64 (CI runs
   x86_64 and all four projects); Firefox and WebKit ran on macOS only. One E2E failure was
   never diagnosed (`set_featured_image_from_picker` on chromium-pg in run 9, which was
   stopped before its artifacts were read; 40 of 40 in repeats afterwards; possibly the
   purged-image 404 that `7fb1f77` accepts or the crossfade that `22d3272` waits for, the
   same test's later failures; not shown). The Firefox console error
   "JSHandle@object" from Task 14 did not appear in any of the twenty-one runs (`grep` over
   every run log), so it stays unexplained. A Docker volume `ferrite-m2b3-exit` and the image
   `ferrite:m2b3-exit` are left on the host.
2. **Safari**, with `⌘⌥R` on a real Mac (R18): the user's manual check. Linux WebKit in CI
   is not Safari (P55).
3. **CI** has never run on this branch (R72). On the final develop→main pull request, watch:
   the `download-artifact` paths (`target/release/ferrite`,
   `plugins/target/wasm32-wasip2/release/admin_e2e.wasm`), `playwright install --with-deps
   webkit` on `ubuntu-latest`, the conditional Postgres service (only `chromium-pg` starts
   one), and whether Linux WebKit needs the Option+Tab step (applied on macOS only).
4. **The flows ran over the API, on SQLite, one node**, with `curl` and Python, not through
   the screens. Postgres is covered by the workspace tests (§2) and by `chromium-pg` (§5.1).
5. **The Users screen's "Send a reset e-mail"** is unit-tested only (R56); it was not run
   here either.
6. **The contact sheet after run 4**: the UI changes `d767108` (a `title`) and `1807f2f`
   (ARIA attributes) are reasoned to change no pixels; the sheets were not re-taken.

## Task 14 notes carried here

- **Three product fixes** came out of the E2E matrix (Task 14), each with a unit test that
  fails without it: a key sequence completes when bindings are registered again between its
  keys (`96a3429`); a rich-text field ignores the late echo of its own earlier value, so fast
  typing keeps every character (`f90c53a`, with fix-round `3209291` clearing the set when a
  value comes from outside); a route loader's `NetworkError` shows the route's error state
  instead of a `console.error` (`a4ea716`).
- **`admin/e2e/support/fixtures.ts` engine workarounds** (10 §3 lists each): Firefox's
  aborted web-font line is accepted (`CANCELLED_LOAD`, Firefox only); WebKit's "Fetch API
  cannot load … due to access control checks." and "Importing a module script failed." are
  accepted only when the same page saw that request (or a script request) cancelled within
  2 s (part B widened the fetch rule to a main-frame navigation within 2 s, §5.2); WebKit's
  `page.reload` is tried once more after "Frame load interrupted" (part B: after 250 ms, at
  most twice); a 404 from
  another test's custom type list, custom taxonomy terms or (outside `plugins.spec.ts`) a
  plugin's metrics is accepted (`GONE_ELSEWHERE`); a toast fading in or out is left out of
  axe.
- **CI not yet run on GitHub**: it runs on the final develop→main pull request; the
  Postgres service for `chromium-pg` only and Linux WebKit are unverified there.
