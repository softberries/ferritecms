# M2a-2a exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M2a-2a): *extensions work against a real
`ferrite serve`: inspect → install with the fingerprint → enable → a plugin route →
settings with a secret → theme upload and activation → password reset through an SMTP
server; workspace tests on both backends.*

**Result (2026-10-24, branch `m2a2-extensions` at `e193cb7` plus the Task 14 commit):** met,
with the limits below. The automated tests ran on both backends; the manual `curl` session
ran on the solo profile (SQLite + local media) against a local SMTP sink.

| Part of the criterion | Verified by | Result |
|---|---|---|
| Inspect → install with the fingerprint → enable | manual session §1 (wrong approval `422`, right one `201`, enable `200`); `install_needs_matching_approval`, `widening_update_without_approval_keeps_old_version` (`crates/ferrite-http/tests/api/plugins.rs`, both profiles) | yes |
| A plugin route | manual session §2 (`/_p/router/…`: CSP sandbox, origin check, `405`, `404`, `303`, `502`, `429`); `route_xss_is_contained`, `route_response_caps` (`tests/app/plugin_route.rs`); `route_flood_is_limited_across_nodes` (`crates/ferrite/tests/cluster/routes.rs`, two nodes on Postgres) | yes |
| Settings with a secret | manual session §3 (secret write-only in the API, sealed in the database, opened by the owning plugin only); `secrets_are_write_only_and_owner_scoped` | yes |
| Theme upload and activation | manual session §4 (broken upload `422`, upload `201`, activation, tampered artifact refused `422`, theme settings); `activation_refuses_broken_theme`; `theme_fallback_is_listed_{solo,cluster}` (real binary) | yes |
| Plugin mail through SMTP | manual session §5 (`notifier` on `post.published`, received by the sink, admins in the envelope only) | yes |
| Password reset through an SMTP server | manual session §6 (no enumeration, mail received, confirm `204`, reuse `422`, old session `401`, new password logs in) | yes |
| Workspace tests on both backends | §"Automated tests" (Postgres 17.11 with `CI=true`, and without Postgres) | yes, except one flaky test (see there) |

## What was not verified (limits)

1. **The manual session ran on SQLite only.** No manual session ran against Postgres or
   two `ferrite serve` processes. Postgres and multiple nodes are covered by the automated
   tests (`theme_fallback_is_listed_cluster`, `route_flood_is_limited_across_nodes`,
   `served::*::postgres`, `actions::*::postgres`, every `api_tests!` and `profile_tests!`
   case on the cluster profile).
2. **The SMTP server was a minimal plain-SMTP sink** (a 70-line Python script on
   `127.0.0.1:2525`, no TLS, no auth; listed at the end). `smtps://` and `?tls=required`
   were not exercised against a real server; they are covered only by the config and
   `SmtpMailer::new` unit tests (R16). No mail left the machine.
3. **Not exercised by hand:** a widening update and its refusal, limits above the
   defaults, `plugin.purge` actually running after 30 days (the queued jobs were checked
   in the database only), `request.served` (`counter`), hostile plugins, the per-node
   call cap and render budget, the `Busy` refund. All of these are covered by automated
   tests (see 05 §12 and the test names in the rulings), not by this session.
4. **The reset link's page does not exist** (`/admin/reset-password` is the M2b SPA,
   F33): the token was read from the mail and posted to the confirm endpoint with `curl`.
5. **No browser.** The route CSP (`sandbox allow-forms; form-action 'self'`) was checked as
   a header only; R14 records a Chrome check during Task 8, Firefox and Safari untested.
   No reverse proxy, no Docker.
6. **Debug build** on one laptop (Darwin 25.6.0 arm64, rustc 1.99.1, PostgreSQL 17.11 at
   `localhost:54329`; CI uses `postgres:18`). CI has not run on this commit at the time of
   writing.

## Automated tests

Run on the Task 14 tree (rustc 1.99.1 after `rustup update stable`; the transcripts are in
the Task 14 report, `.superpowers/sdd/2026-10-24-m2a2-extensions/task-14-report.md`):

