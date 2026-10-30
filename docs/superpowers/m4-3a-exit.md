# M4-3a exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M4-3; plan
[`2026-10-30-m4-3a-plugins-doctor.md`](plans/2026-10-30-m4-3a-plugins-doctor.md), Task 8,
ruling P41): *three example plugins (SEO, privacy-friendly analytics, a contact form) built
inside world `0.1.0`, shipped as a signed release asset, and `ferrite doctor`. The exit
installs the plugins from the packed asset on a fresh site, checks each one through the
running server (SEO tags and JSON-LD, analytics counts on the stats page as a listed viewer,
a contact-form post reaching an SMTP sink and the inbox), runs doctor in several states on
SQLite and Postgres with credentials checked absent, and runs the full gates.*

**Result (2026-10-30, branch `m4-3a-exit` from `m4-3` at `9d3ccd0`):** every exit check
passed on macOS (arm64) with local PostgreSQL 17.11 (port 54329). Nothing was found that
needed a code fix. Nothing was pushed, tagged or published.

| Exit check | Verified by | Result |
|---|---|---|
| (a) Release binary with the admin, the plugins packed by `package-plugins.sh`, installed from the extracted asset on an `init` + `seed demo` site; SEO tags and JSON-LD; analytics counts as a listed viewer; contact form to an SMTP sink and the inbox; a hostile post | §1 (a) | yes |
| (b) `ferrite doctor` while `serve` runs, after it stops, `--offline`, on Postgres (running, stopped, missing schema), on broken folders; no credential in any output | §1 (b) | yes |
| (c) Gates: fmt ×2, clippy ×2, rustdoc, the workspace tests with Postgres (0 failed, 0 SKIP), the admin gates | §2 | yes: 111 test binaries, 3625 passed, 0 failed, 1 ignored, 0 SKIP |

## What shipped

