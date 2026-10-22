# 07 — REST API, Auth & Admin UI

## 1. REST API v1

Base: `/api/v1`. JSON, UTF-8. Errors: RFC 9457 `application/problem+json`
(`type`, `title`, `status`, `detail`, `errors[]` for validation). The admin SPA uses
exactly this API; there are no private admin endpoints (headless use gets the same
surface).

### Conventions

- Ids: UUID strings. Timestamps: RFC 3339 UTC.
- Pagination: cursor-based `?limit=50&cursor=<opaque>` → `{ items, next_cursor }`;
  `limit ≤ 200`.
- Filtering: `?status=draft&type=post&term=news&author=<id>&q=<search>`.
- Optimistic concurrency: resources carry `version`; `PUT/PATCH` require
  `If-Match: "<version>"` → `412` on conflict with the current resource in the body.
- Idempotency: `POST` accepts `Idempotency-Key` header (stored 24 h) for imports/uploads.
- OpenAPI 3.1 generated from code with `utoipa` (6.x) at `/api/v1/openapi.json`; the
  admin's TypeScript client is generated from it (`openapi-typescript` +
  `openapi-fetch`), checked in CI for drift.

### Resources (M2)

| Resource | Endpoints |
|---|---|
| Auth | `POST /auth/login`, `POST /auth/logout`, `GET /auth/me`, `POST /auth/password-reset`, `POST /auth/password-reset/confirm`, `POST /auth/totp/*` |
| Content | `GET/POST /content/{type}`, `GET/PATCH/DELETE /content/{type}/{id}`, `POST /content/{type}/{id}/publish`, `…/unpublish`, `…/schedule`, `…/restore` (from trash) |
| Revisions | `GET /content/{type}/{id}/revisions`, `GET …/revisions/{rid}`, `POST …/revisions/{rid}/restore` |
| Content types | `GET/POST /types`, `GET/PATCH/DELETE /types/{name}` |
| Taxonomies & terms | `GET/POST /taxonomies`, `GET/POST /taxonomies/{tax}/terms`, `PATCH/DELETE /terms/{id}` |
| Media | `POST /media` (multipart, streaming), `GET /media`, `GET/PATCH/DELETE /media/{id}` |
| Menus | `GET/PUT /menus/{name}` (whole tree replace) |
| Users | `GET/POST /users`, `GET/PATCH/DELETE /users/{id}`, `GET/POST/DELETE /users/{id}/tokens` |
| Comments | `GET /comments`, `PATCH /comments/{id}` (moderate), public `POST /content/{type}/{id}/comments` (rate-limited, honeypot) |
| Redirects | `GET/POST /redirects`, `PATCH/DELETE /redirects/{id}`, `POST /redirects/import` (CSV) |
| Settings | `GET/PATCH /settings` (options allowlisted for API) |
| Themes | `GET /themes`, `POST /themes` (upload), `POST /themes/{name}/activate`, `GET/PATCH /themes/{name}/settings` |
| Plugins | `GET /plugins`, `POST /plugins/inspect` (upload → permission report, nothing installed), `POST /plugins` (install with approved grants), `POST /plugins/{name}/enable|disable`, `DELETE /plugins/{name}`, `GET /plugins/{name}/metrics` |
| Import | `POST /import/wordpress` (upload WXR + optional uploads zip → job), `GET /import/{job}` |
| System | `GET /system/status` (versions, schema, nodes, backends), `GET /system/jobs`, `POST /system/jobs/{id}/retry`, `GET /system/metrics/summary` (for dashboard charts) |

### Roles & capabilities

WP-compatible role names; capabilities are core-defined strings checked in core
services (not in HTTP handlers), so CLI and API enforce the same rules.

| Capability | admin | editor | author | contributor |
|---|---|---|---|---|
| read admin | ✓ | ✓ | ✓ | ✓ |
| create/edit own content | ✓ | ✓ | ✓ | ✓ (only while draft or pending) |
| publish own content | ✓ | ✓ | ✓ | — |
| edit/publish others' content | ✓ | ✓ | — | — |
| upload media | ✓ | ✓ | ✓ | — |
| manage terms, menus, comments | ✓ | ✓ | — | — |
| manage users, settings, themes, plugins, import, redirects | ✓ | — | — | — |

As implemented in `ferrite-core` (`src/actor.rs`, `src/content/lifecycle.rs`; M1c-1; the
HTTP layer arrives with M2):

- **Status changes go through lifecycle methods only.** `ContentDraft` has no status,
  author or publish-time fields, so `ContentService::create`/`update` cannot change them.
  `publish`, `unpublish`, `schedule`, `submit`, `trash`, `restore` and `delete` each check
  permission and the expected `version` in one transaction, so the `POST
  /content/{type}/{id}/publish|unpublish|schedule|restore` endpoints above map onto them.
  A transition that is not allowed from the current status is `Invalid("illegal
  transition ...")`.
- **Publish, unpublish and schedule** need `publish_own`, or `publish_others` for another
  author's item. Contributors therefore cannot do any of them.
- **Submit and trash** need the same rights as editing the item, which for a contributor
  means only their own item and only while it is draft or pending.
- **R8: contributors cannot restore or permanently delete**, even their own items: a trashed
  item gives them nothing. For everyone else `restore` and `delete` need `edit_own` on
  own items and `edit_others` otherwise, whatever the status the item had before.

## 2. Auth

