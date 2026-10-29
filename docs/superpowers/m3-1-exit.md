# M3-1 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M3; plan
[`2026-10-28-m3-1-wordpress-import-core.md`](plans/2026-10-28-m3-1-wordpress-import-core.md),
Task 18): *the import core, the conversion and the CLI are built; `ferrite import wordpress`
imports the checked-in exports A and B and WordPress's two theme-test files into a fresh
site, every imported body opens in the editor, and a re-run updates instead of duplicating;
the docs, rulings and roadmap describe what was built; the gates pass: `fmt`, `clippy`,
the workspace tests on both backends, the admin gates and the editor E2E specs on
Chromium.*

**Result (2026-10-29, branch `claude/cool-khayyam-a9355a` at `c68e644` = `m3-import` at
`b33ecd8` plus one E2E fix and this docs commit):** every gate passed. The workspace tests
passed on SQLite and on PostgreSQL 17 (2 868 passed, 0 failed, 1 ignored, each run). The
theme-test corpus imported with the same counts Task 17 recorded, and all 141 imported
items are editor-safe. A release-build import of export A into a fresh site, its re-run and
a `serve` of the result behaved as designed (§2). One gate found a stale test: the editor
E2E `seo_fields_reach_the_head` still expected the SEO object without `canonical` and
`robots`, which Task 5 added. The spec was fixed in `c68e644`; no product bug was found.

| Exit check | Verified by | Result |
|---|---|---|
| 1. `fmt`, `clippy` (with and without `test-seams`), rustdoc | §3 | yes |
| 2. Workspace tests, SQLite and PostgreSQL | §3 | yes: 2 868 passed, 0 failed, 1 ignored (a `no_run` doc example), each run |
| 3. Theme-test corpus (`corpus_real`) on the final code | §1 | yes: counts identical to Task 17's; 79 + 62 items editor-safe |
| 4. Release CLI import, re-run, `serve` | §2 | yes |
| 5. Admin gates (`lint`, `typecheck`, `test`, `api:check`, `build`, `bundle:check`) | §3 | yes: 677 Vitest tests in 116 files |
| 6. Editor E2E specs on Chromium | §3 | yes after `c68e644`: 39 of 39 on `chromium-sqlite`, then 78 of 78 on `chromium-sqlite` + `chromium-pg` |
| 7. The import job, API and admin screen | — | **not built** (M3-2) |
| 8. Docker cluster example with an import | — | **not run** (M3-2 exit) |

What was built is listed in [ROADMAP](../ROADMAP.md) (M3, the ticked items) and described
as built in design [08](../design/08-wp-import.md); the rulings are P1–P22 and R1–R51 in
[`rulings/m3.md`](rulings/m3.md). In short:

- `ferrite-import-wp`: a streaming WXR reader (quick-xml `=0.41.0`; no DOCTYPE, 64 MiB text
  cap, UTF-8 checked); the IR (a temp SQLite file per run, resumable by the WXR's SHA-256);
  the Gutenberg, classic and shortcode conversion to the block tree (`scraper` 0.29 on
  `html5ever =0.40.1`, with the depth and marker guards and the 48-level cap); pass 1
  (users, types, taxonomies, ACF fields, terms), pass 2 (media), pass 3 (permalink
  structure, content with SEO, ACF and meta), pass 4 (comments, menus, redirects, site
  options), the link fix-up, and the JSON and HTML report.
- `ferrite-core::import`: `ImportService` (run start with `Capability::Import`, the fence,
  takeover) and `ImportBatch` (one transaction, one event, P4 through `import_map`).
- Storage: migration `0009_import` (`import_runs`, `import_map`) on both backends with
  contract tests; `ContentRepo::set_updated_at`.
- The block tree gained `embed` (YouTube, Vimeo; privacy-mode iframes) and an image
  `caption`, rendered by core and editable in the admin; SEO gained `canonical` and
  `robots` (head, REST DTOs, OpenAPI snapshot).
- `ferrite-http::import_fetch::HttpSource` for `--fetch-media` (SSRF address rule, 30 s per
  file, no `https` → `http` redirect); `ferrite import wordpress` in `crates/ferrite`.

Docs changed: this file; `rulings/m3.md` (P3, P15 and P21 as built; execution rulings
R1–R51); ROADMAP (M3-1 ticked, the rest under "M3-2", the M1a fallback-author item);
CLAUDE.md (status, crate map, how to import); design 08 (as built throughout, M3-2 parts
marked), 07 §1 and §3 (the Import rows), 02 §3, §4 and §7 (`0009_import`), 09 §8 (the
command). 01 §1 already had the crate and its edges (Task 17).

## 1. Theme-test corpus

`FERRITE_WP_CORPUS=/Users/krzysztofgrajek/Projects/rust/ferritecms/.superpowers/m3-corpus
cargo test -p ferrite-import-wp --test corpus_real -- --nocapture` (each file into a fresh
SQLite site, no uploads folder, so every attachment is reported missing):

| | `themeunittestdata.wordpress.xml` | `64-block-test-data.xml` |
|---|---|---|
| posts | 58 created | 61 created |
| pages | 21 created | 1 created |
| comments | 29 created, 4 skipped (pingbacks, an unknown status or an item not imported; the test prints no reasons) | 2 created |
| terms | 178 created, 1 unchanged, 6 skipped | 11 created, 11 unchanged, 2 skipped |
| menus | 6 created | — |
| redirects | 65 created, 1 refused | 4 created |
| users | 1 created, 1 unchanged | 1 created, 1 unchanged |
| options | 2 created | 2 created |
| media | 37 skipped (missing) | 9 skipped (missing) |
| unknown blocks / shortcodes | 30 / 2 | 29 / 0 |
| editor-safe items checked | 79 | 62 |

Dropped formatting, file 1: `mark:link` 64, `mark:alignment` 54, `mark:style` 36,
`images not imported` 8, `page breaks` 6, `mark:code` 6, `mark:underline` 6, `h1 demoted`
5, `mark:sup` 4, `tag:video` 4, `mark:sub` 3. File 2: `mark:style` 240, `mark:alignment` 93,
`h1 demoted` 6, `mark:highlight` 3, `images not imported` 2, `mark:sup` 2, `mark:link` 1,
`page breaks` 1, `tag:video` 1. Task 17 recorded the same post, page, comment, term, menu,
redirect, missing-media and unknown-block counts (progress log, Task 17).

## 2. Release CLI import

macOS 26.6 arm64, `rustc` 1.99.1 (`rustup update stable`: unchanged), Node 24.18.0, pnpm
10.28.2, PostgreSQL 17.11 (Homebrew). Session scratchpad paths shortened to `$SP`.

```
$ cargo build --release -p ferrite                      # Finished `release` profile in 2m 21s
$ ferrite init --data $SP/site --url http://127.0.0.1:18081
$ ferrite user create --data $SP/site --login importer --email importer@example.test
$ ferrite import wordpress crates/ferrite-import-wp/tests/corpus/site-a.xml \
    --data $SP/site --uploads crates/ferrite-import-wp/tests/corpus/site-a-uploads
  exit 0; created: book 2, comments 4, media 4, menus 1, options 3, page 4, post 8,
  redirects 7, taxonomies 1, terms 9 (1 skipped), types 1, users 4; fixups 2 updated;
  "media variants are made by the next `ferrite serve`"; report.json + report.html under
  <data>/import/reports/<run-id>/; permalink_structure "/%year%/%month%/%slug%/"
$ (the same command again)
  exit 0; every kind unchanged (book 2, comments 4, media 4, menus 1, options 3, page 4,
  post 8, redirects 7, taxonomies 1, terms 9, types 1, users 4); <data>/import/ holds only
  reports/
$ FERRITE_SERVER__LISTEN=127.0.0.1:18081 ferrite serve --data $SP/site
  four media.variants jobs ran ("media variants recorded … variants=4", 4, 4, 3)
$ curl /2024/03/every-core-block/   200; <title>Every core block - Corpus Site</title>;
  iframe src="https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"; <figcaption>The hero,
  medium size; images under /_media/; href="/about/" and href="/about/team/" (fixed links)
$ curl /about/team/                 200
$ curl /hero/                       301 → /_media/<hash>/hero.png (attachment page)
$ curl /tag/rust/                   200
```

`/?p=11` answered 200 (the home page): `?p=N` links are rewritten in bodies (P14), not
redirected.

## 3. Gates

Run on the final code (`b33ecd8`, then `c68e644` for the E2E re-runs):

```
$ cargo fmt --check                                                       # exit 0
$ cargo clippy --workspace --all-targets -- -D warnings                   # exit 0
$ cargo clippy --workspace --all-targets \
    --features ferrite/test-seams,ferrite-http/test-seams,ferrite-import-wp/test-seams \
    -- -D warnings                                                        # exit 0
$ RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps              # exit 0
$ cargo test --workspace                                                  # 2868 passed; 0 failed; 1 ignored
$ FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test \
    cargo test --workspace                                                # 2868 passed; 0 failed; 1 ignored
$ FERRITE_WP_CORPUS=… cargo test -p ferrite-import-wp --test corpus_real  # 1 passed (§1)
$ pnpm -C admin install --frozen-lockfile
$ pnpm -C admin lint                                                      # exit 0
$ pnpm -C admin typecheck                                                 # exit 0
$ pnpm -C admin test                                                      # Test Files 116 passed (116); Tests 677 passed (677)
$ pnpm -C admin api:check                                                 # src/api/schema.d.ts is current
$ pnpm -C admin build                                                     # check-dist: ok
$ pnpm -C admin bundle:check                                              # bundle:check: ok
$ cargo build --release -p ferrite
$ cargo build --manifest-path plugins/Cargo.toml --target wasm32-wasip2 --release -p admin-e2e
$ pnpm -C admin e2e e2e/editor.spec.ts e2e/editor-fields.spec.ts e2e/editor-followups.spec.ts \
    e2e/editor-media.spec.ts e2e/editor-publish.spec.ts e2e/editor-safety.spec.ts
    # at b33ecd8: 1 failed (seo_fields_reach_the_head), 38 passed
    # at c68e644: 39 passed (1.8m)
$ FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test pnpm -C admin e2e <same files>
    # at c68e644: 78 passed (2.1m), chromium-sqlite and chromium-pg
```

The SQLite-only workspace run passes its Postgres cases by skipping them (`SKIP`, captured);
the second run executes them. `CI` was not set, so a missing `FERRITE_TEST_PG_URL` would not
have panicked. The ignored test is `ferrite-sdk`'s `export_plugin` doc example.

## 4. Accepted limits (rulings)

- **Terms are not updated on a re-run** (C6, R46): terms are matched or reused; a term
  renamed in WordPress keeps its old name here, and local taxonomy edits win.
- **A scheduled import published by its schedule counts as kept** (A2, R47): publishing
  bumps the item's version and lifecycle transitions write no revision, so the importer
  cannot tell it from an edit without storing the imported status in the map. Such items
  are not updated from WordPress; the report notes this whenever items were kept, and
  design 08 §6 says so.
- **Bodies nested past 48 levels are flattened** (B2, R48): a blockquote, list item or cell
  at that depth keeps its text, with its marks, in one paragraph, counted as `too deep`;
  images, embeds and HTML below it are dropped. `serde_json` refuses JSON nested past 128
  levels, so a deeper body could not be read back from the store.
- **`ImportHandle` has `run()` and no `actor()` getter** (A4, R51): nothing outside
  `ferrite_core::import` reads the actor.
- Comments are not updated on a re-run (R15); WP attachment parents are not imported (R28);
  the link fix-up writes no revision (R14); a resumed pass 3 can cause one spurious
  `updated` on the next re-run (R50).

## 5. Not verified

- **The import job, API and admin screen** (M3-2): `POST /api/v1/import/wordpress` with
  `Idempotency-Key`, `GET` on runs and the report download, the job's timeout and lock, the
  admin screen and the admin fields for `seo.canonical` and `seo.robots`. Only the CLI
  exists.
- **Redirects for `permalink.*` structure changes** (M2a-2b P26, M3-2): changing the
  structure after an import still moves every published URL without a redirect.
- **A partial report for a failed run** (M3-2): a failed CLI run prints the error, exits 1
  and writes no report.
- **The Docker cluster example with an import** (`examples/cluster/`, M3-2 exit): not run;
  the importer ran only on solo SQLite sites from the CLI here, and on Postgres only
  through the test suites.
- **Media variants are made only by the next `serve`** (P13): an import alone leaves media
  without variants (seen in §2: the jobs ran when `serve` started). Not measured on a large
  site.
- **The Postgres job-claim `EXPLAIN`** with ~50 000 future `content.publish_scheduled` rows
  (M1.5 R17, M3-2).
- **`--fetch-media` against a real remote site**: tested only against a local server
  through the `test-seams` seam and the address-rule unit tests.
- **Real production exports**: only exports A and B (from a scratch WordPress 7.1.3) and
  the two theme-test files were imported; no anonymised production site, no export over a
  few MB.
- **The E2E specs other than the editor's, Firefox and WebKit**: not run in M3-1 (the brief
  asks for the editor specs on Chromium).
