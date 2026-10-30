# FerriteCMS

A WordPress-style CMS shipped as **one Rust binary**, where every plugin runs in a
**WebAssembly sandbox** and can only do what it declared.

```
$ ferrite serve --data ./site
$ ferrite import wordpress export.xml --uploads ./wp-content/uploads --data ./site
```

- **Single binary, single folder** — web server, admin, themes, plugin runtime and SQLite
  compiled in (Postgres + S3 optional for multi-node). Back up or move a site by copying one folder.
- **Sandboxed plugins** — WASM components with a permission manifest, memory/CPU/time
  limits per call. A broken plugin can't take the site down or read your users table.
- **Familiar to WordPress developers** — hooks with priorities, template hierarchy,
  post types, roles.
- **Moves your WordPress content** — posts, pages, media, taxonomies, users and 301
  redirects from a WXR export.

> Status: in development, not released (no tag yet). The server, the REST API, the admin,
> the WordPress importer, backups and restores, signed self-updates with rollback,
> built-in HTTPS (ACME) for solo sites, Prometheus metrics, the release packaging, three
> example plugins (SEO, analytics, a contact form; [`plugins/examples/`](plugins/examples/))
> and `ferrite doctor` are built. See [`docs/BRIEF.md`](docs/BRIEF.md) and [`docs/ROADMAP.md`](docs/ROADMAP.md).

Running a site: [`packaging/systemd/README.md`](packaging/systemd/README.md) (install
under systemd), [`docs/operations/solo-upgrade.md`](docs/operations/solo-upgrade.md)
(update, roll back, HTTPS) and
[`docs/operations/cluster-runbook.md`](docs/operations/cluster-runbook.md) (rolling
deploys of a Postgres + S3 cluster); the design is in
[`docs/design/09-operations.md`](docs/design/09-operations.md).

The admin is a React app in `admin/` that the binary embeds: build it with
`pnpm -C admin install && pnpm -C admin build` before `cargo build` (without it `/admin/`
shows a placeholder page; see [`admin/README.md`](admin/README.md)).
