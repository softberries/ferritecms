# M4-4 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M4-4; plan
[`2026-10-31-m4-4-follow-ups.md`](plans/2026-10-31-m4-4-follow-ups.md), Task 9, ruling P66):
*close the follow-ups that can lose data, show a visitor or an admin something wrong, leak
across a permission boundary, flake CI or make the first release irreproducible; move the
rest of the M4-4 lists to "Later" or a "Before v0.1.0" checklist. The exit runs the gates
(both workspaces, the workspace tests with Postgres twice, the plugins' host tests, the admin
gates, the Playwright projects `chromium-sqlite`, `chromium-pg`, `firefox-sqlite`,
`webkit-sqlite` and `public-themes-chromium`, `site/build.sh`) and, by hand with the release
build, a restore that hits P68 and P69, `media gc`'s exit codes, `/home/` → `/` on the docs
site, the sitemap of a 1200-post WordPress import and the reproducible archive hash twice.*

**Result (2026-10-31, branch `m4-4`; the product code is that of `a775396`, and this exit
adds docs and one E2E test fix, `2c121bd`):** every end-to-end check passed on macOS 26.6 (arm64)
with local PostgreSQL 17.11 (port 54329), `rustc` 1.99.1, Node 24.18.0 and pnpm 10.28.2. Both
workspace test runs passed with no failure and no `SKIP`. The first full Playwright run found
two E2E defects that M4-4 had introduced and its tasks had not run in every project (one spec
built a state Task 4's `check_parent` now refuses; one waited for network idle while it held
a request); both are fixed in the spec, with no assertion removed (§3). The second full run
had one failure, in a test M4-4 did not touch, which passed alone ten times out of ten (§3).
Nothing was pushed, tagged, published or hosted.

| Exit check | Verified by | Result |
|---|---|---|
| (a) A restore that hits P68 and P69: a snapshot written by the scheduled `site.backup` job while `serve` ran, restored; a snapshot whose final check fails; `--force` with a media root inside the folder it replaces | §1 (a) | yes |
| (b) `media gc` exit codes: the first pass, a failed delete, another install's marker, a dry run, `--claim`, an unreadable marker | §1 (b) | yes |
| (c) The docs site: `site/build.sh` with the release binary, `/home/` → `/`, the sitemap names `/`, every sitemap URL 200 | §1 (c) | yes |
| (d) A 1200-post WordPress import: the sitemap index, one file per type, `lastmod`, every URL 200, hidden posts absent | §1 (d) | yes |
| (e) The release archives packed twice: the same sha256 | §1 (e) | yes (bsdtar only) |
| (f) Gates | §2 | yes, but for one E2E test that failed once: the workspace twice 3922 passed, 0 failed, 2 ignored, no `SKIP`; Playwright 802 tests, the second full run 801 passed, 1 failed, then 10 of 10 alone (§3) |

## What shipped