- **CI on GitHub**: nothing pushed.

## 6. Verified claims

Every claim in this file and in the M3-1 doc changes, with how it was checked.

| Claim | Where | Checked by |
|---|---|---|
| Workspace tests: 2 868 passed, 0 failed, 1 ignored on each backend | §3 | the two `cargo test --workspace` logs (summed `test result` lines) |
| The ignored test is `ferrite-sdk`'s `export_plugin` doc example | §3 | `grep ignored` in the log |
| Admin: 116 files, 677 tests | §3 | `pnpm -C admin test` output |
| Editor E2E: 38 passed + 1 failed, then 39, then 78 | §3 | the three `pnpm e2e` runs |
| The failing E2E expected SEO without `canonical`/`robots` | Result | the Playwright diff (`+ "canonical": null`, `+ "robots": []`) |
| Corpus counts equal Task 17's | §1 | `corpus_real` output vs progress.md (Task 17 line) |
| 79 + 62 items editor-safe | §1 | `corpus_real` output |
| Re-run of A: every kind unchanged, no IR folder left | §2 | the second CLI run's output; `ls <data>/import` |
| Inferred structure `/%year%/%month%/%slug%/` | §2 | `report.json` `permalink_structure` |
| Attachment page → media URL (301); embed iframe; fixed `/about/` links | §2 | `curl` against `serve` |
| Variants made by the next `serve` | §2, §5 | `serve` log lines `media variants recorded` |
| `?p=N` is not a redirect | §2 | `curl /?p=11` → 200 |
| CLI flags and exit codes (0, 2, 3, 1) | CLAUDE.md, 08 §1, 09 §8 | `crates/ferrite/src/import_cmd.rs` (`WordpressArgs`, `DONE`, `DONE_WITH_PROBLEMS`, `BUSY`) |
| Default `--as` is the first active admin; only active admins import | 09 §8 | `import_cmd.rs` `importer` |
| Batch of 500, `[all]` past 1 000 keys | 08 §2 | `ImportBatch::BATCH_ITEMS`, `MAX_EVENT_KEYS` in `crates/ferrite-core/src/import/batch.rs` |
| Parse queue of 8 | 08 §2 | `PARSE_QUEUE` in `run.rs` |
| IR tables | 08 §2 | `SCHEMA` in `crates/ferrite-import-wp/src/ir.rs` |
| Dry-run IR marked and deleted on every exit; `find_ir` takes the most passes | 08 §2, R49 | `run.rs` (`DRY_RUN`, `remove_work_dir`, `find_ir`) and its tests |
| `import_runs`/`import_map` columns, indexes, kinds, `ON DELETE CASCADE`, lossy down | 02 §4, R5, R40 | `0009_import.up.sql`/`.down.sql`, both backends |
| Hash covers what is written | P3, 08 §2 | `content_hash` in `pass/content.rs`; menus and options passes |
| Terms and comments matched, not updated | R15, R46 | `import/comments.rs` (mapped → returned unchanged); C6 ruling |
| Placeholder `wp<id>.<login>@users.invalid` | R26, 08 §3 | `pass/people.rs` and its test |
| Depth guard 512, marker cap 1 024 (`object`, `applet`, `marquee`) | R31–R34, 08 §4 | `convert/depth.rs` constants and docs |
| Node cap 48 | R48 | `convert::doc::MAX_DEPTH`; `deep_nesting_loads_back`, `pass3_deep_body_loads_back` |
| Report caps 1 000 / 500 / 100 / 200, top 50 meta keys | R42, 08 §6 | constants in `report.rs` |
| Block and shortcode mappings | 08 §4 | the `match` in `convert/blocks.rs`; the shortcode names in `convert/shortcodes.rs` |
| Pass order (pass 4: comments, menus, redirects, options; then fixup) | 08 §2 | `run.rs` `steps`, `pass/mod.rs` `pass4` |
| Unknown `seo.robots` value → 400 | R19 | `crates/ferrite-http/tests/api/content_seo.rs` |
| Uploads of the corpus: 128 408 bytes (188 KiB on disk) | P15, 08 §7 | `stat` sum and `du -sk` of both upload folders |
| Corpus A: Gutenberg, Yoast, ACF; B: classic, Rank Math; WordPress 7.1.3 | 08 §7, ROADMAP | `tests/corpus/README.md`; `generator` lines of both XML files |
| `scraper 0.29`, `html5ever =0.40.1`, `quick-xml =0.41.0` | R24, this file | workspace `Cargo.toml` |
| `ImportService::new(store, permalinks)` | R6 | `crates/ferrite-core/src/import/mod.rs` |
| `set_body` removed; fix-up via `set_body_and_fields`, no revision | R14 | `import/content.rs` |
| `ContentService::update` sends one event | R13 | `content.rs`; `update_emits_moved_descendants_and_redirects` |
| Fallback author = importing admin | ROADMAP, 02 §3 | `pass/content.rs` (`cx.admin`) |
| `ferrite-http` and `ferrite` depend on `ferrite-import-wp` | CLAUDE.md | both `Cargo.toml` files |
| `import:write` scope, `import` area | 07 §1 | `crates/ferrite-core/src/actor.rs` |
| Toolchain and versions | §2 | `rustc --version`, `node --version`, `pnpm --version`, `select version()`, `sw_vers` |
