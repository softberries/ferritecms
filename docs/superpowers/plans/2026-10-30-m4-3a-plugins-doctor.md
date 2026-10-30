# M4-3a — Example Plugins and `ferrite doctor` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FerriteCMS ships three real example plugins:
- SEO;
- privacy-friendly analytics;
- a contact form.

They are installable from a release asset without a Rust toolchain. The milestone also adds `ferrite doctor`, which checks a site's configuration, files, database, media store, clock, versions and outside services without changing anything.

**Architecture:**
- **Plugins:** the plugins are guest crates in the `plugins/` workspace, built for `wasm32-wasip2` against the existing world `ferrite:plugin/plugin@0.1.0`. No WIT change. They get guest-side helpers added to `ferrite-sdk`.
- **Plugin tests:** host-side, in `crates/ferrite-http/tests/plugins/` and `crates/ferrite/tests/`, through the existing harnesses.
- **Doctor:** `ferrite doctor` is a new CLI command in the `ferrite` crate: a list of checks, each returning `ok`, `info`, `warn`, `error` or `skip`. It reuses the existing open, config, media, migration, node-registry, upgrade and release code in read-only ways.

**Tech Stack:**
- Rust: tokio, sqlx 0.9, object_store, lettre, reqwest, clap.
- wit-bindgen in the guest SDK.
- `serde_json` in the plugin crates. It is already in `plugins/Cargo.lock` transitively.

**Spec:**
- **ROADMAP M4-3:** the example plugins, starter themes, docs site and `ferrite doctor`.
- **Design docs:**
  - 05-plugin-host.md: §2 manifest and permissions, §4 hooks, §4a `request.served`, §4b routes, §7a volatility, §10 SDK, §11 versioning, §12 examples;
  - 09-operations.md §8 (doctor);
  - 01-architecture.md §1 (repo layout);
  - 06-themes-rendering.md §4 (`plugin_route`).
- **Rulings:** m2a2 R7, R14, R19, R20 and R22 (routes, the sandbox CSP, KV volatility, `meta.set` refused in render, data bound to the name); m4 P25 and P37 (release assets, sums).
- **Recon (git-ignored, every fact with file:line):**
  - `.superpowers/m4-3-recon/plugins.md`
  - `.superpowers/m4-3-recon/doctor.md`
  - `.superpowers/m4-3-recon/themes.md` (for M4-3b)

  Each ends with gaps or open questions; the rulings below settle them.

## Plan rulings (written to docs/superpowers/rulings/m4.md, section "M4-3a plan rulings", at Task 1)

Format: `- Plan ruling Pn: decision — reason — cost if wrong: …`. Numbering continues from M4-2 (P25–P40).

**P41: M4-3 is split into two parts.**
- **M4-3a (this plan):** the three example plugins, their release asset, and `ferrite doctor`.
- **M4-3b (next plan):** the template helpers a docs theme needs (page tree, previous/next, heading anchors, TOC), the `docs` and `business` themes, Markdown import, and the docs site.

Reasons: the plugins and doctor need no core rendering changes, and the docs theme needs several. Cost if wrong: two exits instead of one.

**P42: the plugins stay inside world `0.1.0`.**
- No WIT or host change.
- `ferrite-sdk` gets guest-side helpers only. That is not a world change; the SDK is source, not an ABI.
- Gaps the world cannot close are named in each plugin's README and added to ROADMAP "Later": no modified date on `post`, no user role in routes, no `kv.incr`, no Reply-To in mail, no admin pages for plugins.

Reason: 05 §11 says changing a record is a new major world. M4-3 is about examples. Cost if wrong: the examples look thinner than WordPress equivalents, and the README says so.

**P43: the SEO plugin (`plugins/examples/seo`, the `render.head` filter).**
- Capabilities: `render.head`, `content.read`, `options.read = ["site.title", "site.url"]`.
- Core already emits:
  - title, description and canonical;
  - robots, on item pages;
  - `og:title`, `og:description`, `og:url`, `og:type`, `og:image`;
  - sitemaps and `robots.txt`.

  The plugin adds only what core does not, and never a second copy of a tag core emitted. It checks the incoming head first.