```
$ cargo fmt --all --check                                               # ok
$ cargo fmt --manifest-path plugins/Cargo.toml --all --check            # ok (after formatting two plugin files, see the report)
$ cargo clippy --all-targets -- -D warnings                             # ok
$ cargo build --manifest-path plugins/Cargo.toml --release --target wasm32-wasip2   # ok
$ cargo clippy --manifest-path plugins/Cargo.toml --target wasm32-wasip2 -- -D warnings  # ok
$ CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace --no-fail-fast
  run 1: 72 test binaries: 1902 passed, 0 failed, 1 ignored
  run 2: 1901 passed, 1 failed (dropped_meta_write_is_all_or_nothing::solo), 1 ignored
$ cargo test --workspace --no-fail-fast                                 # no FERRITE_TEST_PG_URL, no CI
  run 1: 1902 passed, 0 failed, 1 ignored (Postgres cases skip)
  run 2: 1901 passed, 1 failed (the same test), 1 ignored
$ cargo test -p ferrite-http --test plugins dropped_meta_write          # five more times
  ok, FAILED, ok, ok, FAILED  ("no write was dropped mid-way", main.rs:678)
```

Run 1 was on the base tree with the docs in progress, run 2 on the final tree; the only
code change between them is `cargo fmt` on two plugin sources. **`dropped_meta_write_is_all_or_nothing::solo`
(added in Task 13) is flaky:** it asserts that at least one of its timed-out `meta_set`
calls was dropped mid-way, and on SQLite the first call (a 0 µs timeout) sometimes
completes on its first poll, so no drop happens. The property it checks (no half-done
write) never failed; the test's precondition is timing-dependent. Not fixed here (Task 14
is docs); carried to the final review with the other flaky test (R12).

Real-binary and multi-node cases in that run include
`themes::theme_fallback_is_listed_{solo,cluster}`, `routes::route_flood_is_limited_across_nodes`,
`actions::post_saved_runs_meta_writer::{sqlite,postgres}`,
`served::served_counts_through_the_runner::{sqlite,postgres}`,
`served::shutdown_flushes_the_buffer::{sqlite,postgres}`, `plugin_remove_keeps_data_without_purge`
and `install_plugin_file_*::{sqlite,postgres}`.

## Manual `curl` session (solo)

Setup: a fresh data folder, `[mail] smtp_url = "smtp://127.0.0.1:2525"`, `from = "Exit
Site <noreply@exit.test>"`, `server.listen = 127.0.0.1:18080`, the demo content, one admin.
The plugin folders hold the manifests from `plugins/examples/{router,settings-probe,notifier}`
and their release components. `$O` is `Origin: http://127.0.0.1:18080`, `$C` the
`X-CSRF-Token` from the login, `$U` `http://127.0.0.1:18080/api/v1`, `$SITE` the site root.
`date`, `vary` and `content-length` headers and the session cookie value are left out;
JSON is pretty-printed or reduced with `python3` where marked (`…` = cut here).

```
$ ferrite init --data site --url http://127.0.0.1:18080 --title "Exit Site"
Initialised …/site
$ ferrite config check --data site
ok: mode=solo database=sqlite media=local workers=14 mail=configured
$ ferrite seed demo --data site
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
$ printf '%s\n' '…' | ferrite user create --data site --login root --email root@exit.test --role admin --password-stdin
created user root (admin)
$ python3 smtp_sink.py 2525 mail &          # the sink, listed at the end
$ ferrite serve --data site &
Listening on http://127.0.0.1:18080
$ curl -si -c jar -H "$O" -H "Content-Type: application/json" -d '{"login":"root","password":"…"}' $U/auth/login
HTTP/1.1 200 OK
content-type: application/json
set-cookie: ferrite_session=<redacted>; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000
cache-control: no-store
…
{"status":"session","user":{…,"role":"admin",…},"csrf_token":"…"}
```

### 1. Plugin inspect, approval, install, enable