Per task (merges into `m4-4`; rulings in [`rulings/m4.md`](rulings/m4.md), "M4-4 plan
rulings" P66–P84 and "M4-4 execution rulings" R-c1–R-c3; each task's report and review are in
the controller's ledger):

1. **Ops data safety** (merge `bbecac0`; P68–P75, R-c2):
   - `restore` exits 2 with "restored into DIR, but restore could not check it afterwards"
     when its final config/store check fails; an absolute `[media] root` inside a folder that
     `--force` replaces (or an absent target) is refused before anything moves; the prune-race
     message names `--keep` (P68).
   - `restore` marks the snapshot's own `running` `site.backup` rows `done` (P69).
   - Opening a SQLite store refuses a missing `ferrite.db` beside a
     `ferrite.db.restore-*.tmp`, and (R-c2) a `ferrite.db` whose `-wal`/`-shm` an interrupted
     `--restore-snapshot` moved into `backups/`, naming the `mv` back; a tmp from which nothing
     moved warns (P70). `rollback --restore-snapshot` refuses to start over such leftovers
     (judged under the folder lock); a failed undo stops at its first failed move and keeps
     the staged copy; `doctor`'s `fs.database_files` reports the same states.
   - `self-update --data` runs the new binary's `config check --data` before `migrate
     status` and refuses on exit 1; `--skip-config-check` (P71).
   - `rollback --schema` on Postgres refuses while a node's heartbeat is fresher than
     `STALE_AFTER`, naming the nodes (P72).
   - `--restore-snapshot` opens the snapshot read-only and refuses unless its applied schema
     equals the record's `schema_from`; a refused gid-only change keeps the owner's group with
     a warning (P73).
   - `media gc` exits 2 for a refused store (another install's marker, an unreadable marker,
     a failed claim) or a failed delete; the marker-only first pass stays 0 (P74).
   - `Periodic::offset`; `artifacts.gc` is due at 03:00 UTC, `site.backup` keeps 00:00 (P75).
2. **Public site correctness** (merge `958b42e`; P67): the front page's own permalink (with
   any `?cpage=N`) answers `301` to `/` with `Cache-Control: public, max-age=3600`, cached
   like a page; `/`, children of the front page and previews are untouched; `SiteReader::tree`
   and `SiteReader::links` (menus, the comment form's redirects) name the front page `/`;
   `image()`'s `srcset` keeps the chosen file's shape; `ContentTree.media` seeds the render's
   media map, so card images spend no bridge calls; `menu_order_key` with
   `sort_by_cached_key`; `TreeNode::excerpt` documented.
3. **Sitemap reads** (merge `6abe884`; P76, P67): `ContentRepo::file_bounds` on both
   backends (a window function plus a `GROUP BY` per file: each file's start cursor and
   `max(updated_at)`), `CountingStore::listed_rows`; the index reads no content rows, a kind
   file seeks to its cursor and walks only its own rows; the sitemap names the front page
   `/`. A golden test captured before the change (`sitemaps_are_unchanged`, both backends)
   differs only by `/home/` → `/`.
4. **Security and API consistency** (merge `2775818`; P77): theme `homepage` through
   `web_url`; `check_relations` and `check_parent` treat an item the actor cannot read as
   missing (`readable_target`); a blank `q` is no filter on `GET /users` (and documented for
   `/redirects`); `settings::site_title_or_configured` for the reset, TOTP-issuer and test
   mail and plugin `notify-admins`; `mail_prefix` uses core `mail::subject_title`; WordPress
   sources must be http(s) (`ImportError::Source`); `s3_client_options` keeps the
   environment's `AWS_*` client options; the contact form's `valid_email` refuses Cf
   characters. OpenAPI: two description strings (`openapi.json`, `admin/src/api/schema.d.ts`).
5. **Import robustness** (merge `8920c33`; P78–P80): the Markdown scan caps the total at
   256 MiB (`MdError::TooMuchText`), counts only folders that hold a page toward 5000 and caps
   the walk at 50 000 folders; uploads-zip entries that collide after NFC (readable or not)
   are all reported as missing media (so are entries that collide only after the folders
   above `uploads/` are dropped, which the first entry used to win; see §4); an import
   run's owner job is live only while `running`
   with an unexpired lease (Postgres with `now()`), in both stores and `job_may_start`; the
   `job` span names the node, `import.wordpress` logs `started`/`done`, a database error
   deleting an input row is a `warn`; the truncated-zip regression test.
6. **Admin polish** (merge `4014b1d`; P81, R-c3): `icon-xs`/`icon-sm` buttons are 44 px on
   coarse pointers (input-group buttons get a 44 × 44 hit area inside their field; term chips,
   dialog and sheet headers make room); a `--popover-edge` token on popups, and dark
   `--shadow-*` made transparent instead of `none` so rings draw at all; the palette's stale
   rows keep full colour (`aria-busy`, a spinner in place of the search icon), and nine other
   loading states use a `stale` prop with a `StaleBar`; SEO fields count code points, keep a
   BOM, refuse inserts past the limit and announce limit crossings; the import screen's live
   regions exist before their first text, and the in-progress wait offers Retry; the 7-day
   axis ticks a first bucket at local midnight.
7. **Test reliability and dev ergonomics** (merge `a775396`; P82, P84): argon2 and blake2 at
   `opt-level = 3` in dev and test builds; `ferrite_store_tests::window::in_one_window` for
   every rate-window scenario; `Registry::beat_once`, awaited by `serve` before it serves; a 6 s
   lease in `lease_expiry_after_crashed_node`; 5 s render deadlines for tests that assert
   plugin output; `node.rs` ceilings; SQLite `Store::close()` unlocks `ferrite.lock`; `Command`
   through `useImperativeHandle`; the E2E media guard knows the test's own uploads, the WebKit
   rule counts navigation requests, `users_reset_email_reaches_the_sink`; `ferrite-theme`'s
   `build.rs` reruns on `themes/`; the stale-binary hint; `[::1]` in `site/build.sh`; SDK
   `head.rs` and two `form` tests; the doctor arm; CI runs the plugins' host tests.
8. **Reproducible release archives** (merge `cec19b4`; P83): `.github/scripts/repro-tar.sh`
   (mtime 0, a NUL-separated `LC_ALL=C` sorted member list, ustar, owner 0, `gzip -n -9`;
   bsdtar and GNU tar branches) used by `package.sh` and `package-plugins.sh`;
   `test-repro.sh` in CI's `packaging` job and in each release build job;
   `self_update_installs_an_archive_made_by_package_sh`.

54 non-merge commits, 222 files, +8 435 / −1 838 lines from the plan commit `b3aba62` to
`a775396`. No migration (the schema stays at 12). No WIT change (the world stays `0.1.0`).
No new external dependency (`ferrite-store-tests` gains the workspace's `futures`).

## 1. End-to-end checks

The release binary and the admin of `a775396`: `pnpm -C admin install --frozen-lockfile &&
pnpm -C admin build` (`check-dist: ok`), then `cargo build --release -p ferrite` (2 m 06 s),
copied to the scratch folder `$S` (the session scratchpad's `m44-t9/`, left in place; every
command below ran with `FERRITE_LOG=warn`, paths shortened to `$S`).

### (a) Restore: P69 and P68

A seeded site served with `FERRITE_BACKUP__SCHEDULE=daily FERRITE_BACKUP__TO=$S/repo-a`: the
day's `site.backup` job ran at start-up and wrote snapshot `20261031T170117Z`, then `serve`
was stopped. The snapshot's database holds that job's own row as running, the live site's
row is done:

```
$ sqlite3 -readonly "file:repo-a/snapshots/20261031T170117Z/ferrite.db?immutable=1" \
    "select kind,status,locked_by is not null,locked_until is not null from jobs where kind='site.backup'"
site.backup|running|1|1
$ ferrite restore $S/repo-a --data $S/a1r            # exit 0
Restored snapshot 20261031T170117Z of $S/repo-a into $S/a1r (taken 2026-10-31T17:01:17.944610Z by ferrite 0.1.0, schema 12).
Stored 5 artifacts in the media store (0 were already there).
database version: 12 … verdict: up to date
$ sqlite3 -readonly a1r/ferrite.db "select kind,status,locked_by,locked_until,finished_at is not null from jobs where kind='site.backup'"
site.backup|done|||1
```

P68, final check: a copy of the repository whose snapshot records migration 2 as failed
(`UPDATE _sqlx_migrations SET success = 0 WHERE version = 2`, the manifest's size and sha256
rewritten, as the test `restore_that_cannot_check_the_result_exits_2` does):

```
$ ferrite restore $S/repo-b --data $S/a1x            # exit 2
Restored snapshot 20261031T170117Z of $S/repo-b into $S/a1x (…, schema 12).
Stored 5 artifacts in the media store (0 were already there).
error: the site was restored into $S/a1x, but restore could not check it afterwards: migration error: migration 2 failed partway (dirty); repair the database manually. Look at it with `ferrite migrate status --data $S/a1x` and `ferrite doctor --data $S/a1x` before serving it.
```

`$S/a1x` holds `ferrite.db`, its WAL, `ferrite.toml`, `media/` and `secret.key`.

P68, media root: `--force` over the restored `$S/a1r` with `FERRITE_MEDIA__ROOT=$S/a1r/media`
exits 1 with "the restored configuration keeps the media at $S/a1r/media, inside $S/a1r,
which --force moves aside: the restored site would be left without its media. Restore into an
empty folder instead (…), or point [media] root (FERRITE_MEDIA__ROOT) elsewhere; nothing was
changed". The database's sha256 and the folder listing are the same before and after, and no
staging or moved-aside folder appeared.

### (b) `media gc` exit codes

A fresh `init` site `$S/b1`:

| Pass | State | Output (last part) | Exit |
|---|---|---|---|
| 1 | no owner marker | "deletes skipped: the media store had no owner marker; this pass wrote ferrite-owner.json for this site …" | 0 |
| 2 | marker written | "marked 0, … deleted tmp 0" | 0 |
| 3 | `--grace-days 1`, a row-less blob dated 2026-10-27 in an `artifacts/9d/` folder made `0555` | a `WARN` "cannot delete an artifact blob; a later GC pass retries … Permission denied", then "…, failed 1" | 2 |
| 4 | the folder `0755` again | "deleted unrecorded blobs 1 (31 bytes)" | 0 |
| 5 | `install_id` in `ferrite-owner.json` changed to another install's | an `ERROR` naming both installs and `--claim`, "deletes skipped: the media store belongs to install 01a192e9-0000-…" | 2 |
| 6 | the same, `--dry-run` | "dry run: … deletes skipped: …" | 2 |
| 7 | `--claim` | a `WARN` "claimed the media store for this site", "claimed the media store from install …" | 0 |
| 8 | after the claim | "marked 0, … deleted tmp 0" | 0 |
| 9 | `ferrite-owner.json` replaced by `not json` | "deletes skipped: ferrite-owner.json cannot be read (… not an owner marker …); `ferrite media gc --claim` rewrites it" | 2 |

Exit 3 (another GC holds the lock) was not provoked by hand; `media_gc_cli_busy_exit_3`
(Postgres) passed in both workspace runs (§2).

### (c) The docs site: `/home/` → `/`

`FERRITE=$S/ferrite site/build.sh $S/docs http://localhost:8403`: exit 0, "created 26,
updated 0, unchanged 0, kept (edited here) 0, media 0", "front page: index.md", "activated
theme docs", and the `serve` command with `FERRITE_SERVER__LISTEN=127.0.0.1:8403`. Served:

| Request | Answer |
|---|---|
| `/home/` | `301`, `location: /`, `cache-control: public, max-age=3600` (the same on the second, cached request) |
| `/home/?cpage=2` | `301`, `location: /?cpage=2` |
| `/home/?utm_source=x&cpage=3` | `301`, `location: /?cpage=3` |
| `/home` | `301`, `location: /home/` (the slash redirect first, then the one above) |
| `/`, `/?cpage=2` | `200` |

The sitemap index lists one file, `sitemap-page-1.xml` (`lastmod` 2026-10-31T17:02:20Z); it
has 26 `<loc>`s, `http://localhost:8403/` among them and none with `/home/`, and each of the
26 answers `200`; `sitemap-page-2.xml` is `404`. On a nested page (`/operations/https/`) the
sidebar's first `tree__link` is `href="&#x2f;"` (`/`) and no `href="/home/"` appears; the
canonical of `/` is `http://localhost:8403/`.

### (d) The sitemap of a 1200-post WordPress import

`$S/gen_wxr.py` (kept in the scratch folder) writes a WXR 1.2 file with 1200 published posts
(one every 7 hours from 2020-01-01 09:00 UTC, two categories), two drafts, a private, a
pending and a future post, and three pages (`home`, `about`, `about/team`). Into a fresh
`init` site with one admin (`ferrite user create`):

```
$ ferrite import wordpress $S/wp1200.xml --data $S/w1 --report $S/w1-report   # exit 2, 7.0 s
  page: 3 created, …   post: 1205 created, …   redirects: 1201 created, … 5 skipped
  terms: 2 created   users: 1 created   options: 2 created
  5 redirects refused
```

Exit 2 is the documented "done with refused redirects": the report refuses the old paths of
the five hidden posts ("the item is not published (draft|private|pending|future)"). Served:

| Request | Answer |
|---|---|
| `/sitemap.xml` | `200`: `sitemap-page-1.xml` (`lastmod` 2020-01-01T09:00:00Z) and `sitemap-post-1.xml` (`lastmod` 2020-12-16T02:00:00Z) |
| `/sitemap-post-1.xml` | `200`, 111 709 bytes, 1200 `<loc>`s, all distinct |
| `/sitemap-post-2.xml`, `/sitemap-page-2.xml` | `404` |
| `/sitemap-page-1.xml` | `200`: `/about/team/`, `/about/`, `/home/` (no static front page is set on this site) |

The 1200 post URLs equal the 1200 targets of the import's date-path redirects (`redirects`
rows with a source under `/2…`), none names a hidden post, and each answers `200` (a curl
loop over all 1200); `/hidden-1/` and `/future-post/` answer `404`. The two `lastmod` values
equal `max(updated_at)` of the published posts (1 608 084 000 000 000 µs) and pages
(1 577 869 200 000 000 µs) in `ferrite.db`. The production file length is
`SITEMAP_FILE_LEN` = 50 000, so this site has one file per type; the split across files and
the seek are covered by the golden test and `sitemap_reads_seek` with a file length of 3
(§2).

