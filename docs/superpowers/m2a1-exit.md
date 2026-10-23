# M2a-1 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M2a-1): *the API core works against a real
`ferrite serve` on both backends: login, create, publish, preview, logout; auth and
revocation hold across nodes.*

**Result (2026-10-23, branch `m2a-api` at `48ce393` plus the Task 14 docs commit):** met,
with the limits below. The automated tests ran on both backends; the manual `curl` session
ran on the solo profile only.

| Part of the criterion | Verified by | Result |
|---|---|---|
| Login, create, publish, preview, logout against a real `ferrite serve` | manual `curl` session (solo: SQLite + local media), below; `api_end_to_end_on_real_binary` (solo and cluster profiles) | yes |
| On both backends | `api_end_to_end_on_real_binary_cluster` (Postgres 17.11 + in-process S3, one `ferrite serve` child); the full workspace with `CI=true` and Postgres | yes |
| Auth and revocation across nodes | `revocation_is_immediate_on_peer`, `login_rate_limit_spans_nodes` (`crates/ferrite-core/tests/auth.rs`: two stores on one Postgres schema, `TestDb::peer()`), `settings_reach_every_node` (`crates/ferrite/tests/cluster/reload.rs`) | yes, in process |
| OpenAPI document matches the code | `openapi_snapshot_is_current`, `openapi_json_is_valid_3_1` | yes |

## What was not verified (limits)

1. **The manual session ran on SQLite only.** No manual `curl` session was run against a
   Postgres-backed or two-node `ferrite serve`. Postgres is covered by the automated
   real-binary test (`api_end_to_end_on_real_binary_cluster`), which starts **one** node;
   cross-node revocation and rate limits are covered only by in-process tests with two
   stores on one Postgres schema, not by two `ferrite serve` processes.
2. **No `https` site.** Every run used an `http://127.0.0.1` `site.url`, so the `Secure`
   cookie attribute (sent only for `https`) was not observed on the wire; it is covered by
   the cookie unit tests and `ApiState::cookie_secure`.
3. **Not exercised by hand:** TOTP enrollment and the TOTP login step, `429` rate limits,
   password change, user management and the CSV redirect import. They are covered by the
   HTTP tests in `crates/ferrite-http/tests/api/` (both profiles) and the real-binary test.
4. **No reverse proxy, no Docker, no browser.** The CSRF and origin checks were driven with
   `curl` headers, not with a browser's cross-site form. There is no admin SPA yet (M2b).
5. **The transcripts below come from a debug build** on one laptop (Darwin 25.6.0 arm64,
   14 CPUs, rustc 1.99.1, PostgreSQL 17.11 at `localhost:54329`; CI uses `postgres:18`).
   CI has not run on this commit at the time of writing.
6. **The workspace runs were made on `48ce393`, before the Task 14 edits.** Task 14 changed
   only documentation and one doc comment (`crates/ferrite/src/seed.rs`); fmt, clippy and
   both workspace runs were repeated on the final tree with the same results (69
   test-result lines, 1576 passed, 0 failed, 1 ignored each). The plugins workspace checks
   ran on `48ce393` only (Task 14 did not touch `plugins/`).

## Automated tests

All commands from the repository root. `$PG` = `postgres://ferrite@localhost:54329/ferrite_test`.

### Full workspace, both backends

```
$ cargo fmt --all --check                                       → exit 0
$ cargo clippy --all-targets -- -D warnings                     → exit 0
$ CI=true FERRITE_TEST_PG_URL=$PG cargo test --workspace         → exit 0; 69 test-result lines, 1576 passed, 0 failed, 1 ignored
$ env -u CI -u FERRITE_TEST_PG_URL cargo test --workspace       → exit 0; 69 test-result lines, 1576 passed, 0 failed, 1 ignored (Postgres cases SKIP-pass)
$ cargo fmt --manifest-path plugins/Cargo.toml --all --check    → exit 0
$ cargo build --manifest-path plugins/Cargo.toml --release --target wasm32-wasip2   → exit 0
$ cargo clippy --manifest-path plugins/Cargo.toml --target wasm32-wasip2 -- -D warnings   → exit 0
```