```
$ curl -s -b jar -H "$O" -H "$C" -F manifest=@plugins/router/ferrite-plugin.toml -F component=@plugins/router/router.wasm $U/plugins/inspect
(top level, items cut)
{'plugin': 'router', 'version': '0.1.0', 'artifact': 'b35ee8ce877e6afc2055c17685969a9a24f1a15f2f3f3eb705344c50b81fef63',
 'items': '[6 items]', 'added': '[6 items]', 'limits': [],
 'fingerprint': '5e3db3a3c4262c17a81fceab09762994890d3a528261376111e28afde2c7d002', 'update': False}
(first item)
{"key": "routes", "text": "answer GET requests at /_p/router/away (default rate limit)", "detail": ["GET"], "high_risk": true}

$ curl -si -b jar -H "$O" -H "$C" -F manifest=@… -F component=@… -F approve=0000…0000 $U/plugins
HTTP/1.1 422 Unprocessable Entity
content-type: application/problem+json
{
 "type": "/api/v1/problems/validation",
 "title": "Invalid input",
 "status": 422,
 "detail": "the approval does not match this upload; inspect it again",
 "errors": [{"field": "approve", "detail": "the approval does not match this upload; inspect it again"}],
 "report": {"plugin": "router", "version": "0.1.0", "fingerprint": "5e3db3a3…2c7d002", "update": false, …}
}

$ curl -si -b jar -H "$O" -H "$C" -F manifest=@… -F component=@… -F approve=5e3db3a3c4262c17a81fceab09762994890d3a528261376111e28afde2c7d002 $U/plugins
HTTP/1.1 201 Created
content-type: application/json
location: /api/v1/plugins/router
{"name": "router", "version": "0.1.0", "enabled": false, "disabled_reason": null,
 "installed_at": "2026-10-24T08:07:12.387505Z", "updated_at": "2026-10-24T08:07:12.387505Z",
 "report": {"fingerprint": "5e3db3a3…2c7d002", "added": [], …}, "nodes": []}

$ curl -s -X POST -b jar -H "$O" -H "$C" $U/plugins/router/enable
(reduced) {'name': 'router', 'enabled': True, 'nodes': [{'node': '01a16cf2-…', 'name': 'node-32694', 'version': '0.1.0', 'status': 'disabled', 'error': 'disabled'}]}
$ curl -s -b jar $U/plugins/router        (1.5 s later; nodes only)
[{"node": "01a16cf2-…", "name": "node-32694", "version": "0.1.0", "status": "loaded", "error": null}]
```

A new plugin is installed disabled (F38); the node reported the reload right after
`plugins.changed`.

### 2. Plugin route `/_p/router/…`

```
$ curl -si $SITE/_p/router/hello
HTTP/1.1 200 OK
content-type: application/json
content-security-policy: sandbox allow-forms; form-action 'self'
x-content-type-options: nosniff
cache-control: no-store
x-robots-tag: noindex
referrer-policy: strict-origin-when-cross-origin
x-frame-options: SAMEORIGIN

{"user":null,"ip":"f3d64da9ad91c2ba7703b5e74a151fad"}
$ curl -s -b jar $SITE/_p/router/hello
{"user":"01a16cf2-3a2e-72a9-8723-28fcbaa74c6a","ip":"f3d64da9ad91c2ba7703b5e74a151fad"}
$ curl -si -X POST -d hi $SITE/_p/router/echo          # no Origin
HTTP/1.1 403 Forbidden
content-type: text/plain; charset=utf-8
cache-control: no-store
…
forbidden
$ curl -si -X POST -H "$O" -d hi $SITE/_p/router/echo
HTTP/1.1 200 OK
content-type: text/plain
content-security-policy: sandbox allow-forms; form-action 'self'
…
hi
$ curl -si -X DELETE $SITE/_p/router/hello
HTTP/1.1 405 Method Not Allowed
allow: GET
…
method not allowed
$ curl -si $SITE/_p/router/nope
HTTP/1.1 404 Not Found
…
not found
$ curl -si $SITE/_p/router/go
HTTP/1.1 303 See Other
location: /thanks/
content-security-policy: sandbox allow-forms; form-action 'self'
…
$ curl -si $SITE/_p/router/away          # the plugin answers 303 to https://evil.example/
HTTP/1.1 502 Bad Gateway
…
bad gateway
$ for i in $(seq 1 10); do curl -s -o /dev/null -w "%{http_code} " $SITE/_p/router/hello; done
200 200 200 200 200 200 200 200 429 429
$ curl -si $SITE/_p/router/hello
HTTP/1.1 429 Too Many Requests
retry-after: 21
…
too many requests
```

The route's default limit is 10/min per client (two earlier `/hello` calls plus eight).
The visitor's user id reaches the plugin from the session cookie (second call); that the
cookie and `Authorization` themselves never reach a plugin is covered by
`route_xss_is_contained`, not by this session.

### 3. Plugin settings with a secret