### (e) Reproducible archives

`package.sh 0.1.0 aarch64-apple-darwin target/release/ferrite $S/repro1` and
`package-plugins.sh 0.1.0 $S/repro1`, then two seconds later the same from a fresh copy of
the binary under `umask 077` into `$S/repro2` (bsdtar 3.5.3, libarchive 3.7.4):

```
e5b7c6559c2c224b8909013dfc42a5ef11395a00004f008f9b92767abcc91efc  repro1/ferrite-0.1.0-aarch64-apple-darwin.tar.gz
38ca06b29df26c7af67ad2bce9c0ea9c7d8b4b41583366ac2e3cefd4db88dbc6  repro1/ferrite-plugins-0.1.0.tar.gz
e5b7c6559c2c224b8909013dfc42a5ef11395a00004f008f9b92767abcc91efc  repro2/ferrite-0.1.0-aarch64-apple-darwin.tar.gz
38ca06b29df26c7af67ad2bce9c0ea9c7d8b4b41583366ac2e3cefd4db88dbc6  repro2/ferrite-plugins-0.1.0.tar.gz
```

`tar -tvzf` lists the members sorted, `0 0` owners, 1970-01-01 dates, `drwxr-xr-x` folders,
`-rwxr-xr-x` `ferrite` and `-rw-r--r--` for the rest (3 and 12 members).
`.github/scripts/test-repro.sh` (the CI check) also passed here: the same sha256 twice for
both assets and names with a space, a quote and a newline packed as members.
`shellcheck -x .github/scripts/*.sh site/build.sh` and `actionlint` are clean.