The one ignored test is the `export_plugin` doc example in `crates/ferrite-sdk/src/lib.rs`
(marked `ignore`). With `CI=true` a Postgres case without `FERRITE_TEST_PG_URL` panics
instead of skipping, so the first run executed every Postgres case.

### `api_end_to_end_on_real_binary`

`crates/ferrite/tests/api.rs`: `init` → `user create --password-stdin` → `ferrite serve` on a
free port (solo: SQLite + local media; cluster: Postgres + an in-process `s3s` S3 server),
then a browser-like session (cookie, `X-CSRF-Token`, `Origin` = `site.url`) and bearer
tokens over raw HTTP/1.1: login from a foreign origin refused, wrong password and unknown
login identical `401`s, CSRF required, content create/publish/preview, tokens and the
50-token cap, user disable revoking cookie and token, logout, and no secret in any
response body except where it belongs.

```
$ CI=true FERRITE_TEST_PG_URL=$PG cargo test -p ferrite --test api -- --nocapture
running 2 tests
test api_end_to_end_on_real_binary_solo ... ok
test api_end_to_end_on_real_binary_cluster ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.34s

$ env -u CI -u FERRITE_TEST_PG_URL cargo test -p ferrite --test api -- --nocapture
SKIP: Postgres API test skipped (FERRITE_TEST_PG_URL is not set)
test api_end_to_end_on_real_binary_cluster ... ok
test api_end_to_end_on_real_binary_solo ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.95s
```

### OpenAPI snapshot

`openapi_snapshot_is_current` compares `crates/ferrite-http/openapi.json` with the document
the code generates (OpenAPI 3.1.0, 60 operations under `/api/v1`, security schemes
`session` and `bearer`); `FERRITE_UPDATE_OPENAPI=1` rewrites it.

```
$ CI=true FERRITE_TEST_PG_URL=$PG cargo test -p ferrite-http --test api openapi
test foundation::openapi_snapshot_is_current ... ok
test foundation::openapi_json_is_valid_3_1::solo ... ok
test foundation::openapi_json_is_valid_3_1::cluster ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 84 filtered out; finished in 0.21s
```

## Manual `curl` session (solo)

A fresh site (SQLite + local media) in a temporary folder `$W`, the debug binary
(`target/debug/ferrite`, built from the working tree), `site.url = http://127.0.0.1:18090`
and the server on that address, so `Origin: http://127.0.0.1:18090` is the site's origin.
The session was run by the script at the end of this section; its output is below,
unedited except that:

- the printed command lines lose their shell quoting (the script echoes `$*`);
- secrets are replaced where they appear: the password as `$PW`, the CSRF token as
  `$CSRF`, the bearer token as `$TOKEN`, the logged-out cookie as `$OLD`, and
  `ferrite_session=<43 chars>` in `Set-Cookie`; `csrf_token` and `token` members of
  bodies as `<redacted>`;
- the two long `capabilities` (15 entries) and `scopes` (22 entries) arrays of the admin
  are shortened to `[…15 capabilities…]` / `[…22 scopes…]`;
- only the status line and the headers named in the script are shown; HTML bodies are
  checked with `grep`.

