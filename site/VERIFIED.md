# What the documentation claims, and how it was checked

Every claim in `site/content/` is listed here with what it was checked against. "Run"
means the command was run on 2026-10-31 with a debug build of branch `m4-3b-site-1`
(this branch: `m4-3` plus the `docs` theme), in a scratch folder, unless the row says
otherwise. Paths are relative to the repository root; line numbers are those of that
build. The pages name the data folder `./mysite`; the runs used `./site` in a scratch
folder (the name changes nothing). A row marked **not run** or **not verified** says why.

Part 1 (Task 7) covers the getting-started pages and the guides. Part 2 (Task 8) adds
its own rows.

## Introduction (`content/index.md`)

| Page | Claim | Checked against (file:line, test name, or command run) | Result |
|---|---|---|---|
| index | A WordPress-style CMS as a single Rust binary; web server, REST API, admin, built-in themes and plugin runtime compiled in | `CLAUDE.md` "What this project is"; `crates/ferrite-http` embeds `admin/dist` (`admin/README.md`); themes embedded with rust-embed (`docs/design/06-themes-rendering.md` §1); `ferrite --version` run | Verified (the admin only when `admin/dist` is built first; the Install page says so) |
| index | Every plugin runs in a WebAssembly sandbox and can only use what its manifest declares | `CLAUDE.md` principle 2; `docs/design/05-plugin-host.md`; `ferrite plugin check` run on `seo` (the report lists exactly the manifest's capabilities) | Verified |
| index | Imports WordPress WXR, does not run WordPress PHP plugins or themes | `docs/design/00-overview.md` "Non-goals"; `ferrite import wordpress --help` | Verified |
| index | Nothing released: no release binaries, no published image, no hosted docs | `docs/design/09-operations.md` §7 ("Nothing has been published yet"); `crates/ferrite/release.pub` is a placeholder; P63 (hosting is the user's call) | Verified |
| index | Default setup needs no other service: SQLite in the data folder, media next to it; Postgres and S3 optional | `ferrite init` run: `ferrite.toml` has `url = "sqlite://ferrite.db"`, `backend = "local"`, `root = "media"`; `00-overview.md` "Deployment profiles" | Verified |
| index | Plugins get only declared host functions; no SQL, no file system, no network unless declared; memory, fuel and deadline per call; a failing plugin does not break the page | `CLAUDE.md` principle 2; `[plugins]` defaults in the written `ferrite.toml` (`default_memory = "32MB"`, `default_fuel = 50000000`, `render_deadline_ms = 200`; `crates/ferrite-config/src/load.rs:1393`) | Verified against design and config; the sandbox itself is covered by the hostile-plugin tests (`plugins/examples/hostile/`), not re-run here |
| index | Themes don't run code; templates get data and a fixed set of helpers | `CLAUDE.md` principle 3; `06-themes-rendering.md` §4 | Verified |
| index | Posts, pages, content types, categories, tags, menus, roles admin/editor/author/contributor, hooks with priorities, template hierarchy | `crates/ferrite-store-sqlite/migrations/0001_init.up.sql:248-255`; `ferrite user create --help` (role values); `06-themes-rendering.md` §2; plugin manifests' `[hooks]` priorities (`plugins/examples/seo/ferrite-plugin.toml`) | Verified |
| index | Profiles table: SQLite in `<data>/ferrite.db` / Postgres; `<data>/media/` / S3; one node / one or more; coordination in-process / through Postgres | `docs/design/00-overview.md` "Deployment profiles" | Verified |
| index | A cluster needs Postgres, shared media storage and the same site key on every node | `09-operations.md` §8 `ferrite init` ("Cluster configs need the key up front"); `CLAUDE.md` "Run two cluster nodes locally" | Verified |

## Getting started (`content/01-getting-started/`)

| Page | Claim | Checked against (file:line, test name, or command run) | Result |
|---|---|---|---|
| install | Building from source is the only way that works today; releases and the image are not published | `09-operations.md` §7; `crates/ferrite/release.pub` (placeholder) | Verified |
| install | `rust-toolchain.toml` names stable and the `wasm32-wasip2` target | `rust-toolchain.toml` | Verified |
| install | Node 24.18 or newer and pnpm 10.28.2 for the admin | `admin/package.json` (`engines.node >=24.18`, `packageManager pnpm@10.28.2`) | Verified |
| install | Without the admin build, `/admin/` shows a page saying the admin was not built | `curl http://localhost:8080/admin/` on a build without `admin/dist`: "FerriteCMS admin not built" | Verified (run) |
| install | `git clone https://github.com/softberries/ferritecms.git` | `git remote -v`; `crates/ferrite/src/update/release.rs:13` | **Not run**: the clone was not made (the repository's public visibility was not checked) |
| install | `pnpm -C admin install`, `pnpm -C admin build` | Run (exit 0, `check-dist: ok`) | Verified |
| install | `cargo build --release --locked -p ferrite`; binary at `target/release/ferrite` | Run inside `docker build` (the Dockerfile's builder stage runs exactly this command, exit 0); `cargo build -p ferrite` run natively | Verified in the container; the native release build was not run |
| install | `ferrite --version` | Run: `ferrite 0.1.0` | Verified |
| install | Six targets, the glibc 2.35 note, static musl builds needing CA certificates, the two macOS targets | `09-operations.md` §7 table; `.github/workflows/release.yml:125-149`; `packaging/systemd/README.md` | Verified against the workflow; **not run** (no release exists) |
| install | Asset names `ferrite-<version>-<target>.tar.gz`, `ferrite-plugins-<version>.tar.gz`, `SHA256SUMS`, `SHA256SUMS.minisig` | `09-operations.md` §1, §7; `.github/scripts/package.sh` | Verified against the scripts; **not run** |
| install | Check the minisign signature, then `sha256sum --check --ignore-missing` | The same commands run on a locally made `SHA256SUMS` signed with a throwaway key (`minisign -G -W`, `minisign -S`): verify exit 0, a tampered file exit 1; `sha256sum --check --ignore-missing` exit 0 | Verified with a throwaway key; the release URLs are **not run** (no release) |
| install | macOS: `shasum -a 256 --check --ignore-missing SHA256SUMS` | Run on this Mac (Darwin 25.6): `a.tar.gz: OK`, exit 0 | Verified |
| install | `release.pub` is `crates/ferrite/release.pub` at the release's tag; today a placeholder, so a source build cannot `self-update` | `crates/ferrite/release.pub`; `09-operations.md` §1 step 2 | Verified |
| install | The archive unpacks to `ferrite-<version>-<target>/` with `ferrite` and `README.md` | `.github/scripts/package.sh:3,27-28` | Verified against the script; not run |
| install | Image `ghcr.io/softberries/ferritecms`, `linux/amd64` + `linux/arm64`, tags `X.Y.Z`, `X.Y` (newest of its minor line), `latest` (newest stable) | `.github/workflows/release.yml:6,397,455-460`; `09-operations.md` §7 (`meta`, `image-tags`) | Verified against the workflow; **not run** (not published) |
| install | `docker build -t ferrite .` | Run as `docker build -t ferrite-t7-docs:local .` (exit 0) | Verified |
| install | Image holds `/ferrite` on distroless, non-root, volume `/data`, starts `serve --data /data` | `Dockerfile` (runtime stage) | Verified |
| install | `docker run … init`, `… user create`, `docker run -d … -e FERRITE_SERVER__LISTEN=0.0.0.0:8080` | Run with the image `ferrite-t7-docs:local`, volume `ferrite-t7-data` and host port 8098: init exit 0, user created, log `Listening on http://0.0.0.0:8080`, `/`, `/admin/`, `/readyz` 200 | Verified (container and volume removed after) |
| install | The server must listen on all addresses inside a container (default `127.0.0.1:8080`) | `crates/ferrite-config/src/load.rs:565-566` | Verified |
| install | `self-update` does not apply in a container | `09-operations.md` §7 "Docker image" | Verified |
| first-site | `ferrite init --data ./site --url http://localhost:8080`; `--title` default `My Site` | `ferrite init --help`; run (exit 0) | Verified |
| first-site | init creates `ferrite.toml` (0600), `ferrite.db`, `media/`, `backups/`, `secret.key` (0600) | Run: `ls -la site` showed `-rw-------` for `ferrite.toml` and `secret.key`, the database, `media/`, `backups/` (with a pre-migration snapshot) | Verified |
| first-site | `media/` also holds the files of installed themes and plugins | `00-overview.md` D11; `06-themes-rendering.md` §1 "Install and selection" (`MediaStore::put_artifact`) | Verified |
| first-site | A new data folder gets mode 0750 | Run: `drwxr-x---` | Verified |
| first-site | init refuses an existing `ferrite.toml` or database | Run twice: `Error: ./site/ferrite.toml already exists; refusing to overwrite it`, exit 1; `09-operations.md` §8 `ferrite init` | Verified |
| first-site | `init` creates no user; `user create` prints a one-time password once, to be changed at first sign-in | Run: `created user me (admin)`, `password: …`, `(shown once; it must be changed at the first login)` | Verified |
| first-site | `--password-stdin`: at least 12 characters, at most 1024 bytes | `crates/ferrite-core/src/auth/password.rs:29,31,51,56`; run with a 28-character password (exit 0) and `short` (`invalid password: password must be at least 12 characters`, exit 1) | Verified |
| first-site | `--role` admin (default), editor, author, contributor | `ferrite user create --help` | Verified |
| first-site | `seed demo` adds 12 posts, three pages, categories, tags, two menus, a sample image, and a user `admin` without a password | Run: `Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).`; `09-operations.md` §8 `seed demo` | Verified |
| first-site | `serve` prints `Listening on http://127.0.0.1:8080`; default listen `127.0.0.1:8080`; Ctrl-C stops it | Run; `load.rs:565-566`; `ferrite serve --help` ("stops cleanly on SIGINT/SIGTERM") | Verified |
| first-site | `FERRITE_<SECTION>__<KEY>` overrides any setting, e.g. `FERRITE_SERVER__LISTEN` | Run: `FERRITE_SERVER__LISTEN=127.0.0.1:8099 ferrite serve` listened on 8099; `CLAUDE.md` | Verified |
| first-site | Sign in with login or e-mail; the admin asks for a new password first, then the dashboard | `07-admin-api.md` §3 "Sign-in and account recovery"; E2E `must_reset_session_is_confined` (`admin/e2e/auth.spec.ts:192`); API login run: `must_reset: true` | Verified against the API and the E2E spec (the E2E suite was not run here) |
| first-site | `⌘K` / `Ctrl+K` opens the command palette | `admin/src/routes/_app.tsx:218` (`mod+k`); `admin/src/lib/shortcuts.ts:4` (`mod` is ⌘ on Apple, Ctrl elsewhere) | Verified |
| first-site | With SQLite, `serve` locks the folder; `user`, `seed`, `theme`, `plugin`, `import`, `migrate` fail with "in use"; Postgres has no folder lock | Run: `ferrite user list` while serving: `data folder is in use: …/ferrite.lock is locked by another process`, exit 1; `09-operations.md` §8 (`migrate`, `user`, `theme`/`plugin`, `import`) | Verified |
| first-site | `config check` reads the config like `serve`, without the database, and prints `ok: mode=solo database=sqlite media=local …` | `ferrite config check --help`; run: `ok: mode=solo database=sqlite media=local workers=14 mail=off metrics=off tls=off backup=off` | Verified |
| concepts | Built-in types post (flat, archive), page (hierarchical), media; taxonomies category (nested) and tag (flat), both for posts | `0001_init.up.sql:248-255` | Verified |
| concepts | Default addresses: posts `/<slug>/`, pages `/<parent>/<slug>/`, other types `/<type>/<slug>/` (parents between), media at `/_media/<hash>/<name>` | `crates/ferrite-core/src/permalink.rs:320,322,324`; `06-themes-rendering.md` §3 static routes | Verified |
| concepts | Taxonomy terms at `/category/<slug>/`, `/tag/<slug>/` | `06-themes-rendering.md` §3 (`/{taxonomy}/{slug}/`) | Verified |
| concepts | The body is stored as a block tree, not HTML | `crates/ferrite-core/src/blocks/`; `06-themes-rendering.md` §5 | Verified |
| concepts | Statuses draft, pending, scheduled, published, trash; only published items are public | `crates/ferrite-domain/src/content.rs:6-16` (also `private`, left out of the page); `06-themes-rendering.md` §4 (`SiteReader`, published only) | Verified |
| concepts | Menus: nested links to items, terms or URLs; theme menu locations; the built-in themes have `primary` and `footer` | `07-admin-api.md` §3 "Menus"; `themes/blog/theme.toml`, `themes/docs/theme.toml`, `themes/business/theme.toml` (branch `m4-3b-business`) | Verified |
| concepts | Themes: MiniJinja templates, CSS, assets, settings; one active | `06-themes-rendering.md` §1, §4 | Verified |
| concepts | Plugins: change the head, filter content, routes under `/_p/<plugin>/`, storage, mail to admins; wasm + `ferrite-plugin.toml`; approved at install | The three example manifests (`render.head`, `routes`, `kv`, `mail = "admins"`); `05-plugin-host.md`; P55 (`content.body` filter) | Verified |
| concepts | Role table (contributor ⊂ author ⊂ editor ⊂ admin) | `crates/ferrite-core/src/actor.rs:287,461-463`; `07-admin-api.md` §1 "Roles & capabilities" | Verified |
| concepts | Copying the data folder with the server stopped copies the site | Run: `cp -Rp site site-copy`, then `serve` on the copy: `/`, `/about/`, `/sitemap.xml` 200 | Verified (solo site with local media) |

## Guides (`content/02-guides/`)

| Page | Claim | Checked against (file:line, test name, or command run) | Result |
|---|---|---|---|
| writing | **New post** / **New page** / **New entry** on the lists, the **New** menu in the top bar; status tabs, search, **Mine**, a filter per taxonomy | `admin/src/i18n/en/content.json:4-6,12-21,26`; `admin/src/i18n/en/shell.json:40-42`; `07-admin-api.md` §3 "Posts / Pages / custom types" | Verified |
| writing | `/` opens the block menu: headings 2-4, lists, quote, code block, table, divider, Image, Embed (YouTube or Vimeo), HTML block (sanitised) | `admin/src/editor/slash-menu.tsx:63-105,227`; `admin/src/i18n/en/editor.json:65-127` | Verified |
| writing | Formatting bar: bold, italic, strikethrough, inline code, link, headings, quote | `admin/src/i18n/en/editor.json:49-63` | Verified |
| writing | Side panel contents; parent and order for hierarchical types | `07-admin-api.md` §3 "Editor" | Verified |
| writing | Shortcuts `mod+s`, `mod+.`, `mod+shift+p`, `mod+alt+r`, `mod+shift+f` | `admin/src/editor/editor.tsx:360,368,407,422,448` | Verified |
| writing | Autosave every 10 s into a private slot; the item does not change; restore banner | `admin/src/editor/autosave.ts:22`; `07-admin-api.md` §3 "Editor safety"; migration `0007_autosaves` (`02-storage.md` table) | Verified |
| writing | A save after someone else's is refused; dialog: keep mine, take theirs (kept in the slot), copy my text | `07-admin-api.md` §3 "Editor" (412 dialog); E2E `admin/e2e/editor-safety.spec.ts` (not run here) | Verified against design |
| writing | Publish dialog Now / Later on the browser's clock with its zone; past times refused; scheduled items publish themselves; Unschedule | `07-admin-api.md` §3 "Editor"; `crates/ferrite-core/src/content/lifecycle.rs:3-6`; `09-operations.md` §8 `serve` (scheduled publishing jobs) | Verified |
| writing | Authors, editors and admins publish own; editors and admins anyone's; contributors submit for review (pending) | `actor.rs:287`; `lifecycle.rs:3-6`; `07-admin-api.md` §1 roles | Verified |
| writing | Unpublish takes a published item back to draft | `lifecycle.rs:4-5` | Verified |
| writing | Preview at `/_preview/<id>` in a new tab; the saved item; signed-in readers only; never cached; `noindex` | `admin/src/editor/panel/status-section.tsx:132`; `06-themes-rendering.md` §3 "Previews" | Verified |
| writing | A revision per save; the newest 50 kept; word diff; Restore with undo | `crates/ferrite-core/src/content.rs:41,655,681-704`; `crates/ferrite-core/src/content/admin.rs:30`; `07-admin-api.md` §3 "Revisions" | Verified (transitions write no revision, `lifecycle.rs:12`) |
| writing | Move to trash with undo; restore as draft or delete permanently; contributors cannot restore or delete | `lifecycle.rs:5`; `07-admin-api.md` §1 (R8) and §3 | Verified |
| media | Upload button and drop anywhere on the page; three uploads at once; Retry; picker and drop into the body | `admin/src/i18n/en/media.json:3`; `admin/src/media/use-upload-queue.ts:11`; `07-admin-api.md` §3 "Media library", "Editor" | Verified |
| media | Upload needs author or higher | `actor.rs:287` | Verified |
| media | 256 MiB default, `max_upload` up to 1 GiB (MB means MiB) | `crates/ferrite-config/src/model.rs:212,214`; `crates/ferrite-config/src/cx.rs:233-244` | Verified |
| media | Type from the content; the table of served-inline and download-only types; HTML, XML, programs refused | `docs/design/03-media.md` §3 "Accepted upload types"; `crates/ferrite-media/src/sniff.rs:9-17` | Verified |
| media | Title and alt text save on leaving the field; the details show address, facts and sizes | `07-admin-api.md` §3 "Media library" | Verified |
| media | Featured image in the panel; `blog` shows it on cards and at the top of the post; `og:image` | `themes/blog/templates/partials/listing.html:6`, `partials/card.html:6`, `single.html:15`, `partials/hero.html:2`; `crates/ferrite-http/src/head.rs:150-151` | Verified |
| media | Inline images via `/` Image or a dropped file; alt text per image | `07-admin-api.md` §3 "Editor" (media, M2b-2) | Verified |
| media | Core sizes thumbnail 150×150 crop, medium 768, large 1600; JPEG, PNG, GIF, WebP get them | `crates/ferrite-media/src/image.rs:61-82`; `sniff.rs:9-13` | Verified |
| media | `blog` adds `card` and `hero`; no copy wider than the original; `srcset`; the original until a copy exists | `themes/blog/theme.toml` `[images]`; `ferrite media regenerate --help`; `03-media.md` §4 "Variants as built" | Verified |
| media | `ferrite media regenerate --data ./site` after switching themes; stop `serve` with SQLite | `ferrite media regenerate --help`; run: `regenerated 0, skipped 1, failed 0`, exit 0 | Verified |
| media | Copies never carry EXIF/XMP; `strip_metadata = true` strips JPEG, PNG and WebP originals (default false) | `03-media.md` §4; `model.rs:252`; `load.rs:1314` | Verified |
| media | Trash view restore or delete; daily clean-up of unreferenced files after 7 days (`gc_grace_days`) | `03-media.md` §6; `model.rs:220`; `09-operations.md` §8 `serve` (`artifacts.gc`) | Verified |
| media | Files in `<data>/media/` (solo), an S3 bucket (cluster) | `ferrite init` run (`root = "media"`); `00-overview.md` profiles | Verified |
| content-types | Structure menu; types and taxonomies need an admin; editors manage terms and menus | `admin/src/i18n/en/shell.json:13-17`; `07-admin-api.md` §3 (`manage_settings`, `manage_terms`, `manage_menus`); `actor.rs` | Verified |
| content-types | Type list (every type but media, built-ins first); New type: label, name `[a-z][a-z0-9_-]{0,31}`, hierarchical, archive | `crates/ferrite-core/src/types.rs:10,52`; `07-admin-api.md` §3 "Content types" | Verified |
| content-types | Name and hierarchy fixed, label and archive editable; archive at `/<name>/` | `07-admin-api.md` §3 "Content types"; `06-themes-rendering.md` §3 (`/{type}/`) | Verified |
| content-types | Custom types get a sidebar entry | `admin/src/shell/nav.ts:177-182` | Verified |
| content-types | Delete only without items and without a relation pointing at it; built-ins cannot be deleted | `07-admin-api.md` §3 "Content types" (M2b-3 Task 12) | Verified |
| content-types | Field kinds; choice up to 100 options; relation limited to types; repeater two levels; 100 fields | `crates/ferrite-core/src/fields.rs:5-15,39,41`; `07-admin-api.md` §3 "Content types", "Editor" | Verified |
| content-types | Fields in the panel, or a card under the body with more than four on a wide screen; `.` decimal separator | `07-admin-api.md` §3 "Editor" (M2b-2 Task 8) | Verified |
| content-types | The builder lists removing, renaming, kind and required changes first; stored values stay until an item is saved | `07-admin-api.md` §3 "Content types" | Verified |
| content-types | New taxonomy: label, name, nesting, content types; tree with drag or Move to…; siblings by name; no delete with sub-terms; editor sections | `07-admin-api.md` §3 "Taxonomies", "Editor" | Verified |
| content-types | Term pages at `/<taxonomy>/<slug>/` list their items | `06-themes-rendering.md` §3 "Term archives" | Verified |
| content-types | Menus: theme locations; Content (types in the sidebar), Terms, Custom link (`http(s)://` or a path); 16 levels, 500 entries; saved at once | `07-admin-api.md` §3 "Menus"; `crates/ferrite-core/src/menu.rs:20`; `crates/ferrite-http/src/app/bridge.rs:25`; `admin/src/structure/menus/menu-model.ts:36-39` | Verified |
| themes | Three built-in themes, MIT, plain CSS, light and dark | `themes/blog/theme.toml`, `themes/docs/theme.toml`; `06-themes-rendering.md` §8; `themes/business/theme.toml` on branch `m4-3b-business` (2dd38e3) | `blog` and `docs` verified on this branch; **`business` is not in this branch** (its branch is not merged into `m4-3` yet): checked against `m4-3b-business` only |
| themes | Settings per theme (table) | The three `theme.toml` `[settings]` tables | Verified (`business` on its branch) |
| themes | Menu locations `primary` and `footer` in all three | The three `theme.toml` `[menus]` | Verified (`business` on its branch) |
| themes | `docs` sidebar shows one type's tree (`page` by default), in the pages' order | `themes/docs/theme.toml` (`nav_type`); `06-themes-rendering.md` §8; the built docs site's sidebar (below) | Verified |
| themes | `business`: child pages as cards; contact page (slug `contact`) embeds the contact-form plugin's form behind a setting; `seed demo --profile business` | Branch `m4-3b-business`: `themes/business/templates/front-page.html`, `page-contact.html`, `theme.toml` (`contact_form` label), `crates/ferrite/src/seed/business.rs` | Checked on that branch only; **not run** (not in this branch's binary) |
| themes | Activate in Appearance; each node loads it and flushes its cache; the admin waits for every node | `07-admin-api.md` §3 "Themes"; `06-themes-rendering.md` §1 "Hot swap" | Verified |
| themes | `ferrite theme list`, `ferrite theme activate --data ./site docs`; a theme that does not load is refused | Run: list printed `blog … embedded active`, `docs … embedded`; `activated theme docs`; `activate nosuch`: `theme not found`, exit 1 | Verified |
| themes | Customize on the card; saving refreshes every page | `07-admin-api.md` §3 "Themes"; `06-themes-rendering.md` §1 (`option:theme.<name>.settings` on every page) | Verified |
| themes | Copy a theme, rename it in `theme.toml`, install it | Run: `themes/blog` copied to `my-theme`, renamed; `ferrite theme install --data ./site --dir ./my-theme --activate`: `installed theme my-theme 0.1.0 (active)` | Verified |
| themes | Admin Upload theme (folder packed in the browser, or a bundle); `ferrite theme pack --dir ./my-theme --out my-theme.tar` | `07-admin-api.md` §3 "Themes"; run: `packed 21 files, 52224 bytes, sha256 …` | Verified |
| themes | Templates compiled first; errors with file and line; same name replaces; limits 2000 files and 20 MiB | Run with a broken `index.html`: `templates/index.html:1: syntax error: unknown statement broken`, exit 1; `06-themes-rendering.md` §1 "Loading" | Verified |
| plugins | A plugin is `ferrite-plugin.toml` plus one `*.wasm`; it gets only what it lists | `ferrite plugin check --help`; `05-plugin-host.md` | Verified |
| plugins | Memory, fuel and time limits per call; a failed call is ignored and the page renders | `CLAUDE.md` principle 2; `05-plugin-host.md` | Verified against design (hostile-plugin tests not re-run) |
| plugins | Auto-disable after 20 failures in 10 minutes (`disable_after_failures`, `failure_window`), reason shown | `crates/ferrite-config/src/tests.rs` `defaults_minimal` (lines 61-62); `05-plugin-host.md` (the failure flusher, `auto_disable`); `07-admin-api.md` §3 "Plugins" (`disabled_reason`) | Verified |
| plugins | The report: component SHA-256, a line per capability and action hook, `!` high, `~` medium, limits the manifest sets, `fingerprint` | Run: `ferrite plugin check` on `seo` printed `component: …`, three permission lines (`!` before `render.head`), `fingerprint: …`; `crates/ferrite/src/ext_cmd.rs:473-478` (limits only when `[limits]` is set) | Verified |
| plugins | Approval is of that exact report; a widening update lists what it adds and needs approval | `09-operations.md` §8 `theme`/`plugin`; `00-overview.md` (M2a-2 P3) | Verified against design |
| plugins | `plugin check` stores nothing; `plugin install --enable`; asks `install? [y/N]`; `--yes` required without a terminal; installed disabled by default | Run: `ok: nothing was stored`; without `--yes` and no terminal: `needs --yes when stdin is not a terminal; nothing was installed`, exit 1; with `--yes --enable`: `(enabled)`; reinstall without `--enable`: `(disabled)`; `09-operations.md` §8 (`install? [y/N]`) | Verified (the prompt itself needs a terminal, not run) |
| plugins | `plugin list`, `enable`, `disable` and `remove` | Run: each exit 0 (`seo  0.1.0  enabled`, `disabled plugin seo`, `enabled plugin seo`) | Verified |
| plugins | Admin Install plugin: pick both files, report with high-risk first, Approve and install, installed disabled, Enable offered | `07-admin-api.md` §3 "Plugins"; E2E `admin/e2e/plugins.spec.ts` (not run here) | Verified against design |
| plugins | Settings on the plugin page; secrets write-only; failures per hook over 24 h and call times | `07-admin-api.md` §3 "Plugins" | Verified |
| plugins | Remove keeps data and settings 30 days; `--purge` or Delete and erase data | Run: `removed plugin seo; its KV and settings are kept for 30 days (--purge deletes them now)`; `--purge`: `removed plugin seo and purged its data` | Verified |
| plugins | The three example plugins: what they do and their capabilities | `plugins/examples/{seo,analytics,contact-form}/ferrite-plugin.toml` and `README.md` | Verified |
| plugins | Releases will carry `ferrite-plugins-<version>.tar.gz` | `09-operations.md` §7 (`plugins` job) | Verified; **not run** |
| plugins | Build commands; `contact_form.wasm` | Run: `cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml -p seo` (exit 0), the `mkdir`/`cp` lines; `plugins/target/wasm32-wasip2/release/contact_form.wasm` exists | Verified |
| plugins | Contact form mails only with `[mail]` (`smtp_url`, `from`); otherwise keeps messages | `plugins/examples/contact-form/README.md` "Install" | Verified against the README (mail not run) |
| import-wordpress | What comes over (types, attachments with alt, taxonomies, authors matched by e-mail then login or created as authors, comments, menus without locations, title and description, Yoast/Rank Math, featured images, ACF on custom types, 301s) | `docs/design/08-wp-import.md` §3, §5; run on `crates/ferrite-import-wp/tests/corpus/site-a.xml`: posts, pages, a custom type `book`, media, menus, options, redirects, taxonomies, terms, users created | Verified |
| import-wordpress | Gutenberg, classic HTML and `[caption]`, `[gallery]`, `[embed]`, `[video]`, `[audio]` converted; the rest reported | `08-wp-import.md` §4, §6 | Verified |
| import-wordpress | Imported users have no password and must reset | `08-wp-import.md` §3 (`must_reset`), `00-overview.md` D19 | Verified |
| import-wordpress | Tools → Export makes the file | `ferrite import wordpress --help` ("The WXR file (Tools → Export in WordPress)") | Verified against the help text (WordPress itself not run) |
| import-wordpress | `--dry-run` rolls back; with SQLite stop `serve` | `ferrite import wordpress --help`; run: dry run exit 0 with its report | Verified |
| import-wordpress | `ferrite import wordpress export.xml --data ./site --uploads ./uploads` | Run on the corpus (`site-a.xml`, `site-a-uploads`): exit 0, `media variants are made by the next ferrite serve` | Verified |
| import-wordpress | Options table | `ferrite import wordpress --help` | Verified |
| import-wordpress | Exit codes 0, 2, 3, 1 | `ferrite import wordpress --help` | Verified |
| import-wordpress | Admin Import screen (`g i`), drop the file and a zip, background job, progress, report from the runs | `07-admin-api.md` §3 "Import"; `08-wp-import.md` §1; E2E `admin/e2e/import.spec.ts` (not run here) | Verified against design |
| import-wordpress | Report files at `<data>/import/reports/<run-id>/` and their contents | Run: `report.json` and `report.html` in that folder; `08-wp-import.md` §6 | Verified |
| import-wordpress | A re-run updates what changed, keeps what was edited here, matches terms | Run a second time: `unchanged` counts; `CLAUDE.md`; `08-wp-import.md` §2 (P4) | Verified (unchanged case run; the kept case from the design and the M3 tests) |
| import-markdown | Folders become parent pages; these docs are imported this way | `site/build.sh` run (below) | Verified |
| import-markdown | `ferrite import markdown ./docs --data ./site`; stop `serve` with SQLite; `--dry-run` rolls back | `ferrite import markdown --help`; run on a scratch folder: dry run `dry run: nothing was written` | Verified |
| import-markdown | `NN-` prefix gives order and slug; `index.md` or `README.md` is the folder's page; a folder without an index gets an empty page titled from its name; the root `index.md` is a top-level page with slug `home` | P62; `08-wp-import.md` §8; `crates/ferrite-import-md/src/scan.rs:494-514`; run: `synthesized  02-guides …`; `README.md` as an index run; the docs site serves `/home/` and `/getting-started/install/` | Verified |
| import-markdown | Hidden files skipped; symlinks never followed | P62; `08-wp-import.md` §8 "Never read"; test `outside_dir_is_never_read` (`crates/ferrite/tests/import_md.rs:718`) | Verified against the code and tests (not re-run) |
| import-markdown | Title: front matter, else the first `# heading` (removed), else the file name | P62; `08-wp-import.md` §8 front matter table; the docs site's pages each have one `<h1>` (below) | Verified |
| import-markdown | Front matter keys and effects; any other key, invalid TOML or YAML skips the file | `ferrite import markdown --help`; `08-wp-import.md` §8; run: `bad-front-matter  yaml.md  line 1: skipped, YAML front matter (---) is not supported …`, exit 2 | Verified |
| import-markdown | `draft = true` makes a draft; a future `date` schedules | `08-wp-import.md` §8; tests `a_draft_root_index_is_not_the_front_page`, `a_future_date_schedules_the_page` (`import_md.rs:1155,1190`) | Verified against the code and tests (not re-run) |
| import-markdown | CommonMark plus tables and strikethrough | `crates/ferrite-import-md/src/convert.rs:36` | Verified |
| import-markdown | Links to `.md` files and folders rewritten with fragments; a `.md` not in the import reported broken | Run: `broken-link  index.md  link … is not in the import, kept as is`; the docs site has no `.md` link left and every internal link answers 200 (below) | Verified |
| import-markdown | A relative image inside the folder becomes media; one outside is never read and is reported; `http(s)` kept | Run: `media 1` for `pic.png`, `outside  a/README.md  ../../export.xml is outside the folder or a symlink, not read`, exit 2; `08-wp-import.md` §8 | Verified |
| import-markdown | `--front-page` makes the root index the front page while it is published | Run: `front page: index.md`; the docs site's `/` is the root page; `08-wp-import.md` §8 "Front page" | Verified |
| import-markdown | Re-runs: unchanged, updated, kept (edited here), renamed left and reported; source name `--source` or the folder's name | Run: re-run `unchanged 4`; edited file `updated 1`; renamed file `created 1` and `not-in-source … kept`; the kept case: test `an_admin_edit_is_kept` (`import_md.rs:494`) | Verified (kept case from the test, not re-run) |
| import-markdown | The counts line and one line per problem; exit 0, 2, 3, 1 | Run: `created 4, updated 0, unchanged 0, kept (edited here) 0, media 0`; `ferrite import markdown --help` | Verified |
| import-markdown | 5000 files and 5000 folders (an error, nothing written); 1 MiB per file and folders deeper than 64 skipped and reported | `crates/ferrite-import-md/src/scan.rs:16,18,21,673-680`; `crates/ferrite/src/import_cmd.rs:238,243` (the scan runs before the run starts) | Verified |
| import-markdown | `--type` (default `page`; hierarchical when nested; `--front-page` needs `page`), `--as` (default the first active admin) | `ferrite import markdown --help`; `09-operations.md` §8 | Verified |

## The site build

| Page | Claim | Checked against (file:line, test name, or command run) | Result |
|---|---|---|---|
| README | `site/build.sh OUT_DIR [URL]` builds the site, prints the serve command | Run: `site/build.sh <scratch>/docs-site http://localhost:8099`, exit 0 (output in the Task 7 report) | Verified |
| README | It fails when the import exits 2 | Run with a temporary `zz-tmp-broken.md` linking to `nope.md`: `broken-link …`, build exit 2 (file removed after) | Verified |
| README | Re-running the import updates changed pages | Run: `created 0, updated 3, unchanged 10, kept (edited here) 0, media 0` | Verified |
| site | Every page answers 200, has one `<h1>`; every internal link and TOC anchor resolves; the sidebar follows the `NN-` order | Run: curl of every sitemap URL plus `/`; a link checker over all pages (0 problems); the sidebar order read from `/getting-started/install/` | Verified |