## 2. Gates

At `a775396` for the Rust gates and both workspace runs (run 2 at `5a47c38`, which differs
only in docs); the admin gates and Playwright at `2c121bd` for the second Playwright run.
Logs in the scratch folder (`static-gates.log`, `test-run1.log`, `test-run2.log`,
`admin-*.log`, `e2e-all-run1.log`, `e2e-all-run2.log`, `e2e-fix.log`, `e2e-flake.log`).

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | ok |
| `cargo fmt --manifest-path plugins/Cargo.toml --all --check` | ok |
| `cargo clippy --workspace --all-targets -- -D warnings` | ok (exit 0) |
| `cargo clippy --manifest-path plugins/Cargo.toml --target wasm32-wasip2 -- -D warnings` | ok |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | ok |
| `cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml --locked` | ok |
| `cargo test --manifest-path plugins/Cargo.toml --workspace --locked` (host, P84) | ok: analytics 9, contact-form 17 passed, the other 34 crates have no tests |
| `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace --no-fail-fast`, run 1 (18:01–18:12, while §1's checks used the machine) | exit 0: 120 test binaries (doc-tests included), 3922 passed, 0 failed, 2 ignored (the `ferrite-sdk` doc examples `export_plugin` and `settings`), no `SKIP` line |
| The same, run 2 (18:12–18:20, nothing else running) | exit 0: 120 test binaries, 3922 passed, 0 failed, 2 ignored, no `SKIP` line |
| `pnpm -C admin lint`, `typecheck` | ok (also after `2c121bd`) |
| `pnpm -C admin test` | 130 files, 785 tests passed (also after `2c121bd`) |
| `pnpm -C admin build` | `check-dist: ok` |
| `pnpm -C admin bundle:check` | ok (css 30 408 of 33 792, entry 98 920 of 108 544, entry preloads 241 449 of 262 144, editor 200 227 of 218 112, chart 98 627 of 108 544, largest lazy 28 099 of 31 744) |
| `FERRITE_TEST_PG_URL=… pnpm -C admin e2e:all` (the five projects `chromium-sqlite`, `chromium-pg`, `firefox-sqlite`, `webkit-sqlite`, each with its `-site-wide` teardown, and `public-themes-chromium`; the release binary of §1), run 1 at `5a47c38` | 795 passed, 7 failed (9.5 min): `unreadable_parent_settles` in all four admin projects, `dashboard-metrics` `screens` in all but `chromium-sqlite` (§3) |
| After the fix, the two specs in the four admin projects (`e2e:all e2e/editor-publish.spec.ts e2e/dashboard-metrics.spec.ts`) | 44 passed |
| `e2e:all`, run 2 at `2c121bd` | 801 passed, 1 failed (9.4 min): `editor-followups` `failed_backup_restore_says_so_and_keeps_the_backup` in `chromium-pg` (§3); per project 196 (`chromium-sqlite`, `firefox-sqlite`, `webkit-sqlite`, the 18 site-wide specs included), 195 + 1 (`chromium-pg`), 18 (`public-themes-chromium`) |
| That test alone, `--project chromium-pg --repeat-each 10` | 10 passed |
| `site/build.sh` with the release binary | exit 0 (§1 c) |
| `shellcheck -x .github/scripts/*.sh site/build.sh`, `actionlint`, `.github/scripts/test-repro.sh` | clean, clean, ok (§1 e) |

## 3. Found during the exit

- **`unreadable_parent_settles` (E2E, every admin project) failed**: the spec made an
  author's child under the admin's draft parent through the API, which Task 4's
  `check_parent` change now refuses (`422 parent_id "unknown parent"`, the intended P77
  behaviour). Task 4 ran only `pnpm -C admin typecheck`, not this spec. Fixed in the spec
  (`2c121bd`): the parent is published while the author's child is made and unpublished
  afterwards (`ContentApi.transition` gains `unpublish`), which reaches the same state the
  UI must handle; every assertion is kept.
- **`dashboard-metrics` `screens` timed out in `chromium-pg`, `firefox-sqlite` and
  `webkit-sqlite`**: R-c3 (Task 6) added an axe check while the series request is held, and
  `expectAccessible`'s settle waits for network idle. In `chromium-sqlite` the earlier `snap`
  had waited for network idle before the hold (Playwright then answers at once); in the other
  projects `snap` is a no-op, so the wait never ended. Tasks 6 and 7 ran this spec in
  `chromium-sqlite` only. Fixed in the spec (`2c121bd`): it waits for network idle itself
  before the hold.
- **`failed_backup_restore_says_so_and_keeps_the_backup` failed once** (second full run,
  `chromium-pg` only): the toast "Could not restore the edits from this browser…" did not
  appear within 5 s, and the page's save state read "Couldn't save". It passed in the first
  full run in every project, in the other three projects of the second, and 10 of 10 alone.
  M4-4 did not change the spec or the editor's backup code; the cause is not established
  (a debounced autosave of the typed text meeting the spec's `500` route before the click is
  a guess). Listed under Later.
- During execution (the ledger): the controller's worktree cleanup removed Task 7's live
  worktree, which was clean at `m4-4`'s head and so looked merged; no work was lost, and
  Task 7 was dispatched again on its existing branch.

Docs written in this exit: the M4-4 execution rulings in [`rulings/m4.md`](rulings/m4.md);
the ROADMAP (M4-4 and M4 done, the new "Before v0.1.0" section, every other open item of the
M4-4 lists and of the M0/M1 lists under Later with its pointer); CLAUDE.md and README.md
(status); this file and [`m4-exit.md`](m4-exit.md).

## 4. Accepted limits

- **Links still take one hop:** listing cards, related items, breadcrumbs built from items,
  feeds, plugin `Post.url`, the API's `url` and redirect rows still name the front page's
  permalink (`/home/`) and reach `/` through the 301; only the tree, the sitemap and menus
  name it `/` (06 §3). The 301 drops every query parameter but `cpage` (the cache key).
- **The front page option in the page cache:** single-item pages and pages with a menu
  depend on `option:reading.front_page` (P67), so changing the front page flushes most of
  the page cache, and each content cache miss reads one more option (06 §3).
- **Colliding upload entries block each other:** in an uploads zip, entries whose keys
  collide only after the folders above `uploads/` are dropped (`wp-content/uploads/2024/a.png`
  beside `2024/a.png`) are ambiguous now, like NFC collisions, so none is read and each is
  reported as missing media; before M4-4 the first entry won. A zip with two copies of the
  uploads tree imports none of the duplicated files (08 §1; P79).
- **Accepting an unreadable relation:** `check_relations` accepts a relation to an item the
  writer cannot read (it counts as missing); a later save of that item by someone who can read
  the target is `422` at that path until the value is changed (07 §1).
- **Sitemap cost:** `file_bounds` still visits every published key of the type on each cache
  miss (no bodies, terms or ancestors); the covering index needs a migration (Later, 02).
- **P75:** a backup running past 03:00 UTC, or a node starting late in the day, can still
  meet a GC pass.
- **P70:** a stray `backups/rolled-back-<ts>.db-wal` with no tmp beside it is not refused on
  open; it is reached only by running a refusal's recovery steps out of order (Task 1
  re-review 2). `db copy --from sqlite:` does not run the leftover check (its source is
  opened read-only).