```
$ ferrite --version
ferrite 0.1.0
[exit 0]

$ ferrite init --data $W/site --url http://127.0.0.1:18090 --title M2a-1 exit
2026-10-23T20:47:48.754890Z  INFO ferrite::keys: created a new site key path=$W/site/secret.key
2026-10-23T20:47:48.762131Z  INFO ferrite_store_sqlite::migrate: wrote pre-migration snapshot path=$W/site/backups/pre-0-to-5-20261023T204748Z.db
2026-10-23T20:47:48.770472Z  INFO ferrite_store_sqlite::migrate: applied migrations [1, 2, 3, 4, 5] from=0 to=5
2026-10-23T20:47:48.773676Z  INFO ferrite::init: database migrated applied=[1, 2, 3, 4, 5]
Initialised $W/site
Next steps:
  ferrite migrate status --data $W/site
  ferrite seed demo --data $W/site   (optional sample content)
  ferrite serve --data $W/site
[exit 0]

$ ls -l $W/site/secret.key
-rw------- 65 bytes

$ printf "%s\n" "$PW" | ferrite user create --data $W/site --login admin --email admin@example.com --password-stdin
created user admin (admin)
[exit 0]

$ ferrite user list --data $W/site
admin  admin  active  admin@example.com
[exit 0]

$ FERRITE_SERVER__LISTEN=127.0.0.1:18090 FERRITE_LOG=warn ferrite serve --data $W/site &
Listening on http://127.0.0.1:18090

$ curl http://127.0.0.1:18090/api/v1/meta
[curl exit 0]
HTTP/1.1 200 OK
content-type: application/json
cache-control: no-store
{"api":"v1","version":"0.1.0","site_url":"http://127.0.0.1:18090"}

$ curl http://127.0.0.1:18090/api/v1/auth/me
[curl exit 0]
HTTP/1.1 401 Unauthorized
content-type: application/problem+json
www-authenticate: Bearer
cache-control: no-store
{"type":"/api/v1/problems/unauthenticated","title":"Not authenticated","status":401,"detail":"no valid session or token"}

$ curl http://127.0.0.1:18090/api/v1/nope
[curl exit 0]
HTTP/1.1 404 Not Found
content-type: application/problem+json
cache-control: no-store
{"type":"/api/v1/problems/not-found","title":"Not found","status":404,"detail":"no such API endpoint"}

$ curl http://127.0.0.1:18090/api
[curl exit 0]
HTTP/1.1 301 Moved Permanently
location: /api/


$ curl -X POST http://127.0.0.1:18090/api/v1/auth/login -H Content-Type: application/json -H Origin: http://evil.example --data {"login":"admin","password":"$PW"}
[curl exit 0]
HTTP/1.1 403 Forbidden
content-type: application/problem+json
cache-control: no-store
{"type":"/api/v1/problems/csrf","title":"CSRF check failed","status":403,"detail":"the request does not come from the site's origin"}

$ curl -X POST http://127.0.0.1:18090/api/v1/auth/login -H Content-Type: application/json -H Origin: http://127.0.0.1:18090 --data {"login":"admin","password":"wrong password here"}
[curl exit 0]
HTTP/1.1 401 Unauthorized
content-type: application/problem+json
www-authenticate: Bearer
cache-control: no-store
{"type":"/api/v1/problems/invalid-credentials","title":"Invalid credentials","status":401,"detail":"login, password or code is not valid"}

$ curl -c $W/cookies.txt -X POST http://127.0.0.1:18090/api/v1/auth/login -H Content-Type: application/json -H Origin: http://127.0.0.1:18090 --data {"login":"admin","password":"$PW"}
[curl exit 0]
HTTP/1.1 200 OK
content-type: application/json
set-cookie: ferrite_session=<43 chars>; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000
cache-control: no-store
{"status":"session","user":{"id":"01a16a85-152c-70b1-96cc-631dbd41c122","login":"admin","email":"admin@example.com","display_name":"admin","role":"admin","must_reset":false,"totp_enabled":false,"capabilities":[…15 capabilities…],"scopes":[…22 scopes…],"csrf_token":"<redacted>"},"csrf_token":"<redacted>"}
(csrf token captured: 43 characters)

$ curl -b $W/cookies.txt http://127.0.0.1:18090/api/v1/auth/me
[curl exit 0]
HTTP/1.1 200 OK
content-type: application/json
cache-control: no-store
{"id":"01a16a85-152c-70b1-96cc-631dbd41c122","login":"admin","email":"admin@example.com","display_name":"admin","role":"admin","must_reset":false,"totp_enabled":false,"capabilities":[…15 capabilities…],"scopes":[…22 scopes…],"csrf_token":"<redacted>"}

$ curl -b $W/cookies.txt -X POST http://127.0.0.1:18090/api/v1/content/post -H Content-Type: application/json -H Origin: http://127.0.0.1:18090 --data {"title":"Exit check post","body":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Written through the API."}]}]}}
[curl exit 0]
HTTP/1.1 403 Forbidden
content-type: application/problem+json
cache-control: no-store
{"type":"/api/v1/problems/csrf","title":"CSRF check failed","status":403,"detail":"missing or invalid X-CSRF-Token"}

$ curl -b $W/cookies.txt -X POST http://127.0.0.1:18090/api/v1/content/post -H Content-Type: application/json -H Origin: http://127.0.0.1:18090 -H X-CSRF-Token: $CSRF --data {"title":"Exit check post","body":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Written through the API."}]}]}}
[curl exit 0]
HTTP/1.1 201 Created
content-type: application/json
location: /api/v1/content/post/01a16a85-18e8-71ec-b251-0b04eba23551
etag: "1"
cache-control: no-store
{"id":"01a16a85-18e8-71ec-b251-0b04eba23551","type":"post","status":"draft","slug":"exit-check-post","parent_id":null,"author_id":"01a16a85-152c-70b1-96cc-631dbd41c122","title":"Exit check post","excerpt":"Written through the API.","body":{"content":[{"content":[{"text":"Written through the API.","type":"text"}],"type":"paragraph"}],"type":"doc"},"fields":{},"menu_order":0,"published_at":null,"scheduled_at":null,"created_at":"2026-10-23T20:47:49.992934Z","updated_at":"2026-10-23T20:47:49.992934Z","version":1,"terms":[],"permalink":"/exit-check-post/"}
(id=01a16a85-18e8-71ec-b251-0b04eba23551 permalink=/exit-check-post/)

$ curl http://127.0.0.1:18090/exit-check-post/   (headers only shown)
[curl exit 0]
HTTP/1.1 404 Not Found
content-type: text/html; charset=utf-8
vary: accept-encoding

$ curl http://127.0.0.1:18090/_preview/01a16a85-18e8-71ec-b251-0b04eba23551   (headers only shown)
[curl exit 0]
HTTP/1.1 404 Not Found
content-type: text/html; charset=utf-8
vary: accept-encoding
cache-control: private, no-store
x-robots-tag: noindex

$ curl -sS -b $W/cookies.txt http://127.0.0.1:18090/_preview/01a16a85-18e8-71ec-b251-0b04eba23551 | grep -o "Exit check post\|Written through the API." | sort -u
Exit check post
Written through the API.

$ curl -b $W/cookies.txt http://127.0.0.1:18090/_preview/01a16a85-18e8-71ec-b251-0b04eba23551   (headers only shown)
[curl exit 0]
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
vary: accept-encoding
cache-control: private, no-store
x-robots-tag: noindex

$ curl -b $W/cookies.txt -X POST http://127.0.0.1:18090/api/v1/content/post/01a16a85-18e8-71ec-b251-0b04eba23551/publish -H Origin: http://127.0.0.1:18090 -H X-CSRF-Token: $CSRF
[curl exit 0]
HTTP/1.1 428 Precondition Required
content-type: application/problem+json
cache-control: no-store
{"type":"/api/v1/problems/precondition-required","title":"Precondition required","status":428,"detail":"send If-Match: \"<version>\" with the version you last read"}

$ curl -b $W/cookies.txt -X POST http://127.0.0.1:18090/api/v1/content/post/01a16a85-18e8-71ec-b251-0b04eba23551/publish -H Origin: http://127.0.0.1:18090 -H X-CSRF-Token: $CSRF -H If-Match: "7"
[curl exit 0]
HTTP/1.1 412 Precondition Failed
content-type: application/problem+json
cache-control: no-store
{"type":"/api/v1/problems/precondition-failed","title":"Version mismatch","status":412,"detail":"the resource was changed (current version 1)","current":{"author_id":"01a16a85-152c-70b1-96cc-631dbd41c122","body":{"content":[{"content":[{"text":"Written through the API.","type":"text"}],"type":"paragraph"}],"type":"doc"},"created_at":"2026-10-23T20:47:49.992934Z","excerpt":"Written through the API.","fields":{},"id":"01a16a85-18e8-71ec-b251-0b04eba23551","menu_order":0,"parent_id":null,"permalink":"/exit-check-post/","published_at":null,"scheduled_at":null,"slug":"exit-check-post","status":"draft","terms":[],"title":"Exit check post","type":"post","updated_at":"2026-10-23T20:47:49.992934Z","version":1}}

$ curl -b $W/cookies.txt -X POST http://127.0.0.1:18090/api/v1/content/post/01a16a85-18e8-71ec-b251-0b04eba23551/publish -H Origin: http://127.0.0.1:18090 -H X-CSRF-Token: $CSRF -H If-Match: "1"
[curl exit 0]
HTTP/1.1 200 OK
content-type: application/json
etag: "2"
cache-control: no-store
{"id":"01a16a85-18e8-71ec-b251-0b04eba23551","type":"post","status":"published","slug":"exit-check-post","parent_id":null,"author_id":"01a16a85-152c-70b1-96cc-631dbd41c122","title":"Exit check post","excerpt":"Written through the API.","body":{"content":[{"content":[{"text":"Written through the API.","type":"text"}],"type":"paragraph"}],"type":"doc"},"fields":{},"menu_order":0,"published_at":"2026-10-23T20:47:50.237750Z","scheduled_at":null,"created_at":"2026-10-23T20:47:49.992934Z","updated_at":"2026-10-23T20:47:50.237771Z","version":2,"terms":[],"permalink":"/exit-check-post/"}

$ curl http://127.0.0.1:18090/exit-check-post/   (headers only shown)
[curl exit 0]
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"2754522e4088c02b"
vary: accept-encoding

$ curl -sS http://127.0.0.1:18090/exit-check-post/ | grep -o "Written through the API."
Written through the API.
Written through the API.
Written through the API.

$ curl -b $W/cookies.txt http://127.0.0.1:18090/exit-check-post/   (headers only shown)
[curl exit 0]
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"2754522e4088c02b"
vary: accept-encoding
cache-control: private, no-store

$ curl -b $W/cookies.txt -X POST http://127.0.0.1:18090/api/v1/users/01a16a85-152c-70b1-96cc-631dbd41c122/tokens -H Content-Type: application/json -H Origin: http://127.0.0.1:18090 -H X-CSRF-Token: $CSRF --data {"name":"exit read-only","scopes":["content:read"]}
[curl exit 0]
HTTP/1.1 201 Created
content-type: application/json
location: /api/v1/users/01a16a85-152c-70b1-96cc-631dbd41c122/tokens/01a16a85-1ca7-758f-8867-20d26742c363
cache-control: no-store
{"token":"fer_<redacted>","id":"01a16a85-1ca7-758f-8867-20d26742c363","name":"exit read-only","scopes":["content:read"],"created_at":"2026-10-23T20:47:50.951710Z","last_used_at":null,"expires_at":null}
(token captured: prefix fer_, 47 characters)

$ curl -H Authorization: Bearer $TOKEN http://127.0.0.1:18090/api/v1/content/post?status=published&limit=1
[curl exit 0]
HTTP/1.1 200 OK
content-type: application/json
cache-control: no-store
{"items":[{"id":"01a16a85-18e8-71ec-b251-0b04eba23551","type":"post","status":"published","slug":"exit-check-post","parent_id":null,"author_id":"01a16a85-152c-70b1-96cc-631dbd41c122","title":"Exit check post","excerpt":"Written through the API.","fields":{},"menu_order":0,"published_at":"2026-10-23T20:47:50.237750Z","scheduled_at":null,"created_at":"2026-10-23T20:47:49.992934Z","updated_at":"2026-10-23T20:47:50.237771Z","version":2}],"next_cursor":null}

$ curl -X POST http://127.0.0.1:18090/api/v1/content/post -H Authorization: Bearer $TOKEN -H Content-Type: application/json --data {"title":"Exit check post","body":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Written through the API."}]}]}}
[curl exit 0]
HTTP/1.1 403 Forbidden
content-type: application/problem+json
cache-control: no-store
{"type":"/api/v1/problems/forbidden","title":"Forbidden","status":403,"detail":"missing content:write"}

$ curl -b $W/cookies.txt -c $W/cookies.txt -X POST http://127.0.0.1:18090/api/v1/auth/logout -H Origin: http://127.0.0.1:18090 -H X-CSRF-Token: $CSRF
[curl exit 0]
HTTP/1.1 204 No Content
set-cookie: ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0
cache-control: no-store


(the old cookie value is replayed from a copy taken before logout)

$ curl -H Cookie: $OLD http://127.0.0.1:18090/api/v1/auth/me
[curl exit 0]
HTTP/1.1 401 Unauthorized
content-type: application/problem+json
www-authenticate: Bearer
cache-control: no-store
{"type":"/api/v1/problems/unauthenticated","title":"Not authenticated","status":401,"detail":"no valid session or token"}

$ curl -b $W/cookies.txt http://127.0.0.1:18090/api/v1/auth/me
[curl exit 0]
HTTP/1.1 401 Unauthorized
content-type: application/problem+json
www-authenticate: Bearer
cache-control: no-store
{"type":"/api/v1/problems/unauthenticated","title":"Not authenticated","status":401,"detail":"no valid session or token"}

$ curl -b $W/cookies.txt http://127.0.0.1:18090/_preview/01a16a85-18e8-71ec-b251-0b04eba23551   (headers only shown)
[curl exit 0]
HTTP/1.1 404 Not Found
content-type: text/html; charset=utf-8
vary: accept-encoding
cache-control: private, no-store
x-robots-tag: noindex

$ curl -H Cookie: $OLD http://127.0.0.1:18090/_preview/01a16a85-18e8-71ec-b251-0b04eba23551   (headers only shown)
[curl exit 0]
HTTP/1.1 404 Not Found
content-type: text/html; charset=utf-8
vary: accept-encoding
cache-control: private, no-store
x-robots-tag: noindex

$ kill -TERM 47836
[serve exit 0]
```

