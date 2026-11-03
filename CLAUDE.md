# CLAUDE.md — FerriteCMS

## What this project is

FerriteCMS: a WordPress-style CMS as a single Rust binary, with every plugin running in a
capability-scoped WebAssembly sandbox (wasmtime + Component Model). It imports WordPress
content (WXR) but deliberately does **not** run WordPress PHP plugins or themes.

Read before doing anything substantial:

- `docs/BRIEF.md` — product + technical design (source of truth for intent)
- `docs/design/` — detailed technical design (00-overview first; wins over BRIEF on the *how*)
- `docs/ROADMAP.md` — milestones and checklists; work top-down

## Current status

M0 (plugin host spike), M1a (storage), M1b (media), M1c-1 (config, CLI `init`/`migrate`,
core services), M1c-2 (router, themes, page cache, plugin wiring, `serve`, `seed demo`),
M1.5 (cluster mode: node registry, LISTEN/NOTIFY event bus, job queue, locks, themes as
artifacts, hot swap, shared plugin failure counters, compose example) and M2a-1 (REST API
core under `/api/v1` with OpenAPI, auth: argon2id, DB sessions + CSRF, scoped API tokens,
DB rate limits, TOTP, site key; users + `ferrite user`; content/types/taxonomies/menus/
redirects/settings endpoints; previews; logged-in requests; `sessions.prune`) and M2a-2a
(extensions and the plugin platform: themes/plugins API + `ferrite theme|plugin` with an
approval fingerprint per install, delete with 30-day data retention, plugin routes under
`/_p/`, theme and plugin settings with sealed secrets, `[mail]` + `notify-admins` +
password reset by e-mail, `plugin.action` jobs, batched `request.served`, the per-node call
cap and per-render budget) and M2a-2b (media uploads typed from their content, documents
served download-only, `Idempotency-Key`, the `media.variants` job, SVG sanitising; comments
with moderation and the theme form; the system API: status, nodes, audit, jobs with retry,
metrics; the real plugin `http` import with resolved-address SSRF checks; the URL
allowlist, the `__Host-` session cookie, zeroized keys; term archives for custom types; the
`test-seams` feature) are done, so M2a (the M2 backend) is complete; the exits are written
up in `docs/superpowers/m1-exit.md`, `m15-exit.md`, `m2a1-exit.md`, `m2a2a-exit.md` and
`m2a2b-exit.md` (each lists what was not verified). M2b-1 (the admin SPA's foundation
and writing: embedded `/admin/` with its CSP, design system (11-admin-ui), sign-in/TOTP/
reset screens, shell, `⌘K` palette, dashboard, content list, TipTap editor with publishing,
autosave slot, 412 conflict UI and revisions; API additions `allowed`, `author`, counts,
`0007_autosaves`) is done too (`m2b1-exit.md`), and so is M2b-2 (media and content
modelling: media library with drag-and-drop uploads, media picker, featured and inline
images, custom fields from the type schema, SEO fields, content types with the field
builder, taxonomy term trees, menus with `@dnd-kit`, the comment moderation queue,
redirects with CSV; public comment pages (`?cpage=N`); API additions `seo`, media
`q`/`kind`/`trash`/restore, comment `item` and counts, menu `locations`,
`GET /content/refs`, `0008_autosave_seo`; `m2b2-exit.md`), and so is M2b-3
(administration and extensions: account with TOTP enrolment, tokens and sessions; users;
settings; themes with in-browser bundle packing; plugins with the permission report,
write-only secrets and metrics; system with status, nodes, jobs and the audit log; every E2E
spec in Chromium on both backends, Firefox and WebKit; the bundle budget; API additions
user filters, sessions, admin TOTP reset, retained plugin data, audit filters, the test mail,
relation `types`, inline image `srcset`; `ferrite theme pack`; no migration;
`m2b3-exit.md`, verified in two parts, the browser and Docker checks in part B), so M2 is
done (`m2-exit.md`). Open follow-ups stay listed in the roadmap. M3-1 (the WordPress
import core: `ferrite import wordpress`, the streaming WXR parser and its IR, `import_map`
re-runs with the run fence, Gutenberg/classic/shortcode conversion, media from `--uploads`
or `--fetch-media`, users, terms, comments, menus, options, SEO and ACF, redirects for old
permalinks, the JSON/HTML report; migration `0009_import`; `m3-1-exit.md`) is done, and so
is M3-2 (the `import.wordpress` job with step progress and a partial report for a failed
run, the `/import` API with the inputs as `import` artifacts and the report stored as
artifacts, the admin Import screen (`g i`), uploads read from a zip in place, the SEO
canonical and robots fields, `permalink.redirects` for permalink-structure changes, the
`UNION ALL` job claim on Postgres; migration `0010_import_jobs`), so M3 is done
(`m3-exit.md`, which lists what was not verified). M4-1 (data safety) is done too
(`m4-1-exit.md`): `export` / `import-site` / `db copy` over a table catalogue, `backup` /
`restore` and the scheduled `[backup]` (`site.backup` job), `media sync|gc|regenerate` and the
daily `artifacts.gc` job, the S3 `copy_if_not_exists` / `conditional_put` options,
`media.strip_metadata` and `media.gc_grace_days`, the Postgres `lock_wait`, an atomic
`seed demo`, the cluster example on RustFS; migration `0011_artifact_gc`. M4-2 (release) is
done too (`m4-2-exit.md`): signed `self-update` and `rollback` (`--schema`,
`--restore-snapshot`) with `upgrade.json`, built-in ACME TLS for solo sites, the metrics
registry with `/metrics` on `server.metrics_listen`, per-minute `node_metrics` rows, the
series API and the dashboard's Site traffic, `site.timezone` in the `date` filter (jiff),
a ring-only build, the release workflow (six targets, minisign, GHCR), the systemd unit,
the `compat` CI job and the runbooks in `docs/operations/`; migration `0012_node_metrics`.
The first release, `v0.1.0`, was tagged on 2026-11-01 after the release gate in the exit. M4-3a is done
too (`m4-3a-exit.md`): the example plugins `seo`, `analytics` and `contact-form` (SDK
helpers, the host answering `options.get("site.url"|"site.title")`, percent-encoded
`request.served` paths, analytics' two storage tiers under the KV quota, X7) shipped as the
release asset `ferrite-plugins-<version>.tar.gz`, and `ferrite doctor`; no migration. M4-3b
is done too (`m4-3b-exit.md`), so M4-3 is done (`m4-3-exit.md`): heading ids and `post.toc`,
`SiteReader::tree` and the `page_tree()` helper (breadcrumbs, previous/next), `query(order=
"menu")`, the `docs` and `business` starter themes, `seed demo --profile business`, `ferrite
import markdown` (crate `ferrite-import-md`), code blocks as `<pre tabindex="0">`, the docs
site in `site/` (Markdown, `build.sh`, `VERIFIED.md`, the `docs_site` test) and axe checks
on the three starter themes (Playwright project `public-themes-chromium`); no migration.
Hosting the docs site is the user's decision. M4-4 (the follow-up lists) is done too
(`m4-4-exit.md`), so M4 is done (`m4-exit.md`): restore exits 2 when its final check fails
and refuses a media root inside the folder it replaces, restored `site.backup` rows are
settled, the SQLite open refuses an interrupted `--restore-snapshot`'s leftovers (P70,
R-c2), `self-update --data` runs the new binary's `config check`, `rollback --schema` on
Postgres refuses live nodes, `--restore-snapshot` checks the snapshot's schema, `media gc`
exits 2 for a refused store or a failed delete, `artifacts.gc` runs at 03:00 UTC; the front
page's permalink answers 301 to `/` (the sitemap, the tree and menus name it `/`), `srcset`
keeps one shape, tree card images skip the bridge budget; `ContentRepo::file_bounds` for the
sitemaps; blank `q` is no filter, unreadable relations and parents count as missing, mail
falls back to the configured title, WordPress sources must be http(s), `build_s3` keeps the
environment's client options; the Markdown import's byte and folder caps, NFC-colliding zip
entries reported missing, an expired lease no longer keeps an import run live; the admin's
44 px touch icons, the popup edge, stale states at full contrast (P81, R-c3), SEO counting by
code points; the test flakes fixed (SQLite `close()` now releases `ferrite.lock`);
reproducible release archives (`repro-tar.sh`); no migration. `v0.1.0` is released
(2026-11-01): licensed `AGPL-3.0-only` with a plugin and theme permission
(`LICENSE-EXCEPTION.md`; `LICENSING.md`, a commercial license from SOFTBERRIES), outside
contributions through CLA assistant (`CONTRIBUTING.md`), the docs site on GitHub Pages
(`.github/workflows/pages.yml`, `site/export-static.sh` + `site/rewrite-static.py`). What is
still open is under "Before v0.1.0" (the unchecked items) and "Later" in the roadmap. M5 (AI
assistants over MCP) is done and released in `v0.2.0` (2026-11-02; `m5-exit.md`; the current release; `v0.1.0` was the first): every site
serves an MCP endpoint at `<site.url>/mcp` (`rmcp`, Streamable HTTP, stateless) with 56 tools,
2 resources, 1 resource template and 4 prompts, a built-in OAuth 2.1 authorization server (PKCE, dynamic registration
and Client ID Metadata Documents, refresh rotation, `fera_` access tokens bound to `/mcp`), the `mcp.enabled`
switch, and the admin's consent screen, Connected apps, Settings → AI assistants and dashboard
card; migration `0013_oauth`; design 12, roadmap "M5". M6-6 (outgoing webhooks) is built
(`m6-6-exit.md`): `ferrite_core::webhooks` (catalogue, Standard Webhooks signing, sealed
rotating secrets, `WebhookService`, `emit` from the content, media, comment, user and import
writes in their `Tx`), the `webhook.deliver` (one job per attempt, 8-step schedule,
auto-disable at 5 days + 10 failures or a `410`, mail to admins) and hourly `webhooks.prune`
jobs, `ferrite_http::webhook_send::WebhookClient` over the egress SSRF rules, `[webhooks]
local_targets`, `/api/v1/webhooks` (`manage_webhooks`, scopes `webhooks:read|write`, never
OAuth), the admin's Webhooks screens (`g w`), doctor's `webhooks.endpoints`; migration
`0016_webhooks`; design 18.
M6-2 (plugin trust, design 14,
`m6-2-exit.md`) is built: minisign-signed plugins (`ferrite-plugin.minisig`,
`ferrite-plugin.pub`; the report names the verified publisher, the fingerprint covers the
signer), plugin data bound to its publisher (`plugin_owners`), the signed static registry
(`[plugins] registry|registry_key|signatures|registry_check`, off-by-default daily check per
ruling P5), publisher keys and revocation, `ferrite plugin search|install NAME|update|digest|keys|unbind`,
doctor `plugins.trust`, the admin's Registry tab and Publisher keys; migration
`0015_plugin_publishers`; the real signing keys are the user's to create (09 §7 "Plugin keys";
`crates/ferrite/registry.pub` is a placeholder). Work top-down in
`docs/ROADMAP.md`; per-milestone rulings are in `docs/superpowers/rulings/` (M2a-2a:
`m2a2.md`, M2a-2b: `m2a2b.md`, M2b: `m2b.md`, M3: `m3.md`, M4: `m4.md`, M5: `m5.md`).