- **Palette `aria-busy`** covers the whole listbox, not only the content hits: a wrapper
  inside the listbox fails axe `aria-required-children`.
- **Tests:** `node.rs`'s fixed `sleep(reload_retry_wait())` can pass without the retry having
  run; an `in_one_window` retry is invisible in passing output; the hour-window store tests
  keep a 30 s margin with no retry; `cluster` `invalidation_latency_under_1s` failed once in
  Task 7's load loop (4.5 s against 1 s) while two cluster test binaries ran at once (CI runs
  binaries one at a time, and the bound is M1.5's requirement, so it was kept).
- The `job` span's new `node` field is in every job's log lines; `MdError::TooManyFiles`
  keeps its name though it also counts folders with pages.

## 5. Not verified

| Item | Why | Covered instead by |
|---|---|---|
| Any M4-4 code on Linux: the tests, builds and every exit run were on macOS (arm64) | macOS host; nothing pushed | CI's Linux jobs on the develop→main PR (ROADMAP "Before v0.1.0") |
| The reproducible archives on GNU tar 1.34 (ubuntu-22.04 runners) and on the macOS release runners | no such tar here; Task 8 checked GNU tar 1.35 in two local Docker images, this exit bsdtar 3.5.3 | `test-repro.sh` in each release build job, first run by the real release |
| The first CI run of the `plugin unit tests (host)` step (P84) | nothing pushed | the same command run here (§2) |
| `--restore-snapshot`'s refused `chgrp` path | needs a group the test user is not in | the pure `owner_plan` test |
| An interrupted `--restore-snapshot` killed for real (P70, R-c2) | not run by hand | `cli.rs` and `rollback.rs` tests that leave each interrupted state on disk, including the real WAL `init` leaves |
| P71 against a real release | a repository build has no release key | `self_update` tests with a `test-seams` binary |
| P72 on a running cluster | not run by hand | `rollback_schema_refuses_while_a_postgres_node_is_live` (Postgres, both refuse and allow) |
| `build_s3`'s environment options against a real proxy or provider | no provider here | the pure `s3_client_options` test |
| The 44 px touch targets on a real touch device; the live regions and `aria-busy` with VoiceOver or NVDA | emulated `pointer: coarse` and jsdom/axe only | Vitest, the `styleguide_touch_targets` E2E (`elementFromPoint` on all four corners), axe in the E2E specs, Task 6's 375 px touch screenshots |
| The cause of the one `failed_backup_restore_says_so_and_keeps_the_backup` failure (§3) | not reproduced: 10 of 10 alone | the other seven runs of it in this exit |
| The cause of the old `dashboard-metrics` 404s | not reproduced (Task 7) | the fix (`page.mouse.move(0, 0)`) and the E2E runs of §2 |
| The 301's `Cache-Control` in a browser after a front page change | not observed in a browser | the header (§1 c) and `the_front_pages_permalink_redirects_to_root` |
| `media gc` exit 3 by hand | not provoked | `media_gc_cli_busy_exit_3` (Postgres) |
| The sitemap split on a real site past 50 000 URLs | the 1200-post site has one file per type | the golden test and `sitemap_reads_seek` with a file length of 3, both backends; Task 3's review measured `file_bounds` on Postgres with 257 000 rows (199 → 139 ms after the `GROUP BY` fix, as its commit message records) |