What it shows:

- `GET /api/v1/meta` needs no login; every API answer has `Cache-Control: no-store`; a
  `401` carries `WWW-Authenticate: Bearer`; an unknown path under `/api/v1` is a problem
  `404`, while `/api` belongs to the public site (`301` to `/api/`).
- A login with a foreign `Origin` is `403 csrf` before the password is looked at; a wrong
  password is `401 invalid-credentials`; the right one sets `ferrite_session` with
  `HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000` and no `Secure` (an `http` site).
- A cookie write without `X-CSRF-Token` is `403 csrf`; with it the post is created (`201`,
  `Location`, `ETag: "1"`).
- The draft is `404` on its permalink and on `/_preview/<id>` without a session; with the
  session the preview is `200` with the draft's title and body, `Cache-Control: private,
  no-store` and `X-Robots-Tag: noindex`.
- `publish` without `If-Match` is `428`, with a stale version `412` carrying the current
  item, with `"1"` `200` and `ETag: "2"`. The public permalink then serves the post to an
  anonymous client (weak `ETag`, no `Cache-Control`) and to the logged-in client with
  `Cache-Control: private, no-store` (logged-in bypass).
- A `content:read` token lists content but gets `403` "missing content:write" on create.
- Logout is `204` and clears the cookie; the old cookie value is then `401` on the API and
  the shared `404` on the preview.
- `SIGTERM` stops the server with exit code 0.

The script (`bash curl-session.sh <ferrite binary> <new work dir>`):

```bash
#!/usr/bin/env bash
# Manual M2a-1 exit session: a real `ferrite serve` (solo: SQLite + local media) driven with curl.
# Usage: curl-session.sh <ferrite binary> <work dir>
set -u
BIN="$1"
W="$2"
[ -e "$W" ] && { echo "work dir exists: $W"; exit 1; }
mkdir -p "$W"
D="$W/site"
J="$W/cookies.txt"
URL="http://127.0.0.1:18090"
API="$URL/api/v1"
PW='exit check passphrase 2026'