```
$ (inspect settings-probe) items:
! read every site option
  add tags to the head of pages
$ install with approve=<fingerprint> → 201; POST …/settings-probe/enable → 200
$ curl -s -X PATCH -b jar -H "$O" -H "$C" -H "Content-Type: application/json" -d '{"title":"Hello","limit":5,"api_key":"s3cr3t-value"}' $U/plugins/settings-probe/settings
(schema left out) {"values": {"limit": 5, "title": "Hello"}, "secrets": {"api_key": true}, "invalid": []}
$ curl -s -b jar $U/plugins/settings-probe/settings | grep -c s3cr3t
0
$ curl -s -X PATCH … -d '{"limit":"five"}' $U/plugins/settings-probe/settings
{"type":"/api/v1/problems/validation","title":"Invalid input","status":422,"detail":"must be a finite number","errors":[{"field":"limit","detail":"must be a finite number"}]}
$ curl -s $SITE/ | grep -o '<meta name="settings-probe"[^>]*>'
<meta name="settings-probe" content="title=Hello;limit=5;key=s3cr3t-value;undeclared=-;opt-a=-;opt-b=-">
$ sqlite3 -readonly site/ferrite.db "select key, value from options where key like 'plugin.settings-probe.%' order by key"
plugin.settings-probe.settings.api_key|{"sealed":"v1.WrEwOBC7oklb…"}
plugin.settings-probe.settings.limit|5
plugin.settings-probe.settings.title|"Hello"
```

`settings-probe` is a test fixture that prints what its own `settings.get` returns (the
secret included) into a meta tag. A real plugin would not do that: this only shows the
owning plugin can open the secret while the API never returns it and the database holds
it sealed. Its `undeclared` key reads as none.

### 4. Theme upload, activation, refusal, settings

```
$ tar --format=ustar --no-xattrs --no-mac-metadata -cf uploaded.tar -C upload-theme theme.toml templates/index.html
  (and broken.tar from crates/ferrite-http/tests/fixtures/broken-theme)
$ curl -si -b jar -H "$O" -H "$C" -H "Content-Type: application/x-tar" --data-binary @broken.tar $U/themes
HTTP/1.1 422 Unprocessable Entity
content-type: application/problem+json
{"type":"/api/v1/problems/validation","title":"Invalid input","status":422,"detail":"the theme does not load: template errors: templates/index.html:2: syntax error: unexpected end of block; templates/single.html:4: syntax error: unexpected end of block","errors":[{"field":null,"detail":"templates/index.html:2: syntax error: unexpected end of block"},{"field":null,"detail":"templates/single.html:4: syntax error: unexpected end of block"}]}
$ curl -si -b jar -H "$O" -H "$C" -H "Content-Type: application/x-tar" --data-binary @uploaded.tar $U/themes
HTTP/1.1 201 Created
{"name":"uploaded","version":"1.2.0","description":"A theme uploaded in the API tests.","origin":"installed","active":false,"installed_at":"2026-10-24T08:08:12.399804Z","nodes":[]}
$ curl -si -X POST -b jar -H "$O" -H "$C" $U/themes/uploaded/activate
HTTP/1.1 200 OK
{"name":"uploaded",…,"active":true,…,"nodes":[{"node":"01a16cf2-…","node_name":"node-32694","node_version":"0.1.0","name":"blog","hash":"d9925ca2e52a828d","origin":"embedded","error":null,"fallback":false}]}
$ curl -s $SITE/          (1.5 s later)
<!doctype html>
<html lang="en">
<head><title>FerriteCMS Demo</title>
…
</head>
<body data-tpl="uploaded-index">UPLOADED-THEME</body>
</html>
$ curl -s -b jar $U/themes          (10 s later: the next heartbeat)
{"items": [
  {"name": "blog", "version": "0.1.0", …, "origin": "embedded", "active": false, "installed_at": null, "nodes": []},
  {"name": "uploaded", "version": "1.2.0", …, "origin": "installed", "active": true,
   "nodes": [{"node": "01a16cf2-…", "node_name": "node-32694", "node_version": "0.1.0",
              "name": "uploaded", "hash": "e8a5e408c4d09990", "origin": "artifact", "error": null, "fallback": false}]}],
 "next_cursor": null}
$ curl -s -X PATCH -b jar -H "$O" -H "$C" -H "Content-Type: application/json" -d '{"accent_color":"#123456"}' $U/themes/blog/settings
(values) {'accent_color': '#123456', 'show_author': True, 'show_excerpts': True}
$ curl -s -X PATCH … -d '{"accent_color":"red"}' $U/themes/blog/settings
{"type":"/api/v1/problems/validation","title":"Invalid input","status":422,"detail":"must be a colour #rgb or #rrggbb","errors":[{"field":"accent_color","detail":"must be a colour #rgb or #rrggbb"}]}
$ curl -s -o /dev/null -w "%{http_code}\n" -X POST -b jar -H "$O" -H "$C" $U/themes/blog/activate
200
$ printf x >> site/media/artifacts/4b/4bab8492ea648675402300925b9c1b447fd2a8029ebaab3612cf35b5abef0372   # tamper with the stored bundle
$ curl -si -X POST -b jar -H "$O" -H "$C" $U/themes/uploaded/activate
HTTP/1.1 422 Unprocessable Entity
{"type":"/api/v1/problems/validation","title":"Invalid input","status":422,"detail":"the theme does not load: content hash mismatch: the stored bytes are not the artifact the row names"}
$ curl -s $SITE/ | grep -o "<body[^>]*>"
<body class="is-index">
$ curl -s $SITE/ | grep -o ".\{20\}#123456.\{5\}"
tyle>:root{--accent:#123456}</st
```

