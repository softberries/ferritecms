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
`fmt` and `clippy` passed. No flow turned up a product bug. **Part B is PENDING** (ruling
R76): on this host no browser engine launches and the Docker daemon is down (the fault
recorded in Task 14: `pgrep` says "sysmond service not found", Chromium fails with
`bootstrap_check_in … (141)`), so every check that needs a browser or Docker waits for a
reboot. The controller fills in the PENDING rows.

| Exit check | Verified by | Result |
|---|---|---|
| 1. The flows on a release build | §1: `curl` against the REST API of a fresh site, a computed RFC 6238 code, a Python SMTP catcher | yes, over the API (the screens themselves: E2E, row 4) |
| 2. Workspace tests, both backends, with and without `admin/dist` | §2: two full runs with `CI=true` and PostgreSQL 17 | yes: 2 458 passed, 0 failed, each run |
| 3. Admin gates (`lint`, `typecheck`, `test`, `api:check`, `build`), `fmt`, `clippy` | §3 | yes |
| 4. The four E2E projects (`pnpm -C admin e2e:all`), twice in a row, with flake counts | — | **PENDING** (part B) |
| 5. Contact sheet against 11 §8 (with the account screens' snaps re-checked) | — | **PENDING** (part B) |
| 6. Bundle budget | §4 | yes: every group under its limit |
| 7. `docker build` serving `/admin/` | — | **PENDING** (part B; Docker daemon down) |
| 8. A Linux Chromium run (the CI engine's platform) | — | **PENDING** (part B) |
| 9. actionlint on `.github/workflows/ci.yml` after Task 14's fix round | — | **PENDING** (part B; actionlint runs from its Docker image) |
| 10. Safari, with `⌘⌥R` on a real Mac | — | **not verified**: the user's manual check (P55, R18) |
| 11. CI on GitHub | — | **not run**: no push in M2b-3 (R72); it runs on the final develop→main pull request |

No code changed in this task. Docs changed: this file, [`m2-exit.md`](m2-exit.md),
`docs/superpowers/rulings/m2b.md` (P57's cookie path as built, R18 as updated, the M2b-3
pre-flight Rulings A–C and controller rulings R37–R76), ROADMAP (M2b-3 ticked, the M2
status, follow-ups carried to M4 and Later), CLAUDE.md (status, `theme pack`), README
(status line), 01 §1 (crates, admin layout, example plugins) and §2 (HTTP and Mail rows),
07 §3 (the Shell, Dashboard, Editor, Media and Content types rows as built in M2b-3), 10 §1
(the M2b-3 specs and support files) and 11 §6 (key sequences, Option and AltGraph) and §8
(snapshots in `chromium-sqlite` only).

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
  their E2E specs (row 4, PENDING).

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

`pnpm -C admin e2e` and `e2e:all` were not run: no browser engine launches on this host
(PENDING, row 4).

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
| 07 §3 Media: the forbidden state alone, uploads kept in the tray | Task 13 report (fix-round item 2, E2E `forbidden_keeps_running_uploads`) | from the report (E2E PENDING) |
| 10 §1: the M2b-3 spec and support files | `git diff --name-status e1cd3a5..HEAD -- admin/e2e` (added files); `app-state.ts` reads both caches (`getQueryCache`, `getMutationCache`) | true |
| §"Task 14 notes": the `fixtures.ts` names | `grep` for `CANCELLED_LOAD`, `GONE_ELSEWHERE`, `TOAST_IN_TRANSITION`, `deferCancelled` | true |
| Not verified 3: the paths CI downloads to | Task 14 report, Concerns 3 | from the report |

## Not verified (limits)

1. **Every PENDING row above** (part B): the four E2E projects twice with flake counts, the
   contact sheet against 11 §8 (the account screens' snaps re-checked there, a carry from
   Task 6), `docker build` serving `/admin/`, a Linux Chromium run and actionlint after
   Task 14's fix round. Also owed from Task 14: the two assumptions of the WebKit
   cancelled-fetch rule in the E2E guard (Playwright's `request.url()` matches the URL in
   WebKit's message; the page error's `stack` starts with "Fetch API cannot load"), and the
   Firefox console error "JSHandle@object" seen once in the palette spec (not reproduced in
   30 repeats).
2. **Safari**, with `⌘⌥R` on a real Mac (R18): the user's manual check. Linux WebKit in CI
   is not Safari (P55).
3. **CI** has never run on this branch (R72). On the final develop→main pull request, watch:
   the `download-artifact` paths (`target/release/ferrite`,
   `plugins/target/wasm32-wasip2/release/admin_e2e.wasm`), `playwright install --with-deps
   webkit` on `ubuntu-latest`, the conditional Postgres service (only `chromium-pg` starts
   one), and whether Linux WebKit needs the Option+Tab step (applied on macOS only).
4. **The flows ran over the API, on SQLite, one node**, with `curl` and Python, not through
   the screens. Postgres is covered by the workspace tests (§2) and by `chromium-pg` (PENDING).
5. **The Users screen's "Send a reset e-mail"** is unit-tested only (R56); it was not run
   here either.

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
  2 s; WebKit's `page.reload` is tried once more after "Frame load interrupted"; a 404 from
  another test's custom type list, custom taxonomy terms or (outside `plugins.spec.ts`) a
  plugin's metrics is accepted (`GONE_ELSEWHERE`); a toast fading in or out is left out of
  axe.
- **CI not yet run on GitHub**: it runs on the final develop→main pull request; the
  Postgres service for `chromium-pg` only and Linux WebKit are unverified there.