CSRF=""; TOKEN=""; OLD=""
red() {
  local t="$*"
  t="${t//$BIN/ferrite}"; t="${t//$W/\$W}"
  [ -n "$CSRF" ] && t="${t//$CSRF/\$CSRF}"
  [ -n "$TOKEN" ] && t="${t//$TOKEN/\$TOKEN}"
  [ -n "$OLD" ] && t="${t//$OLD/\$OLD}"
  t="${t//$PW/\$PW}"
  printf '%s' "$t"
}
nocolor() { sed -E $'s/\x1b\\[[0-9;]*m//g'; }
run() { printf '\n$ %s\n' "$(red "$*")"; "$@" 2>&1 | nocolor | sed "s#$W#\$W#g"; echo "[exit ${PIPESTATUS[0]}]"; }
# curl showing status line + selected headers + body
c() {
  printf '\n$ curl %s\n' "$(red "$*")"
  curl -sS -D "$W/h.txt" -o "$W/b.txt" "$@"
  echo "[curl exit $?]"
  grep -iE '^HTTP/|^(content-type|cache-control|etag|location|retry-after|www-authenticate|set-cookie|x-robots-tag|allow):' "$W/h.txt" \
    | sed -E 's/(ferrite_session=)[A-Za-z0-9_-]{43}/\1<43 chars>/' | tr -d '\r'
  sed -E 's/"csrf_token":"[A-Za-z0-9_-]+"/"csrf_token":"<redacted>"/g; s/"token":"fer_[A-Za-z0-9_-]+"/"token":"fer_<redacted>"/g' "$W/b.txt" | head -c 1200
  echo
}
# the same, headers only (HTML bodies)
ch() {
  printf '\n$ curl %s   (headers only shown)\n' "$(red "$*")"
  curl -sS -D "$W/h.txt" -o "$W/b.txt" "$@"
  echo "[curl exit $?]"
  grep -iE '^HTTP/|^(content-type|cache-control|etag|location|x-robots-tag|vary):' "$W/h.txt" | tr -d '\r'
}