The activation response still showed `blog` for the node: `nodes` comes from the
heartbeat, which follows within 10 s (R10). Activating the embedded `blog` installed it as
a row (`theme.install` with `activate: true` in the audit below). After the tampering the
active theme (`blog`) kept serving; the artifact was restored afterwards.

### 5. Plugin mail through the SMTP sink

```
$ (inspect notifier) items:
  read published posts
! send e-mail to the site's administrators
$ install with approve=<fingerprint>, then enable:
201 200
$ curl -s -b jar -H "$O" -H "$C" -H "Content-Type: application/json" -d '{"title":"Exit check post"}' $U/content/post
(reduced) {'id': '01a16cf4-f8d9-765f-b56a-7ec312bbfcb4', 'status': 'draft', 'version': 1}
$ curl -s -X POST -b jar -H "$O" -H "$C" -H 'If-Match: "1"' $U/content/post/01a16cf4-f8d9-765f-b56a-7ec312bbfcb4/publish
(reduced) {'status': 'published', 'version': 2}
$ ls mail/; cat mail/mail-1.eml
mail-1.eml
X-Envelope-From: <noreply@exit.test>
X-Envelope-To: <admin@example.com>, <root@exit.test>
From: "Exit Site" <noreply@exit.test>
To: "Exit Site" <noreply@exit.test>
Subject: [Exit Site] Published: Exit check post
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: 7bit
Date: Sat, 24 Oct 2026 08:09:16 +0000

The post "Exit check post" was published.
Post id: 01a16cf4-f8d9-765f-b56a-7ec312bbfcb4

-- 
Sent by plugin notifier
```

(`X-Envelope-*` lines are written by the sink.) The publish enqueued a `plugin.action`
job in its transaction; the runner delivered it to `notifier`, whose `notify-admins` went
through the node's mail queue. Both active admins (the seed's `admin` and `root`) are
envelope recipients only: there is no `Bcc` or `To` header naming them.

### 6. Password reset by e-mail

```
$ curl -si -H "$O" -H "Content-Type: application/json" -d '{"login":"nobody@exit.test"}' $U/auth/password-reset
HTTP/1.1 202 Accepted
cache-control: no-store
…
$ curl -si -H "$O" -H "Content-Type: application/json" -d '{"login":"root@exit.test"}' $U/auth/password-reset
HTTP/1.1 202 Accepted
cache-control: no-store
…
$ curl -si -H "Content-Type: application/json" -d '{"login":"root"}' $U/auth/password-reset     # no Origin
HTTP/1.1 403 Forbidden
{"type":"/api/v1/problems/csrf","title":"CSRF check failed","status":403,"detail":"the request does not come from the site's origin"}
$ ls mail/
mail-1.eml
mail-2.eml
$ (mail-2.eml headers)
X-Envelope-From: <noreply@exit.test>
X-Envelope-To: <root@exit.test>
From: "Exit Site" <noreply@exit.test>
To: root@exit.test
Subject: [Exit Site] Password reset
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: quoted-printable
$ (mail-2.eml body, decoded; token cut here)
Someone asked to reset the password of the account "root" on Exit Site.

To choose a new password, open this link within one hour:

http://127.0.0.1:18080/admin/reset-password#token=v1.JdhVbQuIKn…

The link works once. If you did not ask for this, ignore this mail: your password stays the same.

$ curl -si -H "$O" -H "Content-Type: application/json" -d '{"token":"<from the mail>","password":"short"}' $U/auth/password-reset/confirm
HTTP/1.1 422 Unprocessable Entity
{"type":"/api/v1/problems/validation","title":"Invalid input","status":422,"detail":"password must be at least 12 characters","errors":[{"field":"password","detail":"password must be at least 12 characters"}]}
$ curl -si -H "$O" -H "Content-Type: application/json" -d '{"token":"<from the mail>","password":"…"}' $U/auth/password-reset/confirm
HTTP/1.1 204 No Content
cache-control: no-store
…
$ (the same token again)
HTTP/1.1 422 Unprocessable Entity
{"type":"/api/v1/problems/validation","title":"Invalid input","status":422,"detail":"the reset link is invalid or has expired","errors":[{"field":"token","detail":"the reset link is invalid or has expired"}]}
$ curl -s -o /dev/null -w "%{http_code}\n" -b jar $U/auth/me      # the session from before the reset
401
$ login with the old password, then the new one:
401 200
```

