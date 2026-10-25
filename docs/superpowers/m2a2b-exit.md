# M2a-2b exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M2a-2b): *media, comments and the system API work
against a real `ferrite serve`: an upload with an `Idempotency-Key` sent twice gives one
item, variants appear, a hostile SVG is served inline sanitised under the sandbox CSP, a
theme-form comment is moderated over the API and shows on the page, system status and a
dead-job retry, a plugin `http` fetch of an allowlisted host and a refused `127.0.0.1`;
workspace tests on both backends.*

**Result (2026-10-25, branch `m2a2b-media` at `b10496d` plus this docs commit):** met, with
the limits below. The automated tests ran on both backends; the manual `curl` session ran on
the solo profile (SQLite + local media) against the debug binary.

| Part of the criterion | Verified by | Result |
|---|---|---|
| Upload with an `Idempotency-Key` twice → one item | manual §2 (two `201`s, same `Location`, the second with `Idempotent-Replayed: true`, one item listed); `idempotency::idempotent_retry_creates_one_item`, `concurrent_duplicates_across_nodes_create_one_item` (`crates/ferrite-http/tests/api/idempotency.rs`) | yes |
| Variants appear | manual §2 (`width`/`height` set, `thumbnail`, `medium` and the `blog` theme's `card` and `hero`; no `large` for a 1000 px source); `media::upload_gets_variants_through_the_runner` (`crates/ferrite/tests/node/media.rs`, SQLite and Postgres) | yes |
| Hostile SVG → inline, sandbox CSP, no `script` | manual §3 (`sanitized: true`, `image/svg+xml` inline with `sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'`, the body has no `script`, `on*`, `foreignObject`, `javascript:` or external `url(`); `media::upload_svg_is_sanitised`, `ferrite-media` `tests/svg.rs` | yes |
| Theme-form comment → moderated over the API → on the page | manual §4 (`303 #comment-pending`, not on the page, pending in `GET /comments`, `PATCH` approved, on the page after the invalidation poll; the same guest then posts directly under `known`); `comments::form_comment_flow`, `comments::comment_api_flow` | yes |
| `GET /system/status`, `/system/jobs?status=dead`, a retry | manual §5 (a corrupt PNG's `media.variants` job dead at attempt 1 with a readable `last_error`, retried `200` → `queued`, `attempts: 0`, `job.retry` audited, dead again after its re-run); `jobs::dead_job_is_listed_and_retried` | yes |
| Plugin `http` fetch of an allowlisted host; refused `127.0.0.1` | manual §6 (`https://example.com/` → `ok 200` with the real page; `https://127.0.0.1/` → `host not allowed`; a listed name resolving to `127.0.0.1` (`localtest.me`) → `address not allowed`, logged at `warn`); `egress::ssrf_targets_are_refused`, `rebinding_answer_is_refused` | yes |
| Workspace tests on both backends | §"Automated tests" (PostgreSQL 17.11 with `CI=true`: all green; without Postgres: one known flaky CLI test failed once, green alone) | yes, except that flaky test |

## Verified (claims in this milestone's docs)

Every number below was read from the code or from a command run for this exit.

| Claim | How verified | Result |
|---|---|---|
| `media.max_upload` default 256 MiB, ceiling 1 GiB | `crates/ferrite-config/src/model.rs` (`DEFAULT_MAX_UPLOAD = 256 << 20`, `MAX_UPLOAD_CEILING = 1 << 30`) | matches 01 §4, 03 §3, 07 §1 |
| 8 uploads at once per node; type from the first 8 KiB; variants skipped above 64 MiB; idle timeout 30 s | `crates/ferrite-http/src/media.rs` (`MEDIA_UPLOAD_SLOTS = 8`, `HEAD_BYTES = 8 * 1024`, `MAX_PROCESS_BYTES = 64 MiB`, `DEFAULT_IDLE_TIMEOUT = 30 s`) | matches |
| SVG input at most 2 MiB | `crates/ferrite-media/src/svg.rs` (`MAX_SVG_BYTES = 2 * 1024 * 1024`) | matches |
| `media.image_workers` default 1, at most 16 | `model.rs` (`image_workers: 1`, `MAX_IMAGE_WORKERS = 16`) | matches |
| Accepted document classes and their types | `media.rs` `ZIP_TYPES` (7 entries), `TEXT_TYPES` (2); manual §3 (CSV `201` as `text/csv`, served `attachment` + `nosniff`; HTML named `.txt` `415`) | matches 03 §3 |
| PDF inline with `sandbox`; sanitised SVG inline with its CSP; everything else `attachment` | `crates/ferrite-http/src/app/media.rs` (`SVG_CSP`, lines 286–304) | matches 03 §5 |
| `Idempotency-Key` kept 24 h, stale after 1 h | `crates/ferrite-domain/src/auth.rs` (`IDEMPOTENCY_RETENTION`, `IDEMPOTENCY_STALE`) | matches 07 §1 |
| `Busy` jobs due again after 1–5 s | `crates/ferrite-cluster/src/jobs/mod.rs` (`BUSY_RETRY_MIN = 1 s` + up to `BUSY_RETRY_JITTER = 4 s`) | matches 03 §4, 04 §6 |
| Comments: default on, `known`, 3/min, 20/h, depth 5, ≤ 2 links, thread ≤ 500 | `crates/ferrite-core/src/comments.rs` (`COMMENTS_ENABLED_DEFAULT`, `COMMENTS_MODERATION_DEFAULT = "known"`, `COMMENTS_PER_MINUTE = 3`, `COMMENTS_PER_HOUR = 20`, `MAX_COMMENT_DEPTH = 5`, `MAX_KNOWN_LINKS = 2`, `MAX_THREAD = MAX_ITEM_COMMENTS = 500`); manual §4 (`GET /settings` shows `null` for both keys, the second comment is approved) | matches 06 §4, 07 §1 |
| 16 capabilities, `manage_system` admin-only | `crates/ferrite-core/src/actor.rs` (`Capability::ALL: [Capability; 16]`, `can`); manual §1 (the admin's `capabilities` list has 16 entries) | matches |
| Egress: 3 fetches per call, 3 redirects, 5 s, 2 MiB in, 1 MiB out, 32 request headers, 64 response headers, 16 lookups | `crates/ferrite-plugin-host/src/state.rs` (`http: 3`, `MAX_HTTP_REQUEST_HEADERS = 32`, `MAX_HTTP_REQUEST_BODY = 1 MiB`), `crates/ferrite-http/src/plugins/egress.rs` (`MAX_REDIRECTS = 3`, `TIMEOUT = 5 s`, `MAX_RESPONSE_BODY = 2 MiB`, `MAX_RESPONSE_HEADERS = 64`, `LOOKUP_SLOTS = 16`) | matches 05 §6 |
| `0006` adds the tables, columns and indexes listed in 02 §4 | `grep` of both `0006_media_comments.up.sql` files: `idempotency_keys` (+ `idempotency_keys_created_idx`), `media.sanitized`, `comments.by_account`, five comment indexes, `jobs_status_created_idx`, `jobs_created_idx`, `jobs_kind_created_idx` | matches after this task added `jobs_kind_created_idx` to 02 §4 |
| Schema version 6 | manual §5 (`schema.db_version: 6`, `binary_version: 6`, `verdict: up_to_date`) | matches |
| The sweep's no-credential list has five entries | `crates/ferrite-http/tests/api/sweep.rs` (`UNAUTHENTICATED: [&str; 5]`) | matches 10 §1 |
| `nodes.meta.job_kinds` comes from the handler list | `crates/ferrite/src/serve.rs:555`, `crates/ferrite/src/node.rs` | matches 04 §3 |
| A normal build has no test seams | `crates/ferrite/tests/seams.rs` `release_build_has_no_seams` passed; `cargo clippy -p ferrite -- -D warnings` and `-p ferrite-core` (no dev targets) clean after gating `with_limits`/`with_lock_wait` | yes |

## Carries handled in Task 16

| Carry (ledger) | How it was handled |
|---|---|
| R1 / F13: documents accepted, download-only; P6/P29, 03 §3, ROADMAP "extra upload types" | 03 §3 gains "Accepted upload types (as built)" with the class table; P6 and P29 annotated in the plan and the rulings file (`rulings/m2a2b.md`); the ROADMAP media item names the classes, and "Later" has `media.extra_types` |
| F14: PDF stays inline with `sandbox` | stated in the 03 §3 table (03 §5 already said it) |
| F15: Direct-mode caveat in 03 §5 | already present in 03 §5 (added in Task 3, `7202d00`), checked; also in the M2a-2b known limits |
| R2 / F20: comments on by default, `known`; P11/P13, 07 §1, 06 §4 | 07 §1 already said it (Task 7), checked; 06 §4 gains a moderation paragraph; P11 and P13 annotated in the plan and the rulings file, with the spoofing caveat (Task 7 review M2) |
| CSV formula injection (Task 4 review minor 3) | 03 §3: CSV is stored as uploaded and served download-only; a spreadsheet still runs `=`-led cells; FerriteCMS does not rewrite uploads |
| R10 docs | already in 07 §1 "Limits" (failed finish, upload over 1 h, drop during commit; Task 5 fix round, `7cfcec4`), checked |
| R11: GPS EXIF stays in the original; ROADMAP M4 item | 03 §4 already said it; the M4 item "Optional location-metadata stripping on upload" already existed (Task 6 fix round, `f1bc51f`), checked |
| Comment pagination to M2b | already in ROADMAP M2b, checked |
| `MailQueue::with_limits` / `with_lock_wait` behind `test-seams` | commit `b10496d`: `MailQueue::with_limits` is used only by `mail.rs`'s unit tests, so it is `#[cfg(test)]` and private (`new` builds through a private `build`); `MailQueue` has no `with_lock_wait` — the `with_lock_wait` builders are `ExtensionService`'s and `ExtensionAdmin`'s, used only by `ferrite-core/tests/extensions.rs` and the API harness, so both are behind `ferrite-core`'s `test-seams`; 10 §2's seam list updated |
| ROADMAP M11 tick | ticked (M2a-2a final-review list), with the `test-seams` note |
| Task 15 report: M11 tick for Task 16 | same as above |
| P26–P30 moves | ROADMAP: permalink-structure redirects and import `Idempotency-Key` to M3; `date` time zones, `seed demo` atomicity, `ferrite media regenerate` and the metrics registry to M4; P29 comment items and `tel:`/extra types to "Later" |
| CLAUDE.md status | M2a-2b done, M2a complete, next M2b; "not built yet" trimmed to the SPA and the importer |
| Parked: Task 13 flakes (`cli` `plugin_remove_keeps_data_without_purge`, `plugins` `dropped_meta_write_is_all_or_nothing::cluster`) | `plugin_remove_keeps_data_without_purge` failed once (the SQLite-only run) and passed 6 of 6 alone; `dropped_meta_write_is_all_or_nothing::cluster` passed in both runs; neither fixed here (docs task, cause not established); listed under "Not verified" |
| Parked: Task 11 minor 5, Task 13 minor 3, Task 14 minors 3 and 7 | left as parked (no code change in a docs task); listed under "Not verified" |

## What was not verified (limits)

1. **The manual session ran on SQLite only**, one node, debug build. Postgres, S3 and two
   nodes are covered by the automated tests only (`concurrent_duplicates_across_nodes_create_one_item`,
   `comment_limits_hold_across_nodes`, every `api_tests!`/`app_tests!` case on the cluster
   profile, `node/media.rs` on Postgres).
2. **Direct S3 mode** (`[media.s3] public_base_url`) was not run: its caveat (no
   download-only rule, no SVG CSP, no published-only check) is documentation only.
3. **No browser.** The SVG CSP, `Content-Disposition: attachment` and `nosniff` were checked
   as headers with `curl`, not by opening the files in a browser. The comment form was
   posted with `curl`, not submitted from a page.
4. **The `127.0.0.1` refusal by address** used `localtest.me`, a public DNS name that
   resolved to `127.0.0.1` on this machine at the time of the session; a direct
   `https://127.0.0.1/` is refused earlier, by the host check (IP literals never match an
   allowlist). Other internal ranges, mixed answers and redirects were not tried by hand
   (they are in `egress::*`).
5. **The network fetch** reached the real `example.com` once over the platform verifier;
   the webpki-roots fallback (R19) and HTTP/2 were not exercised by hand.
6. **Not exercised by hand:** variants after a theme switch, the 64 MiB and decoded-buffer
   limits, the image slot `Busy` path, upload slots and the idle timeout, a stale
   idempotency takeover, replies and the depth rule, the honeypot, rate limits,
   `GET /plugins/{name}/metrics`, the term-archive and slug changes of Task 13, the
   `render.footer` filter and `cache.search_ttl`. Each has automated tests (see the test
   names in 03, 05, 06, 07 and the rulings).
7. **`metrics/summary` once showed `page_cache.entries: 0` with `hits: 4`.** Entries had
   just been dropped by the comment invalidations; the counters are cumulative. This was
   not investigated further.
8. **Parked review items stay open:** Task 11 minor 5 (the hostile `render.head` case
   never reaches the egress's address check), Task 13 minor 3 (a manual redirect to a
   term path without its trailing slash becomes a two-hop chain after a rename), Task 14
   minors 3 (redirect hits recorded by connection tasks still running after a drain
   timeout are lost) and 7 (`config check` does not print the TTLs).
9. **Flaky tests parked from Task 13:** `plugin_remove_keeps_data_without_purge` failed
   once here on the folder lock (see "Automated tests"); `dropped_meta_write_is_all_or_nothing::cluster`
   passed in both runs, which does not show it is fixed.
10. **CI has not run on this commit** at the time of writing; the gates ran locally
    (Darwin 25.6.0 arm64, rustc 1.99.1, PostgreSQL 17.11 at `localhost:54329`; CI uses
    `postgres:18`).

## Automated tests

Run on `b10496d` (the seam-gating commit; this task's other changes are docs only), rustc
1.99.1 after `rustup update stable` ("unchanged"):

```
$ cargo fmt --all --check                                                       # ok
$ cargo fmt --manifest-path plugins/Cargo.toml --all --check                    # ok
$ cargo clippy --all-targets -- -D warnings                                     # ok
$ cargo clippy -p ferrite -- -D warnings                                        # ok (no dev targets, no seams)
$ cargo clippy -p ferrite-core -- -D warnings                                   # ok
$ cargo clippy -p ferrite-http -- -D warnings                                   # ok
$ cargo clippy --manifest-path plugins/Cargo.toml --target wasm32-wasip2 -- -D warnings   # ok
$ CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace --no-fail-fast
  77 test result lines: 2227 passed, 0 failed, 1 ignored (the `ferrite-sdk` `export_plugin` doc test); no SKIP
$ cargo test --workspace --no-fail-fast                                         # no FERRITE_TEST_PG_URL, no CI
  77 test result lines: 2226 passed, 1 failed, 1 ignored (Postgres cases skip)
  failed: cli plugin_remove_keeps_data_without_purge (cli.rs:908, "data folder is in use: …/site/ferrite.lock is locked by another process")
$ cargo test -p ferrite --test cli                                              # six more times, alone
  39 passed, 0 failed (6 of 6)
```

**`plugin_remove_keeps_data_without_purge` is still flaky** (parked from Task 13). It
alternates `ferrite plugin …` child processes with in-process opens of the same SQLite data
folder (`with_site_store`), and under a full-workspace run one in-process open sometimes
finds the folder lock still held. It failed once in the two full runs here and passed six
times in a row on its own. The cause is not established (a lock briefly kept by a process
other than the finished child, e.g. one forked by a parallel test, is a guess, not checked);
it is not fixed here and stays for the final review. The Postgres run was green.

The first `cargo clippy -p ferrite-core -- -D warnings` after gating failed on an unused
`time::Duration` import in `extensions/admin.rs` (only the gated builder used it); fixed
before the commit, then every gate above was clean.

Largest suites in the Postgres run: store contract 357, `ferrite-http` `api` 223, `app` 186,
`ferrite-http` unit 114, `ferrite-core` unit 111, `plugins` 99, `ferrite-plugin-host` unit 96,
`ferrite` `node` 70.

## Manual `curl` session (solo)

Setup: a fresh data folder (`$D`), `site.url = http://localhost:8080`,
`FERRITE_SERVER__LISTEN=127.0.0.1:0` (the server printed a free port; `$B` below), the demo
content, one admin. `$O` is `Origin: http://localhost:8080`, `$C` the `X-CSRF-Token` from
the login, `jar` the cookie jar. The plugin folder holds `plugins/examples/fetcher`'s
manifest with `http = ["example.com"]` and its release component (`cargo build
--manifest-path plugins/Cargo.toml --target wasm32-wasip2 --release -p fetcher`). Headers
other than the ones shown, and the session cookie and CSRF values, are left out; JSON is
reduced with `python3`.

### 1. Site, account, plugin, login

```
$ ferrite init --data $D --url http://localhost:8080
Initialised …/m2a2b-site
$ ferrite seed demo --data $D
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
$ echo '…' | ferrite user create --data $D --login tester --email tester@example.com --password-stdin
created user tester (admin)
$ ferrite plugin install --dir fetcher-plugin --data $D --yes --enable
plugin fetcher 0.1.0 (new)
component: e112da3ef632cf0551bddbfed75420ed2cd15ae7d2d9cb90d7ae2c06368ba845
permissions:
! http: send HTTPS requests to example.com [example.com]
! routes: answer GET requests at /_p/fetcher/get (at most 1000/min per visitor) [GET]
fingerprint: 75bc36f3895f055e331b635d292f7be63582903564917edf7f8c40a54019f293
installed plugin fetcher 0.1.0 (enabled)
$ FERRITE_SERVER__LISTEN=127.0.0.1:0 ferrite serve --data $D &
Listening on http://127.0.0.1:53675
$ curl -c jar -H "$O" -H 'Content-Type: application/json' -d '{"login":"tester","password":"…"}' $B/api/v1/auth/login
HTTP/1.1 200 OK
set-cookie: ferrite_session=<redacted>; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000
user.capabilities: read_admin edit_own publish_own edit_others publish_others upload_media
  manage_terms manage_menus moderate_comments manage_users manage_settings manage_themes
  manage_plugins import manage_redirects manage_system          (16)
```

### 2. PNG upload with an `Idempotency-Key`, twice; variants

`photo.png` is a generated 1000 × 600 RGB PNG (111 271 bytes).

```
$ curl -b jar -H "$O" -H "$C" -H 'Idempotency-Key: m2a2b-exit-png-1' -F 'file=@photo.png;type=image/png' -F title=Gradient $B/api/v1/media
HTTP/1.1 201 Created
location: /api/v1/media/01a17286-04de-7253-b104-6d5a2294f5bd
etag: "2"
{'id': '01a17286-04de-7253-b104-6d5a2294f5bd', 'mime': 'image/png', 'bytes': 111271, 'status': 'published', 'sanitized': False, 'variants': []}
$ (the same command again)
HTTP/1.1 201 Created
location: /api/v1/media/01a17286-04de-7253-b104-6d5a2294f5bd
etag: "2"
idempotent-replayed: true
{'id': '01a17286-04de-7253-b104-6d5a2294f5bd', 'mime': 'image/png', 'bytes': 111271, 'status': 'published', 'sanitized': False, 'variants': []}
$ curl -b jar $B/api/v1/media          # items titled "Gradient"
items titled Gradient: 1
$ curl -b jar $B/api/v1/media/01a17286-04de-7253-b104-6d5a2294f5bd     # after the media.variants job
width 1000 height 600 version 2
  card 640 x 360 image/png /_media/465dac71…/photo.png
  hero 600 x 600 image/png /_media/13645331…/photo.png
  medium 768 x 461 image/png /_media/99933ce4…/photo.png
  thumbnail 150 x 150 image/png /_media/42e22525…/photo.png
$ curl -I $B/_media/ca08d224…/photo.png
HTTP/1.1 200 OK
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
```

`large` (1600 wide) is absent because nothing is upscaled; `hero` (1280 × 640 crop) on a
600 px tall source degrades to the 600 × 600 square, as 03 §4 says.

### 3. Hostile SVG; a CSV and an HTML `.txt`

`hostile.svg` has an `onload` on the root, a `<script>`, a `<foreignObject>` with an iframe
to `javascript:`, an `<a xlink:href="javascript:…">`, and a `rect` with `onclick` and
`style="fill:url(https://evil.example/x)"`.

```
$ curl -b jar -H "$O" -H "$C" -F 'file=@hostile.svg;type=image/svg+xml' $B/api/v1/media
HTTP/1.1 201 Created
{'mime': 'image/svg+xml', 'bytes': 196, 'sanitized': True}
$ curl -i $B/_media/5baba09d…/hostile.svg
HTTP/1.1 200 OK
x-content-type-options: nosniff
content-type: image/svg+xml
content-disposition: inline
content-security-policy: sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'

<?xml version="1.0" encoding="UTF-8"?><svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="red"/><circle cx="50" cy="50" r="40" fill="blue"/></svg>
$ … | grep -ciE "script|onload|onclick|foreignObject|javascript|evil"
0
$ printf 'name,total\n=1+1,3\n' > sheet.csv; curl -b jar -H "$O" -H "$C" -F file=@sheet.csv $B/api/v1/media
201  {'mime': 'text/csv', 'sanitized': False}
$ curl -I $B/_media/e87cb319…/sheet.csv
HTTP/1.1 200 OK
x-content-type-options: nosniff
content-type: application/octet-stream
content-disposition: attachment; filename="sheet.csv"; filename*=UTF-8''sheet%2Ecsv
$ printf '<html><script>alert(1)</script>' > page.txt; curl -b jar -H "$O" -H "$C" -F file=@page.txt $B/api/v1/media
415 /api/v1/problems/unsupported-media-type [{'field': 'file', 'detail': 'files of type text/html are not accepted'}]
```

### 4. A comment through the theme form, moderated over the API

The demo post `hello-world` (`01a17285-40e0-77ff-ba49-2b1bf1412f51`) is at `/hello-world/`;
its page carries the form (`action="/_comment"`, hidden `post`, the honeypot `website`).

```
$ curl -D - -H "$O" --data-urlencode post=01a17285-… --data-urlencode "author=Visitor Ann" \
    --data-urlencode email=ann@example.org --data-urlencode "comment=First visit: lovely gradient demo." \
    --data-urlencode website= $B/_comment
HTTP/1.1 303 See Other
location: /hello-world/#comment-pending
cache-control: no-store
$ curl $B/hello-world/ | grep -c "lovely gradient"
0
$ curl -b jar "$B/api/v1/comments?status=pending"
{'id': '01a17286-d235-724f-bef6-36bd2496f459', 'status': 'pending', 'author_name': 'Visitor Ann', 'author_email': 'ann@example.org', 'item_id': '01a17285-…'}  ip_hash present: True
$ curl -b jar -X PATCH -H "$O" -H "$C" -H 'Content-Type: application/json' -d '{"status":"approved"}' $B/api/v1/comments/01a17286-d235-…
200 approved
$ curl $B/hello-world/            # after 2 polls of 250 ms
<article id="comment-01a17286-d235-724f-bef6-36bd2496f459" class="comment__body">
<p class="comment__meta"><span class="comment__author">Visitor Ann</span>
<div class="comment__text prose"><p>First visit: lovely gradient demo.</p></div>
$ (the same name and e-mail post "Back again." through the form)
HTTP/1.1 303 See Other
location: /hello-world/#comment-01a17287-1d59-735b-b6c6-1519467d49ea      # approved at once ("known")
$ (a form post without Origin)
403
$ curl -b jar $B/api/v1/settings     # comments.*
{'comments.enabled': None, 'comments.moderation': None}            # unset: the core defaults apply
```

### 5. System status, a dead job and its retry

`broken.png` has a PNG signature and header but a corrupt `IDAT`, so it is accepted as
`image/png` and its `media.variants` job fails to decode.

```
$ curl -b jar -H "$O" -H "$C" -F file=@broken.png $B/api/v1/media
201
$ curl -b jar "$B/api/v1/system/jobs?status=dead"
{"items": [{"id": "01a17287-3ace-71c4-9d59-79a740577b11", "kind": "media.variants", "status": "dead",
  "attempts": 1, "max_attempts": 5, "priority": 0, …,
  "last_error": "image could not be processed: unsupported image: Format error decoding Png: Corrupt deflate stream. BadZlibHeader",
  "locked_by": null, "handled": true}], "next_cursor": null}
$ curl -b jar $B/api/v1/system/status
{"version": "0.1.0", "site_url": "http://localhost:8080", "mode": "solo", "database": "sqlite", "media": "local",
 "schema": {"db_version": 6, "binary_version": 6, "pending": [], "verdict": "up_to_date"},
 "theme": {"active": null, "fallback_nodes": []},
 "jobs": {"queued": 0, "running": 0, "done": 12, "dead": 1},
 "nodes": {"live": 1, "stale": 0}, "node": {"id": "01a17285-5fca-…", "started_at": "2026-10-25T10:05:05.868037Z"}}
$ curl -b jar -X POST -H "$O" -H "$C" $B/api/v1/system/jobs/01a17287-3ace-…/retry
200 {'status': 'queued', 'attempts': 0, 'last_error': 'image could not be processed: …BadZlibHeader', 'handled': True}
$ (the same retry without X-CSRF-Token)
403
$ curl -b jar "$B/api/v1/system/audit?limit=1"
{'action': 'job.retry', 'actor': {'id': '01a17285-41d1-…', 'login': 'tester'}, 'details': {'id': '01a17287-3ace-…', 'kind': 'media.variants'}}
$ curl -b jar "$B/api/v1/system/jobs?status=dead"      # after the retried run
{'id': '01a17287-3ace-71c4-9d59-79a740577b11', 'status': 'dead', 'attempts': 1}
$ curl -b jar $B/api/v1/system/metrics/summary
{'renders': 2, 'page_cache': {'entries': 0, 'bytes': 0, 'hits': 4, 'misses': 2}, 'plugins': {'calls': 0, 'failures': 0}, 'jobs': {'queued': 0, 'running': 0, 'done': 12, 'dead': 1}}
```

`theme.active` is `null` because the demo site has no theme row (the embedded `blog`
serves); the node log has the same error text at `warn` for each attempt.

### 6. Plugin `http`: an allowlisted host and refusals

```
$ curl -D - "$B/_p/fetcher/get?u=https://example.com/"
HTTP/1.1 200 OK
content-security-policy: sandbox allow-forms; form-action 'self'
ok 200 <!doctype html><html lang=en><head><meta charset=utf-8>…<title>Example Domain</title>…
$ curl "$B/_p/fetcher/get?u=https://127.0.0.1/"
err host not allowed
$ curl "$B/_p/fetcher/get?u=https://localhost/"
err host not allowed
$ curl "$B/_p/fetcher/get?u=https://169.254.169.254/latest/meta-data/"
err host not allowed
$ curl "$B/_p/fetcher/get?u=http://example.com/"
err only https URLs are allowed
$ curl "$B/_p/fetcher/get?u=https://example.com:8443/"
err only port 443 is allowed
```

Then the resolved-address check: `serve` stopped (SIGTERM, "shut down"), the manifest
changed to `version = "0.1.1"`, `http = ["example.com", "localtest.me"]` (`localtest.me`
resolved to `127.0.0.1` here), reinstalled, `serve` started again (`$B` =
`http://127.0.0.1:55367`):

```
$ ferrite plugin install --dir fetcher-plugin --data $D --yes
…
this update WIDENS the installed grant, adding:
! http: send HTTPS requests to localtest.me [localtest.me]
fingerprint: 75c60ead4981caa4b8b6099b135fefac683fccdd751d535bfd13ed857fc604ef
installed plugin fetcher 0.1.1 (enabled)
$ curl "$B/_p/fetcher/get?u=https://localtest.me/"
err address not allowed
$ curl "$B/_p/fetcher/get?u=https://example.com/"
ok 200 <!doctype html><html lang=en><head><meta charset=utf-…
$ curl "$B/_p/fetcher/get?u=https://127.0.0.1/"
err host not allowed
node log: WARN … ferrite_http::plugins::egress: plugin http request refused (logged at most once a minute per plugin) plugin="fetcher" refusal="address not allowed"
```

The server was stopped with SIGTERM afterwards (`INFO ferrite::serve: shut down`).