run "$BIN" --version
run "$BIN" init --data "$D" --url "$URL" --title "M2a-1 exit"
printf '\n$ ls -l $W/site/secret.key\n'
ls -l "$D/secret.key" | awk '{print $1, $5, "bytes"}'
printf '\n$ printf "%%s\\n" "$PW" | ferrite user create --data $W/site --login admin --email admin@example.com --password-stdin\n'
printf '%s\n' "$PW" | "$BIN" user create --data "$D" --login admin --email admin@example.com --password-stdin; echo "[exit $?]"
run "$BIN" user list --data "$D"

printf '\n$ FERRITE_SERVER__LISTEN=127.0.0.1:18090 FERRITE_LOG=warn ferrite serve --data $W/site &\n'
FERRITE_SERVER__LISTEN=127.0.0.1:18090 FERRITE_LOG=warn "$BIN" serve --data "$D" > "$W/serve.out" 2> "$W/serve.err" &
SERVE=$!
for _ in $(seq 1 100); do
  grep -q "Listening on" "$W/serve.out" 2>/dev/null && break
  sleep 0.1
done
cat "$W/serve.out"

# --- unauthenticated surface
c "$API/meta"
c "$API/auth/me"
c "$API/nope"
c "$URL/api"

# --- login: foreign origin refused, then a same-origin login
c -X POST "$API/auth/login" -H 'Content-Type: application/json' -H 'Origin: http://evil.example' \
  --data "{\"login\":\"admin\",\"password\":\"$PW\"}"