- It adds:
  - `og:site_name` (site title) and `og:locale` (request locale, `-` → `_`);
  - Twitter cards: `twitter:card` is `summary_large_image` when an `og:image` is present, else `summary`. `twitter:title` and `twitter:description` copy core's OG values, and `twitter:site` comes from a setting.
  - On a single item (`request-info.post`, read with `content::get_post`): `article:published_time`, `article:author` (display name), `article:section` (the first `category` term) and one `article:tag` per `tag` term (core's tag taxonomy). These are added only for kind `post`.
  - JSON-LD:
    - `WebSite` with a `SearchAction` (`<site.url>/?s={search_term_string}`) on path `/`;
    - `BlogPosting` on a `post` page: `headline`, `datePublished`, `url`, `author.name`, `image` (from `og:image`) and `publisher` (an `Organization`). The organization's name comes from the `organization_name` setting, else the site title, and its `logo` from the `organization_logo` setting (an `https` URL).
  - Verification metas from the `google_verification` and `bing_verification` settings.
  - The RSS discovery link `<link rel="alternate" type="application/rss+xml" href="<site.url>/feed/">`, unless the setting `feed_link = false`.
- No IndexNow, no robots changes and no content analysis. They are listed under Later.

Cost if wrong: SEO users find it thin; it is an example.

**P44: the analytics plugin (`plugins/examples/analytics`).**
- Capabilities: the `request.served` hook, `kv`, `routes`, `options.read = ["site.url"]`.
- **Counting:** a page view is an event with `ua_class = "browser"` and status 200 or 304 (as M4-2 R8). Bots and `other` are counted separately, and other statuses are counted by class.
- **Storage:** one JSON blob per UTC day under KV key `day:YYYY-MM-DD`:

  ```
  {"v":1,"views":n,"bots":n,"other":n,"status":{"2xx":n,…},"paths":{path:n},"referrers":{host:n}}
  ```

  - `paths` is capped at 500 and `referrers` at 200. Above the cap, new keys go to `"(other)"`.
  - A referrer equal to the site's own host is not counted.
  - Paths under the `exclude_paths` setting (comma-separated prefixes) are skipped.
- **Retention:** the `retention_days` setting (default 90, valid 1..=366). After each batch the plugin lists `day:` keys (ascending, so oldest first) and deletes those older than the cutoff.
- **Stats page:** `GET /_p/analytics/` with `?days=1|7|30|90`. It is HTML, with an inline SVG bar chart and no script (the route CSP forbids scripts). It shows totals, daily views, the top 20 paths and the top 20 referrers. Every value is HTML-escaped.
- **Caveat, documented:** read-modify-write across nodes and at-least-once delivery can lose or double-count events (the `counter` example's caveat, R19).

Cost if wrong: counts drift under concurrency. `kv.incr` is already under Later.

**P45: the contact form plugin (`plugins/examples/contact-form`).**
- Capabilities: `routes`, `kv`, `mail = "admins"`, `options.read = ["site.title"]`.
- **Routes:**
  - `GET /`: a standalone form page.
  - `POST /submit`: `cross_origin = true` and `rate_limit = "10/h"`.
  - `GET /inbox`: viewer-guarded (P46).

  Themes post their own `<form>` to `plugin_route("contact-form", "/submit")`.
- `cross_origin` is needed because the plugin's own page is sandboxed (`Origin: null`, R14). It costs nothing here: the route needs no user, and anonymous cross-site posting is no worse than the spam a contact form already gets.
- **Fields:** `application/x-www-form-urlencoded` only (415 otherwise).
  - `name`: 1..=100 chars;
  - `email`: 3..=254 chars, one `@`, no whitespace or control chars;
  - `subject`: optional, up to 150 chars;
  - `message`: 10..=5000 chars;
  - `website`: the honeypot, which must be empty;
  - `return`: optional, a root-relative path checked like a 3xx `Location`.

  Every field has CR and LF stripped, except `message`.
- **Responses:**
  - **Success:** a 200 HTML thank-you page with the `success_message` setting and a link back to `return` (else `/`).
  - **Validation errors:** a 422 HTML page listing them, plus a back link.
  - **Honeypot filled:** the same thank-you page, but nothing is stored or sent.
- **Mail:** `mail::notify_admins` with subject `<subject_prefix setting, default "[Contact]"> <subject or the first 60 chars of the message>`. The subject is a single line of at most 200 chars. The body holds name, e-mail, page, time (UTC) and message.

  A mail error is logged and the message is still stored, so the visitor still sees thank-you.
- **Storage:** when `store_messages` is true (the default), the message goes into one KV blob `inbox` holding the latest 100 messages, newest first. `GET /inbox` lists them. There is no delete: a delete route would need a logged-in POST from a sandboxed page, which `Origin: null` prevents.

Cost if wrong: no per-message deletion. That is documented.

**P46: the viewer guard for plugin admin pages.**
- Routes get only `user: option<id>` (05 §4b). So `GET /_p/analytics/` and `GET /_p/contact-form/inbox` require that `req.user` appears in the plugin's `viewers` setting: user ids separated by commas or whitespace.
- An anonymous request gets 403 with "Sign in to the admin first".
- A signed-in user who is not listed gets 403 showing their own user id and how to add it to Viewers.
- An empty `viewers` setting means nobody.
- The parsing lives in `ferrite-sdk` (`viewers_allow`).

Cost if wrong: clunky set-up. A role in `route-request` needs a new world version, so it goes under Later.

**P47: shipping the plugins.**
- `release.yml` builds the three plugins once and attaches `ferrite-plugins-<X.Y.Z>.tar.gz`, made by `.github/scripts/package-plugins.sh VERSION OUTDIR`. It holds `seo/`, `analytics/` and `contact-form/`, each with `ferrite-plugin.toml`, `<name>.wasm` (renamed from `<crate_name>.wasm`) and `README.md`.
- The asset is listed in `SHA256SUMS` and signed with it. `sums.sh`'s expected count becomes 7.
- `self-update` is unaffected: it picks its asset by target name. Check this.
- The plugins are not embedded in the binary, because every plugin install needs an approval (05 §2).
- The plugin version is the workspace version at release.

Cost if wrong: one more release asset.

**P48: the `ferrite doctor` contract.**
- Usage: `ferrite doctor --data DIR [--offline]`.
- Output: one line per check, `<status>  <check-id>  <detail>`, with the status left-padded to 5 characters (`ok`, `info`, `warn`, `error`, `skip`), in a fixed check order. Then a summary line: `doctor: N ok, N warn, N error, N skip`.
- Exit codes:
  - **3** when `db.schema` reports the database incompatible (newer than this binary allows), as `migrate status` does;
  - else **1** if any check is an error;
  - else **2** if any is a warning;
  - else **0**.
- No `--json`; it goes under Later.

Cost if wrong: monitoring has to parse text.

**P49: doctor never writes.**
- **Never:**
  - creates or chmods files, folders, keys or schemas;
  - calls `keys::site_key`, `gc::install_id` or `TlsCache::prepare`;
  - opens a local media root that does not exist.
- **The one exception:** `MediaStore::check`'s probe object under `tmp/`. It is deleted at once, and the GC removes any leftover.
- **SQLite:** opened with `OpenMode::MustExist`.
- **Postgres:** before opening the store, doctor checks with its own bare connection (sqlx `PgConnection`) that the schema exists. A missing schema is an `error` and the store is not opened, because `PgStore::open` would create it.

Cost if wrong: none for the operator. It means a little extra code.

**P50: doctor while `serve` holds the SQLite lock.**
- `db.lock_state` tries `<data>/ferrite.lock` without blocking and releases it at once.
- If the lock is held, the database checks use a direct read-only SQLite connection (`SqliteConnectOptions::read_only(true)`, no lock, no migrations). Task 7 proves this with a running `serve`. If that fails, the checks report `skip` with "a node is running".
- `net.listen` probes `/healthz` and `/readyz` (and `/metrics` when `metrics_listen` is set) only when a node is running; otherwise it reports `skip`.

Cost if wrong: less coverage while running.

**P51: network checks.**
- S3 probe, SMTP connect, ACME directory and the release lookup run by default, each with a 5 s timeout. `--offline` makes them `skip`.
- The release lookup never yields `error`: a newer stable release is `warn`, and an offline or API failure is `info`.
- The database is not a "network" check. It is always required.

Cost if wrong: doctor is slow on a dead network, at most about 20 s.

**P52: clock thresholds.**
- `clock.db_skew` compares with the Postgres server's `now()`:
  - over 2 s (`MAX_CLOCK_SKEW`) is `warn`;
  - over 30 s (the TOTP ±1 step window) is `error`.
- On SQLite it is `skip`, because the database uses the local clock.
- `clock.peers` warns about live nodes whose recorded `clock_skew_ms` is over 2 s.

Cost if wrong: the threshold is too strict or too loose; it is a constant.

**P53: the doctor check list, in output order.**

| Check | What it checks |
|---|---|
| `version.binary` | The binary's version and target. |
| `config.load` | The config loads; problems are errors, warnings are warnings. |
| `fs.data_dir` | The data folder exists, is readable and writable, and has no world bits (warn on loose mode). |
| `fs.config_file` | `ferrite.toml` mode is 0600 (warn otherwise). |
| `fs.secret_key` | Solo without `auth.secret_key`: the key file exists and is valid (error), with no group or other bits (warn). |
| `fs.database_files` | SQLite: the db, wal, shm and lock files are owned by the data folder's owner and are writable. |
| `fs.backups_dir` | `<data>/backups/` is writable if it exists, and its `pre-*.db` files are counted. |
| `fs.themes_dir` | Solo: readable. Cluster: info that folders are ignored. |
| `db.lock_state` | Info: whether a node is running. |
| `db.connect` | The database opens; reports backend, latency and the Postgres version. |
| `db.schema` | Migration status, as `migrate status`. |
| `db.identity` | `site_id` present (error), `install_id` present (warn), and `last_binary_version` against the binary (warn if newer). |
| `upgrade.record` | `upgrade.json` parses (warn if not). Warns on a provisional record or a missing named snapshot. |
| `media.store` | Local: the root exists, then the probe runs. S3: the probe runs, and endpoint and bucket are reported, never keys. |
| `media.owner` | The owner marker against `install_id`. Absent is info; a mismatch is warn, with `gc::foreign`'s wording. |
| `clock.db_skew` | See P52. |
| `clock.peers` | See P52. |
| `cluster.versions` | Postgres: live nodes' version and schema version. Warns on a mixed set. |
| `cluster.site_url` | Live peers' `site_url` and mode match this config (warn). |
| `smtp.connect` | With `[mail]` set: connect, EHLO, STARTTLS and auth via lettre `test_connection`; reports host, port and TLS mode (warn on failure). Mail off is info. |
| `tls.dir` | ACME mode: `<data>/tls/` is 0700 and its files 0600 (warn). |
| `tls.acme_directory` | ACME mode: a GET of the directory URL returns JSON (warn). |
| `net.listen` | See P50. |
| `backup.target` | With `[backup]` set: the target is reachable or writable, and the newest snapshot's age is compared with the schedule (warn if older than twice the interval). |
| `release.latest` | See P51. |

- Left out, and listed under Later: an HTTP `Date` clock check, run-as-service-user detection, update-path writability, and `--json`.
- Nothing prints secrets. Database URLs, the SMTP URL, S3 keys and `secret.key` show as host or presence only.

Cost if wrong: a missed check can be added later.

## Global Constraints

- **Toolchain:** Rust stable, edition 2024. `cargo fmt --all`, plus `cargo fmt --manifest-path plugins/Cargo.toml --all`, must be clean.
- **Lints:**
  - `cargo clippy --workspace --all-targets -- -D warnings` must pass;
  - `cargo clippy --manifest-path plugins/Cargo.toml --target wasm32-wasip2 -- -D warnings` must pass;
  - rustdoc with `-D warnings`;
  - no `allow(dead_code)`.
- **Errors:** `thiserror` in libraries, `anyhow` only in the `ferrite` binary crate.
- **Plugins:**
  - Every new plugin is added to `plugins/Cargo.toml` `members`.
  - `plugins/Cargo.lock` is committed. CI builds `-p admin-e2e --locked`, so the lock must stay consistent.
  - Plugin wasm is built by the existing test helper (`build_plugins()`), so any compile error breaks every plugin test: build the plugins workspace before committing.
- **WIT:** no change to `wit/` or the host (P42).
- **Migrations:** none in M4-3a.
- **Tests:** next to the code (unit) or in `tests/` (integration). Postgres cases use `FERRITE_TEST_PG_URL` and print SKIP without it. Run the full suite with it set: `postgres://ferrite@localhost:54329/ferrite_test`.
- **Seams:** `cargo build -p ferrite --features ferrite/test-seams` does not enable the seams; use `-p ferrite --features test-seams`. Tests enable them through the dev-dependency.
- **Docs:** each task updates the design docs it changes (05, 09, 01) in the same commit. Claims in docs must be checked against the code.
- **Commits:** small commits, each ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Contact form abuse.** Check:
   - CR/LF in name or subject reaching the mail subject (header injection);
   - script or HTML in any field echoed into the error, thank-you or inbox pages, which must be escaped despite the sandbox CSP;
   - a `return` such as `//evil.com`, `/\evil` or `javascript:` (refused: the back link falls back to `/`);
   - an oversized or non-UTF-8 body (422 or 400, never a trap);
   - the honeypot path storing or sending nothing.
2. **Analytics.** Check:
   - a path or referrer holding `<script>`, quotes or a very long string, shown on the stats page (escaped and truncated);
   - a batch whose events span midnight (two day blobs, within the 100-op KV budget);
   - the caps on the `paths` and `referrers` maps (the blob stays under 1 MiB);
   - the retention prune deleting only days older than the cutoff.
3. **SEO.** Check:
   - a post title holding `</script>` or quotes inside JSON-LD (it must stay valid JSON, and the host must accept it);
   - no duplicate of a tag core already emitted;
   - pages with no `request-info.post` (archives, search, 404) still getting the site-level tags only.
4. **Doctor secrets and writes.** Check that:
   - a Postgres URL with a password, an SMTP URL with credentials and S3 keys in config never appear in output;
   - running doctor against a folder leaves its files, modes and the Postgres schema list unchanged (a test compares before and after).
5. **Doctor's failure modes.** Check:
   - a missing data folder, a missing `ferrite.toml` or a broken TOML (errors, never panics);
   - a newer schema (exit 3);
   - a running `serve` (the lock-held path);
   - an unreachable SMTP or S3 endpoint with and without `--offline`.

---

### Task 1: SDK helpers and the rulings file

**Files:**
- Modify: `crates/ferrite-sdk/src/lib.rs`. Split it into modules if it grows past about 400 lines; keep the existing public API.
- Modify: `docs/superpowers/rulings/m4.md`. Add the section "M4-3a plan rulings" with P41–P53, copied from this plan, one bullet each.
- Modify: `docs/design/05-plugin-host.md` §10, to describe the new SDK helpers.

**Interfaces (Produces), all guest-side and wasm-safe:**
- `ferrite_sdk::form::parse_urlencoded(body: &[u8]) -> Result<Vec<(String, String)>, FormError>`:
  - `+` becomes a space, and `%XX` is decoded;
  - invalid UTF-8 or a bad escape is an error;
  - at most 100 pairs.
- `ferrite_sdk::html::escape(s: &str) -> String`: escapes `& < > " '`.
- `Head` builder additions:
  - `og(...)` exists already;
  - `json_ld(json: &str)` pushes a `script-tag { src: None, json_ld: Some(..) }`;
  - `link_typed(rel, href, kind: &str)`;
  - `has_meta_name(&self, name) -> bool`, `has_meta_property(&self, prop) -> bool` and `meta_property(&self, prop) -> Option<&str>`.
- `ferrite_sdk::options::get(key) -> Option<String>`: the JSON-decoded string, or None for a non-string.
- `ferrite_sdk::mail::notify_admins(subject, body) -> Result<(), String>`.
- `ferrite_sdk::viewers_allow(setting: Option<&str>, user: Option<&str>) -> bool`: splits on commas or whitespace, ignores blanks and compares exactly.
- `ferrite_sdk::route::html(status, body) -> RouteResponse` and `route::text(...)`, which set `Content-Type` with `charset=utf-8`.

**Steps:**
- [ ] Write unit tests in `crates/ferrite-sdk` for the pure helpers: `form`, `html::escape`, `viewers_allow` and the `Head` queries. The SDK is a member of the root workspace, so `cargo test -p ferrite-sdk` runs them natively. Gate wit-bound wrappers away from native tests if needed. Cover:
  - `a=1&b=x+y%21` → `[("a","1"),("b","x y!")]`;
  - `%ZZ` is an error;
  - 101 pairs is an error;
  - `viewers_allow(Some("u1, u2\nu3"), Some("u2"))` is true, and is false for `None` or `""`;
  - escape of `<a href="x">'&` gives `&lt;a href=&quot;x&quot;&gt;&#39;&amp;`.
- [ ] Implement the helpers.
- [ ] Check that `cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml` builds every existing example unchanged.
- [ ] Add the rulings file section, then commit.

### Task 2: The SEO example plugin

**Files:**
- Create: `plugins/examples/seo/{Cargo.toml, ferrite-plugin.toml, README.md, src/lib.rs}`.
- Modify: `plugins/Cargo.toml` (members), `plugins/Cargo.lock`.
- Test: `crates/ferrite-http/tests/plugins/seo.rs`, registered in that test binary's `main.rs`.

**Interfaces:**
- Consumes the Task 1 SDK.
- The manifest is as in P43:
  - publisher `ferritecms.dev`, world `ferrite:plugin/plugin@0.1.0`;
  - hook `render.head`, priority 20;
  - settings `twitter_site`, `organization_name`, `organization_logo`, `google_verification`, `bing_verification` (text) and `feed_link` (bool, default true).

**Steps:**
- [ ] Write the failing tests, using the existing `Site` harness: install `seo`, publish a post with a category, a tag and a featured image, then render it. Assert:
  - `og:site_name`, `twitter:card=summary_large_image`, `article:published_time`, `article:section` and `article:tag` are present;
  - there is exactly one `og:title`;
  - JSON-LD `BlogPosting` parses as JSON and has `headline`;
  - a title holding `</script>"` keeps the JSON-LD valid and contains no raw `</script`;
  - `/` has the `WebSite` JSON-LD with `SearchAction`;
  - an archive or 404 page has `og:site_name` but no `article:*`;
  - setting `feed_link=false` removes the RSS link;
  - settings for verification produce `google-site-verification` / `msvalidate.01` metas.
- [ ] Implement it, with `serde_json` for JSON-LD.
- [ ] Write the README: install, permissions, settings, what core already does, gaps (P42), and the uninstall/data notes.
- [ ] Run the test binary and the plugin clippy, then commit.

### Task 3: The analytics example plugin

**Files:**
- Create: `plugins/examples/analytics/{Cargo.toml, ferrite-plugin.toml, README.md, src/lib.rs}`. Use a separate `src/store.rs` for the blob aggregation if it helps.
- Modify: `plugins/Cargo.toml`, `plugins/Cargo.lock`.
- Test: `crates/ferrite-http/tests/plugins/analytics.rs`. Add an end-to-end case in `crates/ferrite/tests/node/` if the `request.served` flush path is only reachable there; follow `node/served.rs`.

**Interfaces:**
- P44 and P46.
- Manifest:
  - hook `request.served`, priority 10;
  - `kv = true`, `routes = true`;
  - route `"/" = { methods = ["GET"] }`;
  - settings `viewers` (textarea), `retention_days` (number, default 90), `exclude_paths` (text, default `"/admin,/_p"`).

**Steps:**
- [ ] Write the failing tests:
  - feed the plugin's `on-action` a batch, through the harness's action path, with events for two days, a bot, a 404, a self-referral and an excluded path, then read the KV blobs. Assert their counts.
  - With a path holding `<script>`, the stats page as an allowed viewer shows it escaped.
  - The stats page returns 403 when anonymous, and 403 with the user's own id when signed in but not listed.
  - `?days=7` sums the right blobs.
  - Retention 1 deletes older day keys.
  - The `paths` map caps at 500, with `(other)`.
- [ ] Implement it. Keep a batch within 100 KV ops: read and write each touched day once, then one list and up to the remaining budget of deletes.
- [ ] Write the README, including the counting caveat from R19, then commit.

### Task 4: The contact form example plugin

**Files:**
- Create: `plugins/examples/contact-form/{Cargo.toml, ferrite-plugin.toml, README.md, src/lib.rs}`.
- Modify: `plugins/Cargo.toml`, `plugins/Cargo.lock`.
- Test: `crates/ferrite-http/tests/plugins/contact_form.rs`. Use the existing mail test sink from `tests/plugins/mail.rs`.

**Interfaces:**
- P45 and P46.
- Manifest:
  - `routes = true`, `kv = true`, `mail = "admins"`, `options.read = ["site.title"]`;
  - routes `"/" = {GET}`, `"/submit" = { methods = ["POST"], cross_origin = true, rate_limit = "10/h" }` and `"/inbox" = {GET}`;
  - settings `viewers`, `subject_prefix` (default `"[Contact]"`), `success_message` (textarea), `store_messages` (bool, default true).
- The README includes a copy-paste HTML form for themes that posts to `{{ plugin_route("contact-form", "/submit") }}` with a hidden `return` of `{{ request.path }}`.

**Steps:**
- [ ] Write the failing tests:
  - a valid post queues one admin mail, whose subject has the prefix and no CR/LF, and stores the message in `inbox`;
  - CR/LF in `subject` and `name` is stripped;
  - a filled honeypot gives the thank-you page, with no mail and nothing stored;
  - a missing message gives 422, listing the field;
  - `<script>` in `name` comes back escaped on the 422 page and on the inbox page;
  - a `return` of `//evil.com` makes the back link `/`;
  - `text/plain` gives 415;
  - the 11th post in an hour gives 429 (host);
  - the inbox keeps 100 messages, newest first;
  - a mail failure (mail not configured) still gives the thank-you page and stores the message;
  - the GET `/` form posts to `/_p/contact-form/submit`.
- [ ] Implement, write the README, then commit.

### Task 5: Ship the plugins as a release asset

**Files:**
- Create: `.github/scripts/package-plugins.sh`.
- Modify: `.github/workflows/release.yml` (one job that builds the plugins workspace members `seo`, `analytics` and `contact-form` with `--locked`, then packages them and uploads the asset with the others), `.github/scripts/sums.sh` (EXPECTED 7), `.github/workflows/ci.yml` (the `packaging` job also runs `package-plugins.sh` against a local build) and `docs/design/09-operations.md` §7 (the assets list).
- Modify: `docs/design/05-plugin-host.md` §12 (the examples as built) and `docs/design/01-architecture.md` §1 (the repo layout line for `plugins/examples/`).

**Interfaces:**
- `package-plugins.sh VERSION OUTDIR` → `OUTDIR/ferrite-plugins-VERSION.tar.gz`, as in P47. It fails if any wasm or manifest is missing.
- The manifest `version` in each plugin's `ferrite-plugin.toml` must equal VERSION. The script checks this; release.yml's version step sets the manifests or refuses on a mismatch. Choose the simpler one and document it.

**Steps:**
- [ ] Write the script.
- [ ] Run it locally: build the three plugins, package, list the tar, and confirm that `ferrite plugin check --dir <extracted>/seo` passes for each.
- [ ] Run `sums.sh` with 7 assets (6 fake target tarballs plus the plugins one).
- [ ] Check that self-update's asset selection ignores the plugins asset. Add a unit test in `crates/ferrite/src/update/release.rs` if the selection logic is testable there.
- [ ] Update the docs, then commit.

### Task 6: `ferrite doctor` — framework and file checks

**Files:**
- Create: `crates/ferrite/src/doctor/mod.rs` (the runner, status, report and exit code) and `crates/ferrite/src/doctor/fs.rs` (the file checks).
- Modify: `crates/ferrite/src/cli.rs` (the `Doctor { data, offline }` subcommand), `crates/ferrite/src/lib.rs` (`pub mod doctor`) and `crates/ferrite/src/config_cmd.rs` (move the check body into a reusable function that returns problems and warnings, so `config check` and doctor share it; `config check`'s output must not change).
- Test: `crates/ferrite/tests/doctor.rs`.

**Interfaces (Produces):**
- `pub enum Status { Ok, Info, Warn, Error, Skip }`
- `pub struct Finding { id: &'static str, status: Status, detail: String }`
- `pub struct Report { findings: Vec<Finding>, incompatible: bool }`, with `exit_code() -> u8` (P48).
- `pub async fn run(data: &Path, offline: bool) -> Report`.

Checks in this task:
- `version.binary`
- `config.load`
- `fs.*`
- `db.lock_state`
- `upgrade.record` (the file part)
- `tls.dir`
- `backup.target` (local folder: exists, writable by mode or uid math without writing, repository marker, snapshot age; S3 target: list the prefix, or `skip` when offline)

A check that needs a later task's data reports `skip` until Task 7 wires it in. Task 7 replaces that.

**Steps:**
- [ ] Write the failing tests, spawning the binary like `tests/cli.rs`:
  - a fresh `init` folder with `--offline` gives no `error` lines and exit 0 or 2. Define and assert the exact expected set of warns for a fresh site, if any.
  - `chmod 644 secret.key` gives `warn fs.secret_key` and exit 2.
  - A missing `ferrite.toml` gives `error config.load` and exit 1.
  - A broken TOML gives `error`, not a panic.
  - A missing data folder gives `error fs.data_dir` and exit 1.
  - Running doctor leaves every file's mode, size and mtime under the data folder unchanged (snapshot before and after).
- [ ] Implement it. Output format and order are as in P48 and P53.
- [ ] Document `doctor` in 09 §8, with the check table and exit codes, then commit.

### Task 7: `ferrite doctor` — database, media, cluster, clock and network checks

**Files:**
- Create: `crates/ferrite/src/doctor/{db.rs, media.rs, cluster.rs, net.rs}`.
- Modify: `crates/ferrite/src/open.rs`, adding a read-only SQLite open used only by doctor (P50).
- Modify, only as far as visibility needs: `crates/ferrite/src/healthcheck.rs` (`probe` made `pub(crate)`), `crates/ferrite/src/update/release.rs` and `update/mod.rs` (expose a read-only "latest stable release" lookup that accepts the same hidden test base URL `self-update` uses), and `ferrite-http` mail (an SMTP connection test).
- Test: `crates/ferrite/tests/doctor.rs`, plus Postgres cases.

**Interfaces:**
- Consumes Task 6's `Finding`, `Status` and `Report`.
- Checks in this task:
  - `db.connect`, `db.schema`, `db.identity`;
  - `media.store`, `media.owner`;
  - `clock.db_skew`, `clock.peers`;
  - `cluster.versions`, `cluster.site_url`;
  - `smtp.connect`;
  - `tls.acme_directory`;
  - `net.listen`;
  - `release.latest`.

**Steps:**
- [ ] Write the failing tests:
  - A SQLite site with `serve` running (use the serve test harness) gives `info db.lock_state` naming a running node, `ok` from the database checks via the read-only path (or the documented `skip`), and `ok net.listen`.
  - A schema newer than the binary gives exit 3. Use the existing way migration tests fake a newer applied migration.
  - Postgres:
    - a missing schema gives `error db.connect`, and the schema still does not exist afterwards;
    - an existing site gives `ok db.schema` and a `clock.db_skew` line;
    - two node rows with different versions give `warn cluster.versions`;
    - the config URL holds a password, and the output never contains it.
  - S3 via the existing s3s-fs test server: `ok media.store`. A wrong bucket gives `error`. `--offline` gives `skip`.
  - An SMTP URL with a user and password pointing at a closed port gives `warn smtp.connect`, without the password; mail off gives `info`.
  - `release.latest` with a test base URL serving a newer stable release gives `warn`, and an unreachable one gives `info`.
  - A media owner marker naming another install gives `warn media.owner`.
- [ ] Implement it, then commit.

### Task 8: Docs and the M4-3a exit

**Files:**
- Modify:
  - `docs/ROADMAP.md`: tick the M4-3 plugin and doctor items; add "Deferred from M4-3a" and the Later items from P42–P53;
  - `CLAUDE.md`: status, the crate map (`doctor`), usage lines for doctor and installing the example plugins, and "Not built yet";
  - `docs/design/09-operations.md` §8;
  - `docs/design/05-plugin-host.md` §12;
  - `README.md`, if it lists commands.
- Create: `docs/superpowers/m4-3a-exit.md`, with a Verified table (each row naming its evidence) and a Not-verified table.
- Modify: `docs/superpowers/rulings/m4.md`, adding the "M4-3a execution rulings" from the ledger.

**End-to-end checks, recorded in the exit:**
- a) Build a release binary and the plugins asset with `package-plugins.sh`. Then:
  1. `init` and `seed demo` a site;
  2. `ferrite plugin install` each plugin from the extracted asset, approving its fingerprint;
  3. `serve`;
  4. curl a post page, checking the SEO tags and that the JSON-LD parses;
  5. browse 20 pages and wait for a flush, then open the analytics stats page as a listed viewer, using a session cookie from the API login;
  6. post the contact form with `Origin`. Use a local SMTP sink, the one the repo's tests use or any local sink, and check the mail and the inbox page.
- b) `ferrite doctor`:
  - on that site while running;
  - after stopping it;
  - with `--offline`;
  - against a Postgres site;
  - against a deliberately broken folder.

  Paste the output with credentials checked absent.
- c) Full gates:
  - fmt (both workspaces);
  - clippy (both workspaces);
  - rustdoc;
  - `cargo test --workspace` with `FERRITE_TEST_PG_URL` (0 failed, 0 SKIP);
  - the admin gates (lint, typecheck, test), which only confirm nothing broke.

**Steps:**
- [ ] Run a–c, then write the exit and docs, then commit.

## Task dependencies

- Task 1 comes before Tasks 2–4. Tasks 2–4 run one after another, because they share `plugins/Cargo.toml` and the lock.
- Task 5 comes after Tasks 2–4.
- Task 6 comes before Task 7.
- Tasks 6 and 7 are independent of Tasks 1–5 and may run after Task 1 in any order.
- Task 8 comes last.
