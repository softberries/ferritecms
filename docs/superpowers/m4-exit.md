# M4 exit: ops and release

**Criterion** ([ROADMAP](../ROADMAP.md), M4): a site can be moved, backed up, restored and
cleaned up on both profiles; FerriteCMS can be released and operated (signed self-update and
rollback, built-in HTTPS for solo sites, metrics, release packaging, a systemd unit, the
runbooks); it ships example plugins, starter themes, a Markdown import, a docs site built with
itself and `ferrite doctor`; and the follow-up lists carried into M4 are closed or placed.
M4 was done in four parts, each with its own exit.

**Result: done** (2026-10-31, branch `m4-4` at `a775396` plus the exit's docs commits).
Nothing is released, tagged, published or hosted: the first release waits for the
ROADMAP's "Before v0.1.0" list, which starts with the release gate and the develop→main pull
request (both the user's).

| Part | What it delivered | Exit |
|---|---|---|
| M4-1 data safety (2026-10-29; plan rulings P1–P24, the "M4-1 execution rulings") | `export` / `import-site` / `db copy`, `backup` / `restore` and the scheduled `[backup]` (`site.backup`), `media sync` / `gc` / `regenerate` and the daily `artifacts.gc`, the S3 options, location-metadata stripping, the Postgres `lock_wait`, an atomic `seed demo`, the cluster example on RustFS; migration `0011_artifact_gc` | [`m4-1-exit.md`](m4-1-exit.md): every command end to end, a SQLite → Postgres move, a backup taken while `serve` ran, the cluster example; workspace 3 244 passed, 0 failed |
| M4-2 release (2026-10-30; P25–P40, the "M4-2 execution rulings") | signed `self-update` and `rollback` (`--schema`, `--restore-snapshot`) with `upgrade.json`, ACME TLS for solo sites, the metrics registry, `/metrics`, `node_metrics` and the dashboard series, `site.timezone`, a ring-only build, the release workflow (six targets, minisign, GHCR), the systemd unit, the `compat` job and the runbooks; migration `0012_node_metrics` | [`m4-2-exit.md`](m4-2-exit.md): the two-binary lifecycle against a local signed release, ACME against Pebble, a metrics scrape; workspace 3 391 passed, 0 failed |
| M4-3 examples and docs (2026-10-30/31; P41–P65, R-b1–R-b17 and the "M4-3a execution rulings") | the example plugins `seo`, `analytics`, `contact-form` and their release asset, `ferrite doctor`; heading ids, `post.toc`, `page_tree()`, `query(order="menu")`, the `docs` and `business` themes, `seed demo --profile business`, `ferrite import markdown`, the docs site in `site/` and the axe checks | [`m4-3-exit.md`](m4-3-exit.md) (summary), [`m4-3a-exit.md`](m4-3a-exit.md), [`m4-3b-exit.md`](m4-3b-exit.md) |
| M4-4 follow-ups (2026-10-31; P66–P84, R-c1–R-c3) | the data-safety fixes of `restore`, `rollback`, `self-update` and `media gc`, the front page's 301 and the sitemap reads, the permission and API consistency fixes, the import limits, the admin's touch, contrast and live-region fixes, the test flakes, reproducible release archives; the ROADMAP's follow-up lists closed or moved | [`m4-4-exit.md`](m4-4-exit.md) |

Rulings: [`rulings/m4.md`](rulings/m4.md) (each part's plan and execution rulings). M4 added
migrations `0011` and `0012` (the schema is at 12; migrations 0001–0012 freeze at the
first tag, `v0.1.0`) and no WIT change (the world stays `0.1.0`).

What each part did not verify is in its exit's "Not verified" section. Common to all of M4:
every check ran on a macOS (arm64) host, with local Postgres 17 and, for the cluster example
and the image, Linux containers in Docker; nothing ran on CI (no push), against a real S3 provider, a real SMTP
provider, Let's Encrypt, or a published release.

**For the user** (details in each exit): the release gate of [`m4-2-exit.md`](m4-2-exit.md)
§6 (with the plugins asset as the seventh signed asset, M4-3a §6); the develop→main pull
request and its first CI run; hosting the docs site and the `docs` theme's syntax
highlighting (M4-3b §6); the M4-4 behaviour changes listed in [`m4-4-exit.md`](m4-4-exit.md)
§6. The ROADMAP's "Before v0.1.0" section lists these with the first-real-run checks; the
other follow-ups are under "Later".