c -X POST "$API/auth/login" -H 'Content-Type: application/json' -H "Origin: $URL" \
  --data '{"login":"admin","password":"wrong password here"}'
c -c "$J" -X POST "$API/auth/login" -H 'Content-Type: application/json' -H "Origin: $URL" \
  --data "{\"login\":\"admin\",\"password\":\"$PW\"}"
CSRF=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["csrf_token"])' "$W/b.txt")
echo "(csrf token captured: ${#CSRF} characters)"
c -b "$J" "$API/auth/me"

# --- create a draft: without CSRF refused, with CSRF created
BODY='{"title":"Exit check post","body":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Written through the API."}]}]}}'
c -b "$J" -X POST "$API/content/post" -H 'Content-Type: application/json' -H "Origin: $URL" --data "$BODY"
c -b "$J" -X POST "$API/content/post" -H 'Content-Type: application/json' -H "Origin: $URL" -H "X-CSRF-Token: $CSRF" --data "$BODY"
ID=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$W/b.txt")
LINK=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["permalink"])' "$W/b.txt")
echo "(id=$ID permalink=$LINK)"

# --- the draft is not public; the preview shows it to the session only
ch "$URL$LINK"
ch "$URL/_preview/$ID"
printf '\n$ curl -sS -b $W/cookies.txt %s/_preview/%s | grep -o "Exit check post\\|Written through the API." | sort -u\n' "$URL" "$ID"
curl -sS -b "$J" "$URL/_preview/$ID" | grep -o 'Exit check post\|Written through the API.' | sort -u
ch -b "$J" "$URL/_preview/$ID"