Per task (merges into `m4-3`; rulings in [`rulings/m4.md`](rulings/m4.md), "M4-3a plan
rulings" P41–P53 and "M4-3a execution rulings" R-pf1 and X1–X6):

1. **SDK helpers** (`9b8796b`, P42): guest-side source only, no WIT or host change —
   `form` (`parse_urlencoded`), `html::escape`, `options::get`, `mail::notify_admins`,
   `route::html`/`text`, `viewers_allow` (P46) and `Head` additions for the head filter
   (05 §10).
2. **SEO plugin** (`81ffc14`, `3469180`; P43, X1): `plugins/examples/seo`, `render.head` at
   priority 20, adding only what core does not emit and never a tag already in the head:
   `og:site_name`, `og:locale` (not for a bare language), Twitter cards, `article:*` for
   posts, JSON-LD (`WebSite` with `SearchAction` on `/`, `BlogPosting` with an
   `Organization` publisher), verification metas, the RSS discovery link. One host change
   (X1): `options.get("site.url")` answers the configured URL, `site.title` the stored
   option else the configured title, both behind the `options.read` grant.
3. **Analytics plugin** (`9ddbe38`, `61e84df`; P44, P46, X5): `plugins/examples/analytics`,
   a `request.served` subscriber keeping one JSON blob per UTC day in KV, a script-free stats
   page at `/_p/analytics/?days=1|7|30|90` for the users in `viewers`. Core now
   percent-encodes every served path (X5). Fuel shaped it (hash maps, clipped keys, a decode
   budget per batch and per page, at most three days per batch; 05 §12).
4. **Contact form plugin** (`66fa13a`, `128babf`; P45, P46, X6): `plugins/examples/contact-form`,
   a sandboxed form page, `POST /submit` (`cross_origin`, `10/h`), a viewer-guarded inbox;
   mail through `notify-admins`, the latest 100 messages within 900 KiB as JSON lines (X6).
5. **Release asset** (`484a1bf`, `9d3ccd0`; P47): `.github/scripts/package-plugins.sh
   VERSION OUT_DIR` writes `ferrite-plugins-<version>.tar.gz` (`seo/`, `analytics/`,
   `contact-form/`, each `ferrite-plugin.toml`, `<name>.wasm`, `README.md`); `release.yml`
   job `plugins`; `sums.sh` expects 7 assets; the CI `packaging` job builds and checks it.
6. **`ferrite doctor`, framework and file checks** (`b0b5752`, `a005015`; P48, P49, P53, X2):
   `crates/ferrite/src/doctor/`, the output format and exit codes, the `version`, `config`
   and `fs.*` checks with `access(2)`; `init` creates a new data folder 0750 and the image's
   `/data` is 0750 (X2).
7. **`ferrite doctor`, the rest** (`d35562f`, `3ac1b51`; P50–P53, X3, X4): `db.*`,
   `upgrade.record`, `media.*`, `clock.*`, `cluster.*`, `smtp.connect`, `tls.*`,
   `net.listen`, `backup.target`, `release.latest`; SQLite read through a read-only,
   lock-free connection (X3); no local media probe (X4); every database and S3 error
   scrubbed.

No migration. No WIT change (the world stays `0.1.0`).

## 1. End-to-end checks

Scratch area `$E = /private/tmp/claude-501/-Users-krzysztofgrajek-Projects-rust-ferritecms/a70e2d1a-a8e1-480a-80ba-c15c966313df/scratchpad/m43a-e2e`
(left in place). The SMTP sink is a 60-line asyncio script
(`$E/../m43a-scripts/smtp_sink.py`: `EHLO` with `AUTH PLAIN LOGIN`, accepts any
credentials, appends each message as a JSON line to `$E/mail.jsonl`); the repository's own
sink lives inside a Rust test (`crates/ferrite-http/tests/plugins/mail.rs`) and serves one
session, and `aiosmtpd` is not installed.

### (a) Plugins from the packed asset

**Build.** `pnpm -C admin install --frozen-lockfile && pnpm -C admin build`, `cargo build
--release -p ferrite`, `cargo build --release --target wasm32-wasip2 --manifest-path
plugins/Cargo.toml` (the workspace, as the brief says; the release job builds only the
three), then `.github/scripts/package-plugins.sh 0.1.0 $E/asset`:

```
packed …/m43a-e2e/asset/ferrite-plugins-0.1.0.tar.gz:
drwxr-xr-x  0 krzysztofgrajek staff       0 Oct 30 20:23 seo/
-rw-r--r--  0 krzysztofgrajek staff  211911 Oct 30 20:23 seo/seo.wasm
-rw-r--r--  0 krzysztofgrajek staff    1287 Oct 30 20:23 seo/ferrite-plugin.toml
-rw-r--r--  0 krzysztofgrajek staff    6693 Oct 30 20:23 seo/README.md
drwxr-xr-x  0 krzysztofgrajek staff       0 Oct 30 20:23 analytics/
-rw-r--r--  0 krzysztofgrajek staff    1243 Oct 30 20:23 analytics/ferrite-plugin.toml
-rw-r--r--  0 krzysztofgrajek staff   13420 Oct 30 20:23 analytics/README.md
-rw-r--r--  0 krzysztofgrajek staff  349170 Oct 30 20:23 analytics/analytics.wasm
drwxr-xr-x  0 krzysztofgrajek staff       0 Oct 30 20:23 contact-form/
-rw-r--r--  0 krzysztofgrajek staff    1663 Oct 30 20:23 contact-form/ferrite-plugin.toml
-rw-r--r--  0 krzysztofgrajek staff   14869 Oct 30 20:23 contact-form/README.md
-rw-r--r--  0 krzysztofgrajek staff  275932 Oct 30 20:23 contact-form/contact-form.wasm
```

sha256 `3280e30ecd3922440d71e50a218f75a4f6a076402ace8b32e8f71c9314d0319e`; extracted with
`tar -xzf` into `$E/plugins/`. The layout matches the three READMEs' Install sections (each
folder is the `--dir`; the crate's `contact_form.wasm` is `contact-form.wasm` there). Two
README prose lines were reflowed after packing (no content change).

**Site.** `ferrite init --data ./site --url http://localhost:8091` and `ferrite seed demo`
("12 posts, 3 pages, 4 terms, 2 menus, 1 image"); `server.listen = 127.0.0.1:8091`;
`[mail] smtp_url = "smtp://sinkuser:<password>@127.0.0.1:2526"`, `from = "My Site
<noreply@localhost.test>"` (`config check`: `mail=configured`); an admin `me` from
`user create --password-stdin`.

**Install** (`serve` stopped): `ferrite plugin check --dir plugins/seo` printed the report
and `ok: nothing was stored`; then for each plugin `ferrite plugin install --data ./site
--dir plugins/<name> --enable --yes` (exit 0; `--yes` approves the printed report, which
non-interactive stdin requires):

```
plugin seo 0.1.0 (new)
component: c7bd822425b2456570c2c72fdecf50177f5d436351075e27e51508d5b18877b6
permissions:
  content.read: read published posts [post]
  options.read: read the site options `site.title` and `site.url` [site.title, site.url]
! render.head: add tags to the head of every page, including scripts from any https:// host, which run with the site's origin (as a logged-in admin when one views the site)
fingerprint: 8e1f8bac113fb4308fd0da6b04cbf9ceb5d30c992979b9b4b8821aea44916849
installed plugin seo 0.1.0 (enabled)
plugin analytics 0.1.0 (new)
component: 80271d2f730093c0294ed39a6a77ea4b945c560f7163e8a07fd9f3a993a79056
permissions:
  options.read: read the site options `site.url` [site.url]
  kv: store its own data on the site
! routes: answer GET requests at /_p/analytics/ (at most 30/min per visitor) [GET]
~ hooks: receives every page view of the site (path, status, time, referrer host, browser class, cache outcome) [request.served]
fingerprint: 5c6429fffff97ddf3f35e8da3b0a419861bff359efd997a0099d6386f8bcdcfc
installed plugin analytics 0.1.0 (enabled)
plugin contact-form 0.1.0 (new)
component: 58ecacd2bd2244c14d4f6c7de059a993fd6cfb0417a97e16b520c5b837c8c770
permissions:
  options.read: read the site options `site.title` [site.title]
  kv: store its own data on the site
! routes: answer GET requests at /_p/contact-form/ (default rate limit) [GET]
! routes: answer GET requests at /_p/contact-form/inbox (default rate limit) [GET]
! routes: answer POST requests at /_p/contact-form/submit (at most 10/h per visitor), from other sites too [POST]
! mail: send e-mail to the site's administrators
fingerprint: bcedab4df395cd57051fe7b947449512dab4f36c54f61885b2fc69c8f05f3691
installed plugin contact-form 0.1.0 (enabled)
```

`serve` logged `plugins loaded loaded=3 skipped=0`. Login: `POST /api/v1/auth/login` with
`Origin: http://localhost:8091` → 200 with a session cookie and `csrf_token`. Settings by
`PATCH /api/v1/plugins/{name}/settings` with the cookie and `X-CSRF-Token` (each 200): SEO
`twitter_site = "@ferritecms"`, `organization_name = "Ferrite Demo Org"`,
`google_verification`, `bing_verification`; analytics and contact-form `viewers = <me's
id>`.

**SEO.** `GET /welcome-to-ferritecms/` → 200. Head tags (core's first, then the plugin's):

```
<meta property="og:title" content="Welcome to FerriteCMS">
<meta property="og:url" content="http://localhost:8091/welcome-to-ferritecms/">
<meta property="og:type" content="article">
<meta property="og:image" content="http://localhost:8091/_media/e619…/sample-gradient.png">
<meta property="og:site_name" content="FerriteCMS Demo">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="Welcome to FerriteCMS">
<meta name="twitter:description" content="This site runs on a single binary …">
<meta name="twitter:site" content="@ferritecms">
<meta property="article:published_time" content="2026-10-30T18:24:21.052823+00:00">
<meta property="article:author" content="Site Admin">
<meta property="article:section" content="News">
<meta property="article:tag" content="cms">
<meta name="google-site-verification" content="g-verify-123">
<meta name="msvalidate.01" content="b-verify-456">
<link rel="canonical" href="http://localhost:8091/welcome-to-ferritecms/">
<link rel="alternate" href="http://localhost:8091/feed/" type="application/rss+xml">
```

No `og:locale` (the site locale is the bare `en`, P43); one copy of each tag. The one
`<script type="application/ld+json">` block parsed with `python3 -m json.tool`: a
`BlogPosting` (`headline`, `url` = the canonical, `datePublished`, `image`, `author`
`Person` "Site Admin", `publisher` `Organization` "Ferrite Demo Org"). On `/` the block is a
`WebSite` with a `SearchAction` (`target` `http://localhost:8091/?s={search_term_string}`),
also valid JSON. `/about/` (a page) has `og:site_name` but no `article:*` and no JSON-LD.

**Analytics.** Twenty requests with a Chrome user agent and `Referer:
https://news.example.org/item?id=1`: 19 published pages (three of them twice) → 200 and
`/no-such-page/` → 404, plus one `Googlebot/2.1` request to `/`. After 20 s,
`GET /_p/analytics/?days=1` with the session cookie → 200 (no `<script>` in the page), text:

```
UTC days 2026-10-30 to 2026-10-30. A page view is a browser request answered 200 or 304.
19 page views · 1 bot requests · 4 other clients · 1 days shown
Responses: 24 2xx · 0 3xx · 1 4xx · 0 5xx
Top pages: / 2 · /about/ 2 · /welcome-to-ferritecms/ 2 · then 13 paths with 1
Top referrers: news.example.org 19
```

All counts are exact: 19 browser 200s are views; the 404 is a 4xx and not a view; the bot is
one bot request; the 4 "other clients" are the earlier `curl`-agent fetches of `/`, the post,
`/about/` and `/sitemap.xml` (2xx 24 = 19 + 1 + 4). Anonymous → 403 "Sign in to the admin
first, then open this page again."

**Contact form.** `GET /_p/contact-form/` → 200 with `content-security-policy: sandbox
allow-forms; form-action 'self'`. `POST /_p/contact-form/submit` (urlencoded, `Origin:
http://localhost:8091`, `name=Ada Lovelace`, a two-line message, empty `website`) → 200
"Thank you … Your message was sent." The sink received one message, `MAIL FROM
<noreply@localhost.test>`, `RCPT TO` both active admins (`admin@example.com`,
`me@localhost.test`), `Subject: [FerriteCMS Demo] [Contact] Hello from the e2e`, a
plain-text body with name, e-mail, subject, time and the message. A JSON body → 415.
`GET /_p/contact-form/inbox` with the cookie → 200, "2 messages, newest first" (after the
hostile post below); anonymous → 403.

**Hostile post:** `name=<script>alert(1)</script>Mallory`, `subject=Hi\r\nBcc:
victim@example.net\r\nX-Evil: 1`, a message with `<img src=x onerror=alert(2)>` and a line
holding a lone `.` followed by `MAIL FROM:<x@y>` → 200. In the sink: still only the two admin
recipients (no `victim@example.net`), no `X-Evil` header, `Subject: [FerriteCMS Demo]
[Contact] HiBcc: victim@example.netX-Evil: 1` (CR/LF stripped, P45), the lone `.`
dot-stuffed to `..` on the wire, the body plain text. In the inbox HTML the name is
`&lt;script&gt;alert(1)&lt;/script&gt;Mallory` and the body `&lt;img src=x
onerror=alert(2)&gt;`; the page has no `<script>` element and is served under the same
sandbox CSP.

### (b) `ferrite doctor`

The release binary, `ferrite doctor --data <folder> [--offline]`, exit code appended.
Credentials present in the configs: the SMTP user and password, the Postgres password
(`postgres://ferrite:<password>@…`; the local server trusts, so the password is accepted
and unused), the two `secret.key`s and the admin's password. `grep` for each of them over
all nine outputs: 0 files; no output contains `smtp://` or `postgres://`.

**SQLite site while `serve` runs** (exit 0):

```
info   version.binary  ferrite 0.1.0 (aarch64-apple-darwin)
ok     config.load  mode=solo database=sqlite media=local workers=14 mail=configured metrics=off tls=off backup=off
ok     fs.data_dir  ./site (mode 0750, readable and writable, owner uid 501)
ok     fs.config_file  ./site/ferrite.toml (mode 0600)
ok     fs.secret_key  ./site/secret.key is a valid key
ok     fs.database_files  ferrite.db, ferrite.db-wal, ferrite.db-shm, ferrite.lock are readable and writable
ok     fs.backups_dir  ./site/backups is writable, 1 pre-migration snapshot(s)
info   fs.themes_dir  no ./site/themes folder (optional)
info   db.lock_state  a node is running (it holds ferrite.lock in the data folder)
ok     db.connect  SQLite ./site/ferrite.db, read-only, beside the running node, 5 ms
ok     db.schema  schema 12, up to date
ok     db.identity  site 01a18e45-2e75-75a3-b01f-4cd8f3f596b3, install 01a18e45-2e75-75a3-b01f-4cd9fa91aaa1, last started by ferrite 0.1.0
ok     upgrade.record  no upgrade.json (nothing to roll back to)
ok     media.store  local ./site/media: a writable folder (checked with access(2); no probe written)
ok     media.owner  owned by this install (01a18e45-2e75-75a3-b01f-4cd9fa91aaa1)
skip   clock.db_skew  SQLite uses this machine's clock: nothing to compare
skip   clock.peers  solo SQLite site: no other node can share the database
skip   cluster.versions  solo SQLite site: no other node can share the database
skip   cluster.site_url  solo SQLite site: no other node can share the database
ok     smtp.connect  127.0.0.1:2526 (plain, no TLS): the server accepted a session
info   tls.dir  TLS is off (server.tls.mode)
info   tls.acme_directory  TLS is off (server.tls.mode)
ok     net.listen  http://127.0.0.1:8091: /healthz 200, /readyz 200
info   backup.target  no [backup] target configured
info   release.latest  could not look up the newest release: no release found at https://api.github.com/repos/softberries/ferritecms/releases/latest
doctor: 14 ok, 0 warn, 0 error, 4 skip
exit=0
```

It took 0.49 s, and `serve` kept serving (it was stopped afterwards with SIGTERM: "shut
down").

**After `serve` stopped** (exit 0): the same lines except

```
ok     fs.database_files  ferrite.db, ferrite.lock are readable and writable
info   db.lock_state  no node is running on this data folder
ok     db.connect  SQLite ./site/ferrite.db, read-only (no -wal file: opened immutable), 1 ms
skip   net.listen  no node is running on this data folder
doctor: 13 ok, 0 warn, 0 error, 5 skip
exit=0
```

The folder held the same files before and after (`backups`, `ferrite.db`, `ferrite.lock`,
`ferrite.toml`, `media`, `secret.key`: no `-wal`/`-shm` left, X3).

**`--offline`** (exit 0): as after the stop, except

```
skip   smtp.connect  127.0.0.1:2526 (plain, no TLS): not tried (--offline)
skip   release.latest  the release lookup is not made (--offline)
doctor: 12 ok, 0 warn, 0 error, 7 skip
exit=0
```

**Postgres site** (`ferrite init --data ./pgsite --url http://localhost:8092 --db
'postgres://ferrite:<password>@localhost:54329/ferrite_test' --schema m43a_e2e_doctor`,
`cluster.node_name = "e2e-pg"`), while `serve` runs (exit 0):

```
info   version.binary  ferrite 0.1.0 (aarch64-apple-darwin)
ok     config.load  mode=solo database=postgres media=local workers=14 mail=off metrics=off tls=off backup=off
ok     fs.data_dir  ./pgsite (mode 0750, readable and writable, owner uid 501)
ok     fs.config_file  ./pgsite/ferrite.toml (mode 0600)
ok     fs.secret_key  ./pgsite/secret.key is a valid key
info   fs.database_files  Postgres: the database is not in the data folder
info   fs.backups_dir  Postgres: no pre-migration snapshots are taken
info   fs.themes_dir  no ./pgsite/themes folder (optional)
info   db.lock_state  Postgres: there is no data-folder lock
ok     db.connect  Postgres localhost:54329/ferrite_test (schema m43a_e2e_doctor), server 17.11 (Homebrew), 6 ms
ok     db.schema  schema 12, up to date
ok     db.identity  site 01a18e49-2bce-766d-b025-9cd23934cc89, install 01a18e49-2bce-766d-b025-9cd3703f1423, last started by ferrite 0.1.0
ok     upgrade.record  no upgrade.json (nothing to roll back to)
ok     media.store  local ./pgsite/media: a writable folder (checked with access(2); no probe written)
ok     media.owner  owned by this install (01a18e49-2bce-766d-b025-9cd3703f1423)
ok     clock.db_skew  this machine's clock is 0 ms ahead of the database's
ok     clock.peers  1 live node(s), each within 2 s of the database clock
ok     cluster.versions  1 live node(s) on ferrite 0.1.0 (schema 12)
ok     cluster.site_url  1 live node(s) agree with this configuration (http://localhost:8092, solo mode)
info   smtp.connect  mail is off ([mail] smtp_url is not set)
info   tls.dir  TLS is off (server.tls.mode)
info   tls.acme_directory  TLS is off (server.tls.mode)
ok     net.listen  http://127.0.0.1:8092: /healthz 200, /readyz 200
info   backup.target  no [backup] target configured
info   release.latest  could not look up the newest release: no release found at https://api.github.com/repos/softberries/ferritecms/releases/latest
doctor: 15 ok, 0 warn, 0 error, 0 skip
exit=0
```

After `serve` stopped (exit 0), the differences:

```
info   clock.peers  no live node in the registry
info   cluster.versions  no live node in the registry
info   cluster.site_url  no live node in the registry
skip   net.listen  no live node has this cluster.node_name
doctor: 11 ok, 0 warn, 0 error, 1 skip
```

Before the first `serve` (no `node_name` yet, no owner marker) it also said `info
media.owner  no ferrite-owner.json yet: the first media GC pass writes one for this install`
and `skip net.listen  cluster.node_name is not set, so doctor cannot tell whether a node runs
for this folder` (10 ok, 1 skip, exit 0).

**Postgres site whose schema is missing** (a copy of `pgsite` with `schema =
"m43a_e2e_missing"`, `--offline`; exit 1):

```
error  db.connect  localhost:54329/ferrite_test (schema m43a_e2e_missing): the schema m43a_e2e_missing does not exist; doctor does not create it (`ferrite init --db … --schema …` or `ferrite migrate up` does)
skip   db.schema  the database did not open (see db.connect)
skip   db.identity  the database did not open (see db.connect)
skip   media.owner  the database did not open (see db.connect)
skip   clock.db_skew  the database did not open (see db.connect)
skip   clock.peers  the database did not open (see db.connect)
skip   cluster.versions  the database did not open (see db.connect)
skip   cluster.site_url  the database did not open (see db.connect)
skip   net.listen  the node registry was not read (see db.connect)
doctor: 6 ok, 0 warn, 1 error, 9 skip
exit=1
```

(the other lines as for `pgsite`). Afterwards `select count(*) from pg_namespace where
nspname = 'm43a_e2e_missing'` → 0: doctor did not create it (P49).

**Broken: `secret.key` 0644** (a copy of `site`, `chmod 0644 secret.key`, `--offline`; exit 2):

```
warn   fs.secret_key  ./broken-key/secret.key has mode 0644: readable by group or others; chmod 600 it
doctor: 11 ok, 1 warn, 0 error, 7 skip
exit=2
```

(every other line as in the `--offline` run above, with `./broken-key`).

**Broken: no `ferrite.toml`** (an empty 0750 folder; exit 1):

```
info   version.binary  ferrite 0.1.0 (aarch64-apple-darwin)
error  config.load  ./broken-noconf/ferrite.toml not found (create the site with `ferrite init`)
ok     fs.data_dir  ./broken-noconf (mode 0750, readable and writable, owner uid 501)
skip   fs.config_file  ./broken-noconf/ferrite.toml not found
skip   fs.secret_key  the configuration did not load (see config.load)
… (every check that needs the configuration: skip, "the configuration did not load (see config.load)")
ok     upgrade.record  no upgrade.json (nothing to roll back to)
info   release.latest  could not look up the newest release: no release found at https://api.github.com/repos/softberries/ferritecms/releases/latest
doctor: 2 ok, 0 warn, 1 error, 20 skip
exit=1
```

The folder was still empty afterwards (doctor created nothing).

`release.latest` answers `info` in every online run: the repository has no published
release, and the lookup never yields an error (P51).

## 2. Gates

At `9d3ccd0` plus this exit's docs (logs in the scratchpad: `m43a-test.log`,
`m43a-gates.log`, `m43a-admin.log`):

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | ok |
| `cargo fmt --manifest-path plugins/Cargo.toml --all --check` | ok |
| `cargo clippy --workspace --all-targets -- -D warnings` | ok |
| `cargo clippy --manifest-path plugins/Cargo.toml --target wasm32-wasip2 -- -D warnings` | ok |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | ok |
| `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace` | exit 0: 111 test binaries (doc-tests included), 3625 passed, 0 failed, 1 ignored; no `SKIP` line in the log; first run, nothing re-run |
| `pnpm -C admin lint`, `typecheck` | ok |
| `pnpm -C admin test` | 124 files, 743 tests passed |

The test suite ran while the end-to-end checks (a release build of the same tree, two
`serve`s and the sink) shared the machine; no test timed out. The fmt, clippy and rustdoc
gates were re-run after the docs edits of this exit.

## 3. Found during the exit

Nothing that needed a code change. Docs: CLAUDE.md (status, crate map, the doctor and
example-plugin usage lines, the release line, "Not built yet"), 01 §1 (`doctor/`, the SDK
helpers), the ROADMAP (M4-3 split into the done M4-3a items and the open themes and docs
site, "Deferred from M4-3a", the Later items of P42, P43 and P53), the README status, the
P53 ruling's pointer to 09 §8, and three over-long lines reflowed (09 §7's `release` bullet,
09 §8, two plugin README paragraphs: parked Minors of Tasks 3–5). The ROADMAP's M4-2 item
about `sums.sh` now says its default is 7.

