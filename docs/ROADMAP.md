# FerriteCMS — Roadmap

Rough target: credible MVP in ~3 months of focused work. Each milestone ends with
something runnable and demoable.

## M0 — Plugin host spike (de-risk first, ~1–2 weeks)

The riskiest part. Prove it before building a CMS around it.

- [ ] Cargo workspace with `ferrite-plugin-host`, `ferrite-sdk`, `wit/`.
- [ ] Minimal WIT world: one filter hook `render-head(head) -> head`, one import
      `content.get-post(id)`.
- [ ] wasmtime host loading a component, linking imports **only** if declared in manifest.
- [ ] Limits: memory cap, fuel, epoch deadline. Test: infinite loop plugin is killed,
      page still renders.
- [ ] Test: plugin without `http` capability cannot even link an HTTP import.
- [ ] Hello-world plugin in Rust via `ferrite-sdk`.
- [ ] Benchmark: per-call overhead of a filter hook (instantiate vs. pooled instances).

**Exit criterion:** a filter plugin runs in < 1 ms per call warm, and hostile plugins
(loop, OOM, panic, undeclared import) are contained.

## M1 — Core content + serving (~3 weeks)

- [ ] SQLite schema + migrations: content items, revisions, taxonomies, users, options,
      redirects, plugin KV.
- [ ] `ferrite serve --data ./site` with axum.
- [ ] Router: permalinks, redirects table, 404.
- [ ] Theme engine: minijinja + template hierarchy resolution + safe helpers.
- [ ] One starter theme (blog).
- [ ] In-memory page cache with dependency-based invalidation.
- [ ] Wire `render.head` + `content.body` filters into render pipeline.

## M2 — Admin + API (~4 weeks)

- [ ] REST API v1 (content CRUD, media upload, taxonomies, users, settings).
- [ ] Auth: argon2 passwords, sessions, roles; passkeys stretch.
- [ ] Admin SPA embedded in binary: posts/pages list + editor (adopt TipTap/ProseMirror),
      media library, settings, plugins page showing declared permissions.
- [ ] Custom content types defined in admin.

## M3 — WordPress import (~2 weeks)

- [ ] WXR parser (`quick-xml`) → posts, pages, media, terms, authors, comments, meta.
- [ ] Media copy + URL rewrite.
- [ ] 301 redirects for all old permalinks.
- [ ] Yoast/Rank Math meta → SEO fields.
- [ ] `import-report.html` listing unknown shortcodes/blocks.
- [ ] Test against 2–3 real WP exports of different sizes.

## M4 — Ops + release (~1–2 weeks)

- [ ] `ferrite backup` (SQLite online backup + media), `self-update`, `rollback`.
- [ ] ACME TLS.
- [ ] `/metrics`, per-plugin hook timings in admin.
- [ ] Static release builds, Docker image, systemd unit.
- [ ] Docs site (built with FerriteCMS itself).
- [ ] 3 example plugins: SEO, privacy-friendly analytics, contact form.

## Later

- Postgres backend, multi-site.
- Plugin routes + declarative admin settings UI.
- Signed plugin registry with verified publishers.
- JS/Go/Python plugin SDKs (after checking Component Model toolchain maturity).
- Built-in forms, i18n/multilingual content, full-text search UI.
- Managed hosting offering.