# --- publish: no If-Match 428, stale 412, right version 200
c -b "$J" -X POST "$API/content/post/$ID/publish" -H "Origin: $URL" -H "X-CSRF-Token: $CSRF"
c -b "$J" -X POST "$API/content/post/$ID/publish" -H "Origin: $URL" -H "X-CSRF-Token: $CSRF" -H 'If-Match: "7"'
c -b "$J" -X POST "$API/content/post/$ID/publish" -H "Origin: $URL" -H "X-CSRF-Token: $CSRF" -H 'If-Match: "1"'
sleep 0.5
ch "$URL$LINK"
printf '\n$ curl -sS %s%s | grep -o "Written through the API."\n' "$URL" "$LINK"
curl -sS "$URL$LINK" | grep -o 'Written through the API.'
ch -b "$J" "$URL$LINK"

# --- a narrow bearer token: reads, but cannot write
ME_ID=$(curl -sS -b "$J" "$API/auth/me" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["id"])')
c -b "$J" -X POST "$API/users/$ME_ID/tokens" -H 'Content-Type: application/json' -H "Origin: $URL" -H "X-CSRF-Token: $CSRF" \
  --data '{"name":"exit read-only","scopes":["content:read"]}'
TOKEN=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$W/b.txt")
echo "(token captured: prefix ${TOKEN:0:4}, ${#TOKEN} characters)"
c -H "Authorization: Bearer $TOKEN" "$API/content/post?status=published&limit=1"
c -X POST "$API/content/post" -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' --data "$BODY"

# --- logout: the cookie stops working everywhere
cp "$J" "$W/cookies.before"
OLD=$(awk '$6=="ferrite_session"{print "ferrite_session="$7}' "$W/cookies.before")
c -b "$J" -c "$J" -X POST "$API/auth/logout" -H "Origin: $URL" -H "X-CSRF-Token: $CSRF"
printf '\n(the old cookie value is replayed from a copy taken before logout)\n'
c -H "Cookie: $OLD" "$API/auth/me"
c -b "$J" "$API/auth/me"
ch -b "$J" "$URL/_preview/$ID"
ch -H "Cookie: $OLD" "$URL/_preview/$ID"

printf '\n$ kill -TERM %s\n' "$SERVE"
kill -TERM "$SERVE"
wait "$SERVE"
echo "[serve exit $?]"
grep -iE "warn|error" "$W/serve.err" | head -5 || true
```