- **Passwords**: argon2id (`argon2` crate), parameters from OWASP guidance, rehash on
  login if parameters changed. Imported WP users get a forced password reset (WXR exports
  contain no password hashes). Verifying WP hashes on first login (`legacy_hash`) is
  **later**, only if a direct WP-database user import is added.
- **Sessions**: DB-backed ([04 §5](04-cluster.md#5-sessions)). Cookie `ferrite_session`:
  `HttpOnly; Secure; SameSite=Lax; Path=/`. Idle timeout 7 days, absolute 30 days.
- **CSRF**: for cookie-authenticated mutations, require header `X-CSRF-Token` equal to
  an HMAC of the session's `csrf_secret` (delivered via `GET /auth/me`). Plus `Origin`
  check against `site.url`.
- **API tokens**: `Authorization: Bearer fer_<random>`, stored hashed, scoped
  (`content:read`, `content:write`, `media:write`, …), no CSRF needed.
- **TOTP** optional per user (M2). **Passkeys** (`webauthn-rs`) M2 stretch.
- **Rate limiting**: login 10/min per IP-hash + 5/min per login; DB-backed counters.
- Password reset e-mail needs SMTP config (`[mail]` section, `lettre`); without SMTP
  admins reset via CLI `ferrite user reset-password <login>`.

## 3. Admin SPA (D6, D12)

### Stack

| Concern | Choice | Why |
|---|---|---|
| Build | **Vite** + **React** + **TypeScript** (strict) | Static output embeddable in the binary; no SSR needed |
| Components | **shadcn/ui** (Radix primitives, Tailwind CSS) | User requirement; components are copied into the repo, fully ownable |
| Charts | **Recharts v3** via shadcn `chart` component | shadcn's chart is built on Recharts (verified: `recharts@3.8.0` in the shadcn registry) |
| Transitions / animation | **Motion** (`motion` package, formerly Framer Motion) for layout/page transitions; Tailwind `animate-*` / `tw-animate-css` for simple ones | User requirement |
| Routing | **TanStack Router** (file-based, type-safe search params) | Typed routes for many list/filter screens |
| Server state | **TanStack Query** | Caching, optimistic updates, retries |
| Forms | `react-hook-form` + `zod` (schemas also generated from OpenAPI where practical) | shadcn `form` components pair with these |
| Tables | TanStack Table via shadcn `data-table` pattern | Posts/media/users lists |
| Editor | **TipTap** (ProseMirror) with React bindings | Block editor; JSON document stored as `body` |
| Icons | `lucide-react` | shadcn default |
| Toasts | `sonner` (shadcn) | |
| Drag & drop | `@dnd-kit` | Menus, block reordering, media |
| i18n | `i18next` (admin strings) | EU audience; English first |
| Tests | Vitest + Testing Library; Playwright for E2E against a real `ferrite serve` | |

Versions pinned in `admin/package.json` at scaffold time (latest stable then); pnpm.

### Embedding & serving

- `admin/` builds to `admin/dist/`. `crates/ferrite-http` embeds it with `rust-embed`
  (8.x) under `/admin/*`, SPA fallback to `index.html`, hashed assets
  `Cache-Control: immutable`.
- `build.rs` does **not** run pnpm (keeps `cargo build` hermetic); CI builds the admin
  first. A missing `dist/` compiles in a placeholder page saying "admin not built".
- Dev: `pnpm dev` (Vite on :5173) proxies `/api` to `ferrite serve` on :8080.
- CSP for admin: `default-src 'self'; img-src 'self' data: blob: <media base>;
  style-src 'self' 'unsafe-inline'` (shadcn chart injects `<style>` — verify whether a
  nonce can be used; otherwise allow inline styles only), no inline scripts.

### Screens (M2)

| Screen | Notes |
|---|---|
| Dashboard | Charts: page views (core lightweight counter, privacy-friendly; full analytics is a plugin), cache hit rate, plugin hook p95 latency per plugin, job queue depth, recent content, failed jobs |
| Posts / Pages / custom types | Data table with filters, bulk actions, status tabs; animated row transitions |
| Editor | TipTap; sidebar: status, schedule, slug, terms, featured image, SEO fields, custom fields; revisions drawer with diff; autosave (draft revision) every 10 s; conflict UI on 412 |
| Media library | Grid + list, drag-and-drop upload with progress, alt text, variant preview |
| Content types | Field builder (text, rich text, number, date, bool, select, relation, media, repeater) |
| Taxonomies | Tree editor for hierarchical |
| Menus | Drag-and-drop tree |
| Comments | Moderation queue |
| Users | List, roles, tokens, TOTP |
| Themes | Upload, activate, settings form generated from `theme.toml [settings]` |
| Plugins | List with status/failures; **install flow shows the permission report** ("This plugin can: read posts and pages; write meta `seo.*`; connect to `api.indexnow.org`") with high-risk items highlighted; per-plugin metrics chart |
| Redirects | Table, CSV import/export, hit counts |
| Import | WXR upload, progress (job polling), link to import report |
| System | Versions, schema status, backends, nodes (cluster), jobs (retry dead), backups (solo) |
| Settings | General, reading (front page, posts per page), permalinks, media sizes, comments, mail |

### UX principles

- Every destructive action has undo (trash, revision restore) rather than confirm
  dialogs where possible.
- Motion is used for meaning (list reorder, panel open, page transitions ≤ 200 ms) and
  respects `prefers-reduced-motion`.
- Dark mode via shadcn theme tokens.
- Keyboard: command palette (`⌘K`, shadcn `command`) for navigation and "new post".