Crate map (`crates/`): `ferrite` (bin: CLI, node wiring, hot-swap reloader, job handlers;
M4-1: `archive` (the `tar.zst` format), `site_copy` + `export_cmd` (export, import-site, db
copy), `backup/` (`repo`, `restore`, `target`), `gc`, `media_cmd`, `media_url`,
`jobs/{gc,backup}.rs`; M4-2: `update/` (`mod` self-update, `release`, `verify`, `swap`,
`rollback`), `upgrade` (`upgrade.json`, `last_binary_version`), `tls` (ACME), `node_metrics`
(the minute writer), `build.rs` (target triple), `release.pub` (embedded key); M4-3a:
`doctor/` (`ferrite doctor`'s checks); M6-2: `registry` (the configured registry and the
embedded `registry.pub`), `jobs/registry.rs` (`plugins.registry_check`), `doctor/plugins.rs`) -> `ferrite-http` (public site, router, head,
previews and logged-in requests, plugin wiring, plugin reloader, shared failure counters,
plugin routes, `request.served` buffer, SMTP mailer, the extension install pipeline
`extensions::Installer`, `import_fetch` (the importer's SSRF-checked `--fetch-media`
source), `import_blobs` (import reports over the media store), `metrics` (the registry,
`/metrics` text), `tls` (HSTS, the redirect app); the REST API in `src/api/` with its
OpenAPI snapshot `openapi.json`; M5: `mcp/` (the MCP endpoint: `auth` guard, `server`,
`tools/` one file per area, `resources`, `prompts`, `results`), `oauth.rs` (the OAuth
endpoints and the off-switch gate), `oauth_fetch.rs` (the SSRF-checked Client ID Metadata
Document fetch), `api/oauth.rs` (the consent API); M6-2: `registry_fetch` (the SSRF-checked
registry fetcher, file registries), `extensions/{trust,registry}.rs`, `api/plugin_registry.rs`) -> `ferrite-import-wp` (the WordPress importer: WXR
parser, IR, conversion, passes, report, `ZipSource`; -> `ferrite-core`, `ferrite-media`),
`ferrite-import-md` (M4-3b: the Markdown importer: folder scan, front matter, Markdown to
blocks through `ferrite-import-wp`'s HTML converter (M5: `markdown_to_body`, used by the MCP
content tools), `run` over `ImportService`; ->
`ferrite-core`, `ferrite-import-wp`, `ferrite-media`, `ferrite-domain`),
`ferrite-cache` (page cache + outbox invalidation
loop with LISTEN wake-ups), `ferrite-theme` (themes, bundles, hierarchy, MiniJinja
renderer; M4-3b: `page_tree`), `ferrite-core` (services, permalinks, `SiteReader` (M4-3b:
`site/tree`, `ListOrder`), blocks (M4-3b: heading ids, `TocEntry`), `ExtensionService` +
`ExtensionAdmin`, `ext_settings`, `mail` (`Mailer`, `MailQueue`), action jobs;
`AuthService` (incl. password reset; M5: grant-bound tokens, the MCP call limit), M5: `oauth/`
(`OAuthService`: clients, requests, codes, grants, PKCE, redirect rules, metadata) and
`blocks/markdown.rs` (blocks to Markdown), `UserService`, content types, settings allowlist,
roles + token scopes, `import` (`ImportService`/`ImportBatch`: the import's write path; the
import jobs, run reads and report artifacts), `seed` (`DemoBatch`: `seed demo` in one
transaction)), `ferrite-media` (`strip`: EXIF/XMP removal; store listing, copy and the owner
marker), `ferrite-plugin-host` (grants, permission report;
-> `ferrite-domain`);
`ferrite` also -> `ferrite-import-wp` (`import_cmd`, the `import.wordpress` job in
`jobs/import.rs`), `ferrite-import-md` (`import_cmd`) and `ferrite-cluster` (node registry, job runner, periodic scheduler, built-in
prune jobs incl. `node_metrics.prune`) -> `ferrite-store` (traits; `catalogue`, `export`
reader and `import_writer` for the portable copies; `NodeMetricsRepo`, `node_metrics` skipped
by the catalogue); `ferrite-core` -> `ferrite-store` (traits) <-
`ferrite-store-sqlite` / `ferrite-store-postgres` (+ `ferrite-store-tests` contract suite, `CountingStore`, `window::in_one_window`, `FailingOptionsStore`);
`ferrite-domain` (types; `metrics`: counts, series buckets, page p95), `ferrite-config`,
`ferrite-sdk` (guest SDK; M4-3a: `form`, `html::escape`, `options`, `settings`, `time`,
`mail`, `viewers::allow`, `route::html`, `Head` additions; helpers used by module path).
Packaging: `Dockerfile`, `packaging/systemd/`, `.github/workflows/{ci,release}.yml` and
`.github/scripts/` (package, package-plugins, repro-tar, test-repro, sums, verify-sig, newest, compat). Starter
themes (embedded): `themes/blog`, `themes/docs` (page-tree sidebar, TOC, pager) and
`themes/business` (hero, feature cards, contact form). Plugins: `plugins/examples/` (own
workspace, built for `wasm32-wasip2`): the shipped examples `seo`, `analytics`, `contact-form` (a README
each), the rest test fixtures. Admin SPA: `admin/` (pnpm, Vite + React + TS; `pnpm build` writes
`admin/dist/`, which `ferrite-http`'s `admin` module embeds; without it the binary serves a
placeholder page; Vitest + Playwright E2E, see `admin/README.md`; the axe checks of the
public themes in the `public-themes-chromium` project). Docs site: `site/` (`content/`
Markdown with screenshots in `content/images/`, `build.sh`, `README.md`, `VERIFIED.md`;
`export-static.sh` + `rewrite-static.py` publish it on GitHub Pages with the brand layer
`brand/` and a Pagefind index). Logo files: `docs/brand/`. Not built yet: `ferrite plugin new`
(later, 09 §8) (`docs/design/01-architecture.md` §1; API status
per endpoint in 07 §1, admin status per screen in 07 §3).