## 4. Accepted limits

- The analytics counts are approximate: read-modify-write across nodes and at-least-once
  delivery can lose or double-count (P44, the `counter` caveat R19); a node running the
  batch whose clock is more than a day behind the serving node's drops those events (logged
  at `warn`); the most expensive measured
  batch needs 26–30 M of the default 50 M fuel (05 §12).
- The contact form has no per-message delete and no Reply-To; the inbox keeps the newest
  100 messages within 900 KiB (as few as 40 of the largest, X6).
- The viewer pages use a list of user ids, not roles (P46).
- Doctor checks as the user that runs it, not the service user (09 §8); a node starting
  during a SQLite read can give a stale finding, never a write (X3); a local media root that
  passes `access(2)` but fails a real write is reported `ok` (X4).

## 5. Not verified

| Item | Why | Covered instead by |
|---|---|---|
| A GitHub run of `release.yml`'s `plugins` job, the `sign` job over seven assets, and the CI `packaging` steps that build and check the plugins asset | never pushed or tagged (rule) | `package-plugins.sh` run here on macOS (bsdtar) and the asset installed (§1 a); Task 5's local run of the CI check |
| The packaging script under GNU tar on Linux | macOS host | the script's options are common to both (`-C`, `-czf`, `-tzvf`) |
| A real SMTP provider (TLS, `smtps://`, `?tls=required`, real authentication) for the contact form and doctor's `smtp.connect` | no provider used | a local plain-SMTP sink (§1 a, b); the mailer's TLS modes are M2a-2a's |
| Doctor's `tls.acme_directory` success path (an ACME directory that answers) | not run; Pebble was not started | `tests/doctor.rs` covers TLS off, `--offline` and the `tls.dir` modes; the ACME client itself is M4-2's (Pebble) |
| `release.latest` reporting a newer stable release from GitHub | no release is published | `tests/doctor.rs` against a local fake API (a newer release `warn`; a prerelease and an unreachable API `info`) |
| Doctor's S3 checks (`media.store` probe, `backup.target` on `s3://`) against a real provider | no S3 here | `tests/doctor.rs` against `s3s-fs` (Task 7) |
| Doctor in cluster mode with several nodes and a real clock skew | one node per site here | `tests/doctor.rs` on Postgres (a stale registry row aged by `UPDATE` is ignored; `clock.db_skew` against the server's `now()`); the skew thresholds unit-tested (`judge_skew`) |
| Doctor run as another user than `serve`, as root, inside the Docker image | not run | 09 §8 says to run it as the service user; the root case is skipped in the tests |
| The plugins on a cluster (two nodes counting into one day; the contact-form rate limit across nodes) | solo site only | the plugin tests run on the solo (SQLite) and cluster (Postgres) profiles in `ferrite-http/tests/plugins/`, one node each |
| A real browser posting the sandboxed form (`Origin: null`) | curl only | the route test with `Origin: null` (Task 4) and the `route-flood` hostile plugin |
| The admin E2E (Playwright) after M4-3a | no admin change in M4-3a | the admin unit tests, lint and typecheck (§2) |
| Byte-for-byte reproducibility of the asset | not a goal (P47); the tar is not normalised | — |

## 6. For the user

Nothing new in the release gate beyond M4-2's ([`m4-2-exit.md`](m4-2-exit.md) §6: license,
release key, the `release` environment and tag ruleset, GHCR visibility), except that the
plugins asset `ferrite-plugins-<version>.tar.gz` is now the seventh asset, listed in and
signed with `SHA256SUMS`. A release bumps the `version` of the three
`plugins/examples/{seo,analytics,contact-form}/ferrite-plugin.toml` together with the
workspace version (`package-plugins.sh` and the CI check refuse otherwise).