## 6. For the user

- **The develop→main pull request** is your decision; the first CI run on it is the first
  Linux run of everything since M2b-3 (ROADMAP "Before v0.1.0").
- **The release gate** is unchanged from [`m4-2-exit.md`](m4-2-exit.md) §6 (with the plugins
  asset as the seventh signed asset, `m4-3a-exit.md` §6). The first real release run also
  proves the reproducible archives on GNU tar 1.34 and the macOS runners.
- **Behaviour changes to know before you upgrade a site:**
  - `ferrite restore` can exit 2 (restored, but the final check failed) and `ferrite media
    gc` exits 2 for a refused store or a failed delete: a cron wrapper that treats any
    non-zero exit as "nothing happened" should read the message.
  - `self-update --data` now refuses an update whose new binary rejects the configuration;
    run it with the service's environment, or pass `--skip-config-check`.
  - The daily `artifacts.gc` runs at 03:00 UTC (it was 00:00 with the backup).
  - The front page's permalink (`/home/`) now answers `301` to `/`; browsers keep it for up
    to an hour after the front page changes.
  - Changing the front page flushes most of the page cache (single items and pages with a
    menu depend on the option), and a zip with two copies of the uploads tree now imports
    none of the duplicated files (§4).
  - Authors and contributors can no longer pick another user's draft, pending, private or
    scheduled page as a parent (existing parents stay).
  - A blank `q` lists everything on `GET /users` (it was `422`).