The unknown address got the same `202` and no mail (two mails in total: the notifier's
and this one).

### 7. Delete, editors, audit, CLI

```
$ curl -si -X DELETE -b jar -H "$O" -H "$C" $U/plugins/router
HTTP/1.1 204 No Content
$ curl -s -o /dev/null -w "%{http_code}\n" $SITE/_p/router/go
404
$ curl -s -b jar $U/plugins | (names)
[('notifier', True), ('settings-probe', True)]
$ sqlite3 -readonly site/ferrite.db "select kind, status, run_at from jobs where kind like 'plugin.purge%'"; … options like 'plugin.router.%'
plugin.purge_sweep|done|1792800000000000
plugin.purge|queued|1795421391650766
plugin.router.deleted_at|1792829391650766
```

The purge job is due exactly 30 days (2 592 000 s) after the deletion marker.

```
$ curl -s … -d '{"login":"ed","email":"ed@exit.test","display_name":"Ed","role":"editor","password":"…"}' $U/users
201
$ (login as ed) 200
$ curl -s -b edjar $U/plugins
{"type":"/api/v1/problems/forbidden","title":"Forbidden","status":403,"detail":"missing manage_plugins"}
$ curl -s -b edjar $U/themes
{"type":"/api/v1/problems/forbidden","title":"Forbidden","status":403,"detail":"missing manage_themes"}
$ sqlite3 -readonly site/ferrite.db "select action, details from audit_log order by at"   (artifacts cut, grants left out)
user.create {"login": "root", "must_reset": false, "password": true, "role": "admin"}
plugin.install {"artifact": "b35ee8ce877e…", "granted": "{…}", "name": "router", "version": "0.1.0"}
plugin.enable {"name": "router"}
plugin.install {"artifact": "7120611a492b…", "granted": "{…}", "name": "settings-probe", "version": "0.1.0"}
plugin.enable {"name": "settings-probe"}
plugin.settings {"keys": ["api_key", "limit", "title"], "name": "settings-probe"}
theme.install {"activate": false, "artifact": "4bab8492ea64…", "name": "uploaded", "version": "1.2.0"}
theme.activate {"artifact": "4bab8492ea64…", "name": "uploaded"}
theme.settings {"keys": ["accent_color"], "name": "blog"}
theme.install {"activate": true, "artifact": "5aa29485c566…", "name": "blog", "version": "0.1.0"}
plugin.install {"artifact": "e1f91eb921b1…", "granted": "{…}", "name": "notifier", "version": "0.1.0"}
plugin.enable {"name": "notifier"}
user.password_reset {}
plugin.delete {"name": "router", "purge": false}
user.create {"login": "ed", "must_reset": false, "password": true, "role": "editor"}
```

No audit row holds a setting value or the reset token. Then the operator CLI:

```
$ ferrite plugin list --data site          # while serve runs
…
Caused by:
    database error: data folder is in use: site/ferrite.lock is locked by another process
$ kill -TERM <serve pid>                    # stopped
$ ferrite plugin list --data site
notifier  0.1.0  enabled
settings-probe  0.1.0  enabled
$ ferrite theme list --data site
blog  0.1.0  installed  active
uploaded  1.2.0  installed
$ ferrite plugin check --data site --dir plugins/router
plugin router 0.1.0 (new)
component: b35ee8ce877e6afc2055c17685969a9a24f1a15f2f3f3eb705344c50b81fef63
permissions:
! routes: answer GET requests at /_p/router/away (default rate limit) [GET]
! routes: answer GET requests at /_p/router/big (default rate limit) [GET]
! routes: answer POST requests at /_p/router/echo (default rate limit) [POST]
! routes: answer GET requests at /_p/router/go (default rate limit) [GET]
! routes: answer GET requests at /_p/router/hello (default rate limit) [GET]
! routes: answer POST requests at /_p/router/whoami (default rate limit), from other sites too [POST]
fingerprint: 5e3db3a3c4262c17a81fceab09762994890d3a528261376111e28afde2c7d002
ok: nothing was stored
$ ferrite plugin install --data site --dir plugins/router < /dev/null
…
fingerprint: 5e3db3a3c4262c17a81fceab09762994890d3a528261376111e28afde2c7d002
Error: `plugin install` needs --yes when stdin is not a terminal; nothing was installed
(exit 1)
$ ferrite plugin install --data site --dir plugins/router --yes
…
installed plugin router 0.1.0 (disabled)
$ sqlite3 -readonly site/ferrite.db "select count(*) from options where key like 'plugin.router.%'"
0                                           # the reinstall removed the deletion marker
$ ferrite plugin remove --data site router --purge
removed plugin router and purged its data
$ sqlite3 -readonly site/ferrite.db "select status, unique_key, run_at from jobs where kind = 'plugin.purge'"   (run_at as offset from the deletion)
queued plugin.purge:router:1792829391650766 due 2592000 s after the deletion
queued plugin.purge:router:1792829450136352 due 120 s after the deletion
```

The CLI and the API produced the same fingerprint for the same upload. The first purge
job (from the API delete) will find no matching marker and do nothing; the second is the
`--purge` final sweep (R5).

### The SMTP sink

`smtp_sink.py <port> <dir>`: accepts plain SMTP on `127.0.0.1`, answers `EHLO`/`HELO`,
`MAIL FROM`, `RCPT TO`, `DATA`, `RSET`, `NOOP`, `QUIT`, and writes each message to
`<dir>/mail-<n>.eml` with two `X-Envelope-From` / `X-Envelope-To` lines in front. No TLS,
no AUTH (it offers neither, so lettre sends in plaintext to the loopback relay, which R16
allows without credentials). It is not in the repository; the full script:

```python
"""Minimal SMTP sink for the M2a-2a exit check: accepts plain SMTP on 127.0.0.1,
writes every message (envelope + data) to <outdir>/mail-<n>.eml. No auth, no TLS."""
import socket
import sys
import threading

port = int(sys.argv[1])
outdir = sys.argv[2]
counter = [0]
lock = threading.Lock()


def handle(conn):
    f = conn.makefile("rwb")

    def send(line):
        f.write(line.encode() + b"\r\n")
        f.flush()

    send("220 sink ESMTP")
    mail_from, rcpts = None, []
    while True:
        line = f.readline()
        if not line:
            break
        cmd = line.decode(errors="replace").strip()
        up = cmd.upper()
        if up.startswith("EHLO") or up.startswith("HELO"):
            f.write(b"250-sink\r\n250 8BITMIME\r\n")
            f.flush()
        elif up.startswith("MAIL FROM:"):
            mail_from, rcpts = cmd[10:].strip(), []
            send("250 ok")
        elif up.startswith("RCPT TO:"):
            rcpts.append(cmd[8:].strip())
            send("250 ok")
        elif up == "DATA":
            send("354 go")
            data = []
            while True:
                d = f.readline()
                if d in (b".\r\n", b".\n", b""):
                    break
                data.append(d)
            with lock:
                counter[0] += 1
                n = counter[0]
            with open(f"{outdir}/mail-{n}.eml", "wb") as out:
                out.write(f"X-Envelope-From: {mail_from}\r\n".encode())
                out.write(f"X-Envelope-To: {', '.join(rcpts)}\r\n".encode())
                out.write(b"".join(data))
            send("250 queued")
        elif up == "QUIT":
            send("221 bye")
            break
        elif up == "RSET" or up == "NOOP":
            send("250 ok")
        else:
            send("502 not implemented")
    conn.close()


s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", port))
s.listen()
print(f"sink listening on 127.0.0.1:{port}", flush=True)
while True:
    c, _ = s.accept()
    threading.Thread(target=handle, args=(c,), daemon=True).start()
```