## 7. Open follow-ups

In [ROADMAP](../ROADMAP.md), M4-4, "Deferred from M4-3a" (the unverified release and SMTP
items, the parked Minors of Tasks 1–7), and "Later" (the plugin world gaps, the SEO and
doctor items left out). Next: M4-3b (the template helpers for a docs theme, the `docs` and
`business` themes, Markdown import, the docs site).

## 8. Verified claims

| Claim (where) | Source | Result |
|---|---|---|
| The asset holds `seo/`, `analytics/`, `contact-form/`, each a valid `--dir` (READMEs, 05 §12, 09 §7, CLAUDE.md) | `tar -tzvf` and `plugin check`/`install` from the extracted folders (§1 a) | verified |
| `plugin install` needs `--yes` when stdin is not a terminal (READMEs, CLAUDE.md) | `ferrite plugin install --help` | verified |
| Doctor's exit codes 0/2/1/3 (CLAUDE.md, 09 §8) | `crates/ferrite/src/doctor/mod.rs` `Report::exit_code` and its test; runs with exit 0, 1, 2 (§1 b); 3 not run here (`tests/doctor.rs`) | verified (3 by test only) |
| Doctor has 25 checks (ROADMAP, CLAUDE.md) | P53's list; 25 lines per run (§1 b) | verified |
| `sums.sh` expects 7 assets by default (ROADMAP) | `.github/scripts/sums.sh` header | verified (code) |
| The plugin READMEs name the world gaps listed under Later: no modified date, no role, no `kv.incr`, no Reply-To, no admin pages, no IndexNow/robots/sitemap changes | `grep` of the three READMEs | verified |
| The host answers a failed `kv.get` as `None` and a failed `kv.list` as an empty list (ROADMAP Later) | analytics README "Failed reads"; Task 3 re-review 2 (`ferrite-http/src/plugins/services.rs`) | verified there, not re-read |