Run a site locally: build the admin first (`pnpm -C admin install && pnpm -C admin build`;
needs Node >= 24.18 and pnpm 10.28.2; skip it for the placeholder page), then
`cargo run -p ferrite -- init --data /tmp/site --url http://localhost:8080` (for a solo site
it also creates the site key `<data>/secret.key`), then optionally
`cargo run -p ferrite -- seed demo --data /tmp/site` (sample content; `--profile business`
writes a small-business site with a static front page for the `business` theme, then
`theme activate business --data /tmp/site`), then an account:
`cargo run -p ferrite -- user create --data /tmp/site --login me --email me@example.com`
(prints a one-time password that must be changed at the first login; `--password-stdin`
reads one from stdin instead; seed's own `admin` has no password until
`user reset-password --data /tmp/site --login admin`), then
`cargo run -p ferrite -- serve --data /tmp/site` (prints `Listening on http://…`; stop with
Ctrl-C/SIGTERM). The admin is at `http://localhost:8080/admin/`; for admin work run
`pnpm -C admin dev` (Vite on `:5173`, `http://localhost:5173/admin/`, proxying `/api` and
`/_preview` to `FERRITE_DEV_URL`, default `http://localhost:8080`). The API is at `http://localhost:8080/api/v1` (spec at
`/api/v1/openapi.json`): log in with `POST /api/v1/auth/login` and an `Origin:
http://localhost:8080` header, then send the cookie plus `X-CSRF-Token` on writes, or use a
bearer token from `POST /api/v1/users/{id}/tokens`. AI assistants (M5): the MCP endpoint is
`http://localhost:8080/mcp`; connect Claude Code with `claude mcp add --transport http ferrite
http://localhost:8080/mcp`, then `/mcp`, choose `ferrite` and follow the browser sign-in (consent at
`/admin/connect`), or add `--header "Authorization: Bearer fer_…"` with a personal token; Cursor
and VS Code take the URL in `mcp.json` (guide `site/content/02-guides/08-ai-assistants.md`);
Claude.ai/Desktop connectors and ChatGPT connect from the vendor's cloud and need the site on
public HTTPS. Probe it with curl: `POST /mcp` with `Authorization: Bearer …`, `Content-Type:
application/json`, `Accept: application/json, text/event-stream` and a JSON-RPC body
(`tools/list`, `tools/call`); a missing token answers `401` with the `resource_metadata`
challenge. Settings → AI assistants holds the `mcp.enabled` switch (off: `404`).
Webhooks (M6-6): admin → Webhooks (`/admin/webhooks`) or `POST /api/v1/webhooks` with `{name,
url, events: ["content.*"]}` (the `201` shows the `whsec_` secret once); deliveries need
`serve` running (the job runner sends them). To try them against a local receiver, start
`serve` with `FERRITE_WEBHOOKS__LOCAL_TARGETS=true` (allows `http` and loopback; otherwise
only public `https` targets) and point a webhook at e.g. `http://127.0.0.1:9000/`; guide
`site/content/02-guides/09-webhooks.md`. `cargo run -p ferrite -- migrate status
--data /tmp/site` exits 0 ok, 2 pending, 3 incompatible, 1 error; with SQLite it (and `seed`
and `user`) fails while `serve` holds the data-folder lock. `FERRITE_LOG=debug` sets log
verbosity; `FERRITE_<SECTION>__<KEY>` overrides config (e.g.
`FERRITE_SERVER__LISTEN=127.0.0.1:0` for a free port).
`cargo run -p ferrite -- config check --data /tmp/site` validates config + env without
touching the database (exit 0 prints `ok: mode=… database=… media=… workers=… mail=… metrics=… tls=… backup=… webhooks=…`, exit 1 lists
problems).
Themes and plugins: `ferrite theme install|activate|list` and `ferrite plugin
check|install|list|enable|disable|remove` (`--dir` holds `ferrite-plugin.toml` and one
`*.wasm`; with SQLite `serve` must be stopped), or the API (`/api/v1/themes` takes the bundle
`ferrite theme pack --dir DIR --out FILE` writes, without a data folder;
`/api/v1/plugins`: inspect, then install with the report's `fingerprint` as `approve`), or
the admin's Appearance and Plugins screens. The example plugins: unpack the release asset
`ferrite-plugins-<version>.tar.gz` (folders `seo/`, `analytics/`, `contact-form/`, each a
`--dir`), then `ferrite plugin install --data DIR --dir seo --enable [--yes]`; the stats and
inbox pages (`/_p/analytics/`, `/_p/contact-form/inbox`) need the user's id in the plugin's
`viewers` setting (each plugin's README). Mail (password reset, plugin `notify-admins`, the
admin's test mail) needs `[mail] smtp_url` + `from` (01 §4).
Signed plugins and the registry (M6-2, design 14): a folder may hold `ferrite-plugin.minisig`
(`ferrite plugin digest --dir DIR` prints the payload to sign with `minisign -S`) and
`ferrite-plugin.pub`; the API takes them as the multipart parts `signature` and `publisher_key`.
`ferrite plugin search|install NAME[@VERSION]|update [--check]` use `[plugins] registry` (+
`registry_key`; `file:///abs/index.json` works for local tests) and fetch it first;
`ferrite plugin keys list|trust|revoke`, `ferrite plugin unbind NAME` (operator only). API:
`/api/v1/plugins/registry` (+ `/refresh`, `/inspect`, `/install`), `/api/v1/plugins/publishers`
(+ `/{key_id}/revoke`).
Check a site: `ferrite doctor --data DIR [--offline]` (27 checks that write nothing locally,
S3 a probe object; `<status> <check-id> <detail>` lines, then a count; works while `serve`
runs; `--offline` skips the S3, SMTP, ACME and release lookups; 0 ok, 2 warnings, 1 errors,
3 schema incompatible; 09 §8).
Import a WordPress site (Tools → Export in WordPress gives the WXR file): `cargo run -p
ferrite -- import wordpress export.xml --data /tmp/site [--uploads <copy of
wp-content/uploads, or a .zip of it>] [--fetch-media] [--dry-run] [--report DIR] [--no-keep-permalinks]
[--no-site-options] [--category-base category] [--tag-base tag] [--as LOGIN] [--takeover]`
(with SQLite `serve` must be stopped; exit 0 done, 2 done with refused redirects, missing
or refused media, 3 another import running, 1 error, still with the partial report of a
run that had started; the report goes to
`<data>/import/reports/<run-id>/report.{json,html}`; a re-run of the same site updates
what changed in WordPress and keeps what was edited here, terms are matched, not updated; media variants are made by the
next `serve`). With `serve` running, import from the admin's Import screen
(`/admin/import`: the WXR file, optionally a zip of `wp-content/uploads`) or the API:
`POST /api/v1/import/wordpress` as `multipart/form-data` with parts `wxr`, optional
`uploads` (zip) and optional `options` (JSON: `dry_run`, `keep_permalinks`,
`site_options`, `fetch_media`, `category_base`, `tag_base`), an `Idempotency-Key` if
wanted; `202` gives the job, `Location: /api/v1/import/jobs/{job_id}` (poll it), then
`GET /api/v1/import/runs/{id}/report.json` or `report.html` (`import:write` to start,
`import:read` to read; `409` while another import is queued or running).
Import a folder of Markdown files as pages (09 §8, 08 §8): `cargo run -p ferrite -- import
markdown DIR --data /tmp/site [--as LOGIN] [--type page] [--front-page] [--dry-run]
[--source NAME] [--takeover]` (TOML front matter between `+++` lines; `index.md`/`README.md` is a
folder's page; with SQLite `serve` must be stopped; prints the counts and one
`<kind>  <file>  <detail>` line per problem; exit 0 done, 2 skipped files, missing or
refused images or broken links, 3 another import running (`--takeover` takes over one with
no heartbeat for 120 s), 1 error; a re-run of the same
`--source` (default the folder's name) updates changed files and keeps pages edited here).
Build the docs site: `site/build.sh OUT_DIR [URL]` (a fresh folder;
`FERRITE=target/release/ferrite` to use a built binary, default `cargo run -q -p ferrite
--`; it runs `init`, `user create --login docs`, `import markdown site/content --front-page
--source ferrite-docs` and `theme activate docs`, fails on an import exit 2, then prints the
`serve` command; `site/README.md`). `cargo test -p ferrite --test docs_site` builds and
link-checks it;
`pnpm -C admin e2e --project public-themes-chromium` runs axe on the three starter themes
(after `cargo build --release -p ferrite`; `admin/README.md`).
Move, copy and back up a site (09 §4 and §8; with SQLite `serve` must be stopped for each
of these except `media sync`, which opens no database): `ferrite export --data DIR --out
FILE|- [--with-media]` (0, 2 artifacts missing from the store, 3 schema newer, 1 error; never
holds `secret.key`), `ferrite import-site ARCHIVE|- --data DIR [--with-media]` (into a fresh
`init` folder; 0, 3 newer, 1), `ferrite db copy --from URL --to URL [--from-schema S]
[--to-schema S]` (`postgres://…` or `sqlite:<path>`; 0, 3, 1; copies no media), `ferrite
backup --data DIR --to FILE.tar.zst|-|DIR|s3://… [--no-media] [--no-secret-key] [--keep N]`
(SQLite only; 0, 2 artifacts missing, 3 newer repository, 1), `ferrite restore SOURCE --data
DIR [--snapshot NAME] [--force]` (0, 2 restored but the check afterwards failed, 3, 1),
`ferrite media sync --to STORE (--from STORE |
--data DIR)` (`local:PATH` or `s3://bucket[/prefix]`, `AWS_*` env; 0, 2 some failed, 1),
`ferrite media gc --data DIR [--dry-run] [--grace-days N] [--claim]` (0, 2 store refused or a
delete failed, 3 lock held, 1),
`ferrite media regenerate --data DIR [--all] [--item ID]` (0, 2 some failed, 1). A running
SQLite site is backed up by `[backup] schedule = "daily"|"weekly"`, `to` (a folder outside
the data folder, or `s3://`), `keep`, `with_media` (the `site.backup` job; on Postgres it
writes export archives). Config: `[media] strip_metadata`, `gc_grace_days` (1..=365, default
7), `[media.s3] copy_if_not_exists`, `conditional_put`. One running site per media store:
each database has an install id (`schema_meta.install_id`; `restore`, `import-site` and `db
copy` give the copy its own), and the GC deletes only from a store whose
`ferrite-owner.json` names it (`media gc --claim` after a restore in place).
Update and roll back (09 §1–§2, runbook `docs/operations/solo-upgrade.md`): `ferrite
self-update [--version X] [--check] [--yes] [--data DIR [--skip-config-check]]` (0 updated or up to date, 2 newer
release with `--check`, 1 error; clap usage errors are 2 too; a repository build has no
release key and refuses), `ferrite rollback [--data DIR] [--schema] [--allow-lossy]
[--restore-snapshot] [--yes]` (0, 3 the kept binary cannot open the schema, 1; `--schema`
runs as the record's `to`). A test-seams binary for local release tests: `cargo build -p
ferrite --features test-seams` (not `--features ferrite/test-seams`, which with `-p ferrite`
names the self dev-dependency and enables nothing), then `--base-url`, `--test-current-exe`
and `FERRITE_TEST_RELEASE_PUBKEY`. HTTPS for solo sites: `[server.tls] mode = "acme"`,
`contact = "mailto:…"`, `directory = "staging"` first (09 §5; `FERRITE_TEST_PEBBLE=1 cargo
test -p ferrite --test tls` runs Pebble in Docker). Metrics: `[server] metrics_listen =
"127.0.0.1:9464"` serves `GET /metrics`; `GET /api/v1/system/metrics/series?from&to&step=
minute|hour|day[&node]` (`system:read`). Release: `.github/workflows/release.yml` (a manual
run is a dry run with a throwaway key; a `v*` tag publishes, signed with the `release`
environment's key; the tag ruleset lets only admins create, move or delete `v*` tags; it also attaches the plugins asset from `.github/scripts/package-plugins.sh
VERSION OUT_DIR`, which refuses a manifest whose `version` is not the workspace version, so a
version bump moves the three `plugins/examples/{seo,analytics,contact-form}/ferrite-plugin.toml`
with it); systemd: `packaging/systemd/` (README there).

Run two cluster nodes locally: two data folders whose `ferrite.toml` share `site.url`,
`database.url` (Postgres) + `database.schema`, and `[media] root = "<shared dir>"` with
`local_shared = true`, `cluster.mode = "cluster"`, different `server.listen` ports and
`cluster.node_name`s, and the same site key in `FERRITE_AUTH__SECRET_KEY` (64 hex
characters, e.g. `openssl rand -hex 32`; cluster mode refuses to start without one); `init`
the first folder with `--db postgres://… --schema …`, then
`serve` both. Docker example (not run in CI, only validated; Postgres 18 and RustFS):
`examples/cluster/` (`docker compose -f examples/cluster/docker-compose.yml up --build`;
design 09 §7).
Cluster integration tests: `FERRITE_TEST_PG_URL=… cargo test -p ferrite --test cluster`
(serialised in-process; about 30 s).

Two deployment profiles must both work: **solo** (SQLite + local media, one node) and
**cluster** (Postgres + S3-compatible media, N nodes). See `docs/design/00-overview.md`.

## Principles (don't violate without discussing)

1. **Single binary, single data folder.** No external services required to run a site on
   the default profile. Postgres/S3 are opt-in; the database is the only coordination service.
2. **Plugins are untrusted.** They get only the WIT imports their manifest declares.
   No raw SQL, no filesystem, no shared memory, no ambient network. Every hook call is
   bounded by memory, fuel and an epoch deadline. A failing plugin must never break a page.
3. **Themes don't execute code.** Templates get data from core plus a fixed set of safe
   helpers. Logic belongs in plugins.
4. **WIT files in `wit/` are the source of truth** for the plugin API. Version them;
   breaking changes need a new world version.
5. **WordPress familiarity, not compatibility.** Mirror WP concepts (hooks with
   priorities, template hierarchy, roles, post types) so WP devs feel at home.

## Conventions

- Rust stable, edition 2024, Cargo workspace under `crates/` (layout in `docs/design/01-architecture.md` §1).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` must pass.
- Errors: `thiserror` in libraries, `anyhow` only in the binary crate.
- Async: `tokio`. Logging: `tracing`.
- SQL migrations live in `crates/ferrite-store-sqlite/migrations/` and
  `crates/ferrite-store-postgres/migrations/` — same versions/names on both, every migration
  has a `.down.sql`, expand-only unless marked `-- ferrite:phase contract` (design 02 §7–8).
- Storage changes need contract tests on **both** backends.
- Tests: unit tests next to code; integration tests in `tests/`. Plugin-host tests must
  include hostile plugins (infinite loop, OOM, panic, undeclared import).
- Postgres-backed store tests: set `FERRITE_TEST_PG_URL` to any Postgres >= 15, e.g.
  `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:5432/ferrite_test cargo test --workspace`.
  Each test creates and drops its own schema, so a shared database is fine. Without the
  variable the Postgres cases print `SKIP` and pass; CI (which sets `CI=true`) panics if
  it is missing and runs a `postgres:18` service.
- API changes: DTOs in `crates/ferrite-http/src/api/` with `utoipa` annotations; regenerate the
  checked-in spec with `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`
  (`openapi_snapshot_is_current` fails on drift). Capabilities are checked in core, never in
  handlers.
- Prefer small, reviewable commits per checklist item.

## Naming

- Product: FerriteCMS. Binary/CLI: `ferrite`. Crates: `ferrite-*`.
- Plugin manifest: `ferrite-plugin.toml`. Site config: `ferrite.toml`. DB: `ferrite.db`.
- Name is provisional — keep it easy to rename (no name baked into data formats beyond
  the file names above).