- **Hosting the docs site** and the `docs` theme's lack of syntax highlighting are still
  yours to decide ([`m4-3b-exit.md`](m4-3b-exit.md) §6).

## 7. Open follow-ups

The ROADMAP's M4-4 section now lists only what was done. What needs you or a first real run
is under "Before v0.1.0"; every other open item of the old lists, and the ones this milestone
found (the `xs`/`sm` text buttons under 44 px on touch, about 65 call sites; the sitemap's
covering index; no live-node check for a Postgres `migrate down`; the untested refused
`chgrp`; the test notes of §4 and the one E2E failure of §3), is under "Later", "Follow-ups moved here from M4-4", grouped
by origin with its pointer. The open items of the M0 and M1 lists were moved there too.

## 8. Verified claims

| Claim (where) | Source | Result |
|---|---|---|
| 54 non-merge commits, 222 files, +8 435 / −1 838 lines (What shipped) | `git log --no-merges --oneline b3aba623..a7753968 \| wc -l`; `git diff --shortstat b3aba623 a7753968` | verified |
| No migration and no WIT change (What shipped) | `git diff --stat b3aba623 a7753968 -- crates/ferrite-store-sqlite/migrations crates/ferrite-store-postgres/migrations wit` is empty | verified |
| No new external dependency; `ferrite-store-tests` gains `futures` (What shipped) | `git diff b3aba623 a7753968 -- Cargo.lock` (one line, `futures` under `ferrite-store-tests`) and `-- 'crates/*/Cargo.toml'` (one line) | verified |
| OpenAPI: two description strings (What shipped) | `git diff --stat b3aba623 a7753968 -- crates/ferrite-http/openapi.json`: 2 insertions, 2 deletions | verified |
| Nine loading states use the `stale` prop (What shipped, R-c3) | `grep -rn 'stale={' admin/src`: eight `stale={….isPlaceholderData}` sites, plus the media library, which passes `list.isPlaceholderData` (`library.tsx:203`) to its grid and its table | verified |
| `artifacts.gc` at 03:00 UTC (What shipped, §6) | `ARTIFACTS_GC_OFFSET = 3 * 3_600` s in `crates/ferrite/src/jobs/gc.rs:22`; `artifacts_gc_runs_at_three` (§2) | verified |
| The Postgres lease check uses `now()` (What shipped) | `crates/ferrite-store-postgres/src/repos/import.rs:143`; SQLite binds `now` (`…/sqlite/src/repos/import.rs:141`) | verified |
| P82 covers argon2 and blake2 in `profile.dev` and `profile.test` (What shipped) | root `Cargo.toml:119-130` | verified |
| §1's outputs | the commands' output in this session (the scratch folder keeps `a1r.out`, `a1x.out`, `a1f.out`, `w1-import.out`, `docs-build.out`, `w1-report/`) | verified |
| The `lastmod` values equal `max(updated_at)` (§1 d) | `sqlite3` on `w1/ferrite.db`; 1 608 084 000 s = 2020-12-16T02:00:00Z, 1 577 869 200 s = 2020-01-01T09:00:00Z | verified |
| Task 3's Postgres timing (§5) | commit `b9667b0`'s message; not re-measured here | taken from the commit, not re-run |
| Task 8's GNU tar 1.35 runs (§5) | Task 8's report; not re-run here | taken from the report, not re-run |
| Task 7's 20× loops under load (§4) | Task 7's report; not re-run here | taken from the report, not re-run |
