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

> Status: in development, not released. The server, the REST API and the admin (sign-in,
> dashboard, content list, editor, media library, content types, taxonomies, menus,
> comments, redirects, account, users, settings, themes, plugins, system) are built; the
> WordPress importer shown above is not yet built (M3). See [`docs/BRIEF.md`](docs/BRIEF.md) and
> [`docs/ROADMAP.md`](docs/ROADMAP.md).

The admin is a React app in `admin/` that the binary embeds: build it with
`pnpm -C admin install && pnpm -C admin build` before `cargo build` (without it `/admin/`
shows a placeholder page; see [`admin/README.md`](admin/README.md)).
