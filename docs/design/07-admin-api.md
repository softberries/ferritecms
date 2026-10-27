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
- Filtering (content lists): `GET /content/{type}?status=draft,pending&term=<id>&author=<id>&parent=root|<id>&q=<search>&order=published|updated|title|menu`
  (defaults: every status but `trash`, order `updated`).
- Optimistic concurrency: content carries `version`; writes require
  `If-Match: "<version>"` → `412` on conflict with the current resource in the body
  (other resources are last-write-wins, P11; see "Resources").
- Idempotency (M2a-2b P10, built): `POST /media` accepts an `Idempotency-Key` header
  (1–255 bytes of `0x21..=0x7E`; empty, longer, other bytes or the header twice are
  `400 malformed-header`). The key is scoped to (user, key): the store keeps the SHA-256
  hex of `user id || 0x00 || key` with the request's method and path (query included),
  so one user cannot replay or probe another's key. It is claimed after the caller is
  authorized and before the body is read. A finished key replays the stored status,
  JSON body, `Location` and `ETag` with `Idempotent-Replayed: true` without reading the
  body (a retry with another file is not detected); a key whose request is still running
  is `409 conflict` with `Retry-After: 5`, and one pending for more than 1 h (database
  clock) is taken over by the next retry (F5: an ownership token, so the original
  request can no longer record or free the key); a key reused for another method or
  path is `422 validation` (`errors[0].field` = `Idempotency-Key`; checked before the
  pending/finished state, so a pending key on another path is `422`, not `409`). Only a
  `2xx` outcome is stored (≤ 64 KiB); any failure, and a request dropped mid-upload,
  frees the key so a retry runs. Keys are deleted 24 h after the claim by
  `sessions.prune` (04 §6). Imports take the header in M3 (P26); extension uploads do
  not (M2a-2 P11: an install is a primary-key upsert, so a retry creates no second
  row). Limits — each can give a second item for one key:
  - *A failed finish.* When storing the outcome fails (logged; the `201` still goes
    out), the key stays pending: retries get `409` until it is 1 h old, then one takes
    it over and uploads again.
  - *An upload longer than 1 h.* A retry takes the key over while the original still
    streams; the original still records its media item, only its outcome is not stored
    under the key.
  - *A drop during the commit.* A client that goes away while the item's transaction
    commits frees the key (the request is treated as failed) although the item may be
    committed; a retry then uploads again.
- OpenAPI 3.1 generated from code with `utoipa` (6.x) at `/api/v1/openapi.json`. As
  built (M2b-1): the admin's types are generated from the checked-in
  `crates/ferrite-http/openapi.json` by `pnpm -C admin api:gen` (`openapi-typescript`
  7.13.0 with `--root-types`) into the committed `admin/src/api/schema.d.ts`, and
  `pnpm -C admin api:check` regenerates to a temp file and fails with a diff when the
  committed file differs (drift is a regenerate-and-diff, M2b P6; the CI admin job that runs it arrives in M2b-1 Task 6). Every
  `$ref` in the spec must resolve (`openapi_json_is_valid_3_1`). The request layer
  (`admin/src/api/`) is one `openapi-fetch` 0.17.0 client on the page's origin (the
  generated paths carry `/api/v1`) with one middleware: `Accept: application/json` on
  every request, `X-CSRF-Token` (held in memory only) on every request but `GET`/`HEAD`,
  and an `application/problem+json` response thrown as `ApiError` (`status`, `problem`,
  `slug`, `fieldErrors()`, `retryAfter`); a failed response without a problem document
  becomes one too. Request bodies are not validated at runtime. The query client's
  global `onError` sends `unauthenticated` to `/admin/login?next=<path and query>`
  (never the fragment; not from the sign-in pages; once for parallel failures) and
  `password-reset-required` to `/admin/change-password`, both through the router with
  `ignoreBlocker` (never a full-page load), and shows `rate-limited`/`busy` as a toast
  with the `Retry-After` seconds. Queries retry network errors (a rejected `fetch`, wrapped as `NetworkError`) and `503` twice (after
  `Retry-After` when sent) and never another HTTP error; mutations never retry.

### As implemented (M2a-1 foundation, `crates/ferrite-http/src/api/`)

- **One process, one stack.** `ferrite_http::build_site` merges the API router in front
  of the public fallback; both sit inside the same layers (security headers,
  compression, request span, client address, catch-panic). The API never reads or fills
  the page cache, and every API response has `Cache-Control: no-store` (P24).
- **Routing.** Handlers carry `#[utoipa::path]`s relative to `/api/v1` and are
  registered with utoipa-axum's `routes!` on an `OpenApiRouter` nested at `/api/v1`, so
  the spec and the routes come from one list. An unknown path under `/api/v1` (also
  `/api/v1` and `/api/v1/` themselves) is a `404 not-found` problem for any method; a
  known path with another method is `405 method-not-allowed` with `Allow`; a panicking
  handler is a `500 internal` problem. Paths outside `/api/v1` (`/api`, `/api/v2/…`)
  belong to the public site (canonical slash, then the shared 404 page).
- **Unauthenticated endpoints:** `GET /api/v1/meta` → `{ api: "v1", version, site_url }`
  (the admin's first call) and `GET /api/v1/openapi.json`. The document is OpenAPI
  3.1.0 with security schemes `session` (cookie `__Host-ferrite_session`, `ferrite_session`
  on an `http` site; §2) and `bearer`; the
  pretty-printed copy `crates/ferrite-http/openapi.json` is checked by the test
  `openapi_snapshot_is_current` (`FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http
  --test api openapi` rewrites it).
- **Problems** (RFC 9457, `application/problem+json`, P10): `type` is
  `/api/v1/problems/<slug>`, plus `title`, `status`, `detail`, `errors[]` (`{ field,
  detail }`, omitted when empty) and `current` (412 only); `Retry-After` when set.
  Core errors map as follows:

  | Core error | Status | Slug |
  |---|---|---|
  | `NotFound` | 404 | `not-found` |
  | `Forbidden` | 403 | `forbidden` (detail names the capability or scope) |
  | `Unauthenticated` | 401 | `unauthenticated` |
  | `RateLimited` | 429 | `rate-limited` + `Retry-After` |
  | `Invalid` | 422 | `validation` |
  | `InvalidField { field, .. }` | 422 | `validation` with `errors[0].field` naming the input member |
  | `Conflict` (version) | 412 | `precondition-failed` + `current` |
  | store unique violation | 409 | `conflict`; a known constraint adds `errors[0].field` (e.g. `slug`); the constraint name is never sent |
  | store foreign-key violation | 409 | `conflict` ("a related record changed; retry") |
  | store busy | 503 | `busy` + `Retry-After: 5` |
  | randomness, hashing, other store errors | 500 | `internal` |

  The API layer adds its own problems (login `invalid-credentials`, `csrf`,
  `password-reset-required`, request-shape slugs) and `503 mail-unavailable` (M2a-2a): a
  password-reset request on a site without `[mail]`.

  Store, randomness and hashing errors are logged with their text; the body says only
  "internal error" or "busy, retry".
- **Request rules.** JSON bodies need `Content-Type: application/json` (`415
  unsupported-media-type`), are at most 4 MiB (`413 too-large`), and must parse into the
  endpoint's shape (`400 malformed-json`, with the serde path of the bad member in
  `errors[0].field`). A NUL character in any string of the document is `422
  validation`. A query string that does not parse is `400 malformed-query`. Lists take
  `?limit=` (default 50, outside 1..=200 `400 validation`) and `?cursor=` and answer
  `{ items, next_cursor }` with one list type per resource. `If-Match` must be one
  strong tag holding the version, `"<n>"`; without it `428 precondition-required`, any
  other form `400 malformed-header`. Resources send `ETag: "<version>"`; creates answer
  `201` with `Location` and `ETag`.

### Resources

Paths are relative to `/api/v1`. **Status:** M2a-1 and M2a-2a = built (in
`crates/ferrite-http/openapi.json`); M2a-2b = next backend plan; M3 = with the WordPress
importer. The admin SPA that uses them is M2b.

| Resource | Endpoints | Status | Core check (role capability, then token scope) |
|---|---|---|---|
| Meta | `GET /meta`, `GET /openapi.json` (no auth) | M2a-1 | none |
| Auth | `POST /auth/login`, `POST /auth/login/totp`, `POST /auth/logout`, `GET /auth/me`, `POST /auth/password`, `POST /auth/totp/setup`, `POST /auth/totp/confirm`, `POST /auth/totp/disable`; `GET /auth/sessions`, `DELETE /auth/sessions/{id}`, `POST /auth/sessions/revoke-others` (see **Sessions** in §2) | M2a-1; sessions M2b-3 | own account |
| Auth: password reset by e-mail | `POST /auth/password-reset`, `POST /auth/password-reset/confirm` (no credentials; origin-checked like login) | M2a-2a (needs `[mail]`) | none (the sealed token is the credential) |
| Content | `GET/POST /content/{type}`, `GET/PATCH/DELETE /content/{type}/{id}` (`DELETE` trashes; `?force=true` deletes a trashed item), `POST /content/{type}/{id}/publish`, `…/unpublish`, `…/submit`, `…/schedule`, `…/restore` (from trash); type `media` is 404 here (uploads are `/media`). Items and list entries carry `author` (`{id, login, display_name}`) and `allowed` (`{edit, publish, unpublish, schedule, submit, trash, restore, delete, trash_undoable}`, from core's `allowed_actions`; `trash_undoable`: trash is allowed and restore would be once trashed, the admin's Undo, M2b-1 Task 13 fix round 1); `GET /content/{type}/counts` → `{draft, pending, scheduled, published, private, trash}` (M2b P13, P14); `GET /content/refs?ids=<uuid>,…` names items by id across types (relation fields, menu targets, the comments queue's item filter): `{items: [{id, type, title, status, permalink}]}` for the ids the caller may read (the lists' rule, any status; media items only with `media:read`; `permalink` a path while the item has a public page now, else `null`), in no particular order; unknown and unreadable ids are absent, never an error; 1..=200 ids (`422`, `errors[0].field` `ids`), a malformed id `400` (`ids`); one store query plus the permalink reads; the static `refs` segment wins over `{type}` and core reserves `refs` as a type (and taxonomy) name (M2b-2 Task 14); `GET/PUT/DELETE /content/{type}/{id}/autosave`, the caller's own working copy (see **Autosave** below, P15). Items (not list entries) carry `seo: {title, description}` (each a string or `null`; `content_meta` `seo.title`/`seo.description`, M2b P28); create and `PATCH` take `seo?: {title?, description?}`: absent keeps a member, `null` or a string blank after trimming deletes it, a string sets it trimmed (no NUL, and no control characters once trimmed, so an edge newline is trimmed and an inner one refused; title at most 200 and description at most 500 characters, else `422` with `errors[0].field` `seo.title`/`seo.description`), in the item's transaction; a `PATCH` of `seo` alone is a full save (`If-Match`, a new version and revision); revisions do not restore SEO (P13). A `body` that is sent (create, a `PATCH` that includes `body`, autosave `PUT`) or restored (revision restore) must be an object with `"type": "doc"` (`422`, `errors[0].field` `body`); a `PATCH` without `body` keeps the stored body as it is, even one that predates the check, and stored bodies are never rewritten (M2b-2 Task 1) | M2a-1; `author`, `allowed`, `counts`, `autosave` M2b-1; `seo`, body root check, `refs` M2b-2 | `read_admin` + `content:read` and `can_read`; writes `edit_own`/`edit_others`/`publish_*` + `content:write` |
| Revisions | `GET /content/{type}/{id}/revisions`, `GET …/revisions/{rid}`, `POST …/revisions/{rid}/restore` | M2a-1 | reads as content plus a role that may edit the item (R26, §1 "Reading revisions"); restore as a content update |
| Content types | `GET/POST /types`, `GET/PATCH/DELETE /types/{name}`. A field (`FieldDefDto`) carries `types: string[]` (M2b-3 P52), only for `relation` fields, sub-fields of a repeater included: distinct registered type names (the new type's own name too), never `media`; empty means any type; on another kind, an unknown or repeated name, or `media`, the type write is `422` (`errors[0].field` `fields`, the message names the path). A content create or update whose relation value names an existing item of a type the field does not list is `422` with `errors[0].field` at the value's path (`fields.related`, `fields.chapters[1].related`); a missing item is accepted; the items are read with one store query per 200 distinct ids (one for any usual item). A type another type's relation lists cannot be deleted (`422`), nor can any type while another type's stored schema no longer parses | M2a-1; `types` M2b-3 | reads `content:read`; writes `manage_settings` + `settings:write` |
| Taxonomies & terms | `GET/POST /taxonomies`, `GET/PATCH/DELETE /taxonomies/{name}`, `GET/POST /taxonomies/{taxonomy}/terms`, `GET/PATCH/DELETE /terms/{id}` | M2a-1 | taxonomy reads `content:read`, writes `manage_settings` + `settings:write`; term reads `terms:read`, writes `manage_terms` + `terms:write` |
| Menus | `GET /menus`, `GET/PUT/DELETE /menus/{name}` (`PUT` replaces the whole tree); the list also carries `locations: [{name, label, menu}]`, the `[menus]` table of the theme the answering node serves now (after any hot swap; by name, empty when it declares none), `menu` = a menu with that name exists (M2b-2 Task 3) | M2a-1; `locations` M2b-2 | reads `menus:read`; writes `manage_menus` + `menus:write` |
| Users & tokens | `GET/POST /users` (`GET ?q&role&status&limit&cursor`: `q` is trimmed, 1 to 100 characters, else `422` with `errors[0].field` `q`, and matches the login, display name or e-mail as a case-insensitive substring with `%`, `_` and `\` literal (`LIKE`/`ILIKE … ESCAPE '\'`; SQLite folds ASCII case only); `role` and `status` match exactly, an unknown value is `400 malformed-query`; order (login) and cursor as without them, M2b-3 P46), `GET/PATCH/DELETE /users/{id}` (`DELETE ?reassign=<id>` moves the user's content; a user who owns content and no `reassign`, or a `reassign` that is the user itself, unknown or not active, is `422` with `errors[0].field` `reassign`; a `PATCH` changing one's **own** `email` to a different address needs `current_password`: missing `422` with `errors[0].field` `current_password`, wrong `403 forbidden`, counted on the per-login bucket of `POST /auth/password` (5/min, `429`); the same address needs none; an admin changing another user's e-mail needs none, M2b-3 P44), `POST /users/{id}/password`, `POST /users/{id}/totp/disable` (`204`; turns off another user's TOTP, the lost-authenticator path: their sessions and tokens are kept, audited `user.totp_disable` with the actor; oneself `422` with `errors[0].field` `id`, TOTP not on `422`; M2b-3 P43), `GET/POST /users/{id}/tokens`, `DELETE /users/{id}/tokens/{tid}` | M2a-1; filters, `current_password`, `totp/disable` M2b-3 | others: `manage_users` + `users:read`/`users:write`; oneself: own e-mail (with the password), display name and tokens (`users:read` to list tokens, `users:write` to change); own password via `POST /auth/password`, own TOTP via `/auth/totp/*` |
| Redirects | `GET/POST /redirects` (`GET ?q&limit&cursor`: `q` is trimmed, 1 to 200 characters, else `422` with `errors[0].field` `q`, and matches `source` or `target` as a case-insensitive substring with `%`, `_` and `\` literal (`LIKE`/`ILIKE … ESCAPE '\'`; SQLite folds ASCII case only); order and cursor as without it), `GET/PATCH/DELETE /redirects/{id}`, `POST /redirects/import` (CSV), `GET /redirects/export` (CSV) | M2a-1; `q` M2b-2 | `manage_redirects` (admins) + `redirects:read`/`redirects:write` |
| Settings | `GET/PATCH /settings` (the allowlist `ferrite_core::settings::SETTINGS`) | M2a-1 | reads `settings:read`; writes `manage_settings` + `settings:write` |
| Media | `POST /media` (multipart, streaming, `Idempotency-Key`), `GET /media?q&kind&author&trash&limit&cursor`, `GET/PATCH/DELETE /media/{id}` (`PATCH` title and alt with `If-Match`; `DELETE` trashes, `?force=true` deletes a trashed item), `POST /media/{id}/restore` (`If-Match`; from trash to the status it had: published, or draft when it was a draft, M2b-3 P53); see **Media** below | M2a-2b (built); `q`, `kind`, `trash`, restore M2b-2 (P33); restore keeps the status M2b-3 | upload `upload_media` + `media:write`; reads `media:read` and `can_read` per item; edit, trash, restore and delete the content role rules with `media:write` in place of `content:write` (restore publishes only with `publish_own`/`publish_others`, else the item comes back a draft) |
| Comments | `GET /comments?status&item&limit&cursor`, `GET /comments/counts?item` → `{pending, approved, spam, trash}` (every status, zero-filled), `PATCH /comments/{id}` (`{ status }`), `DELETE /comments/{id}` (spam or trash only), public `POST /content/{type}/{id}/comments` (rate-limited, honeypot; see **Comments** below). Listed and moderated comments carry `item: {id, type, title, permalink, status}` (`permalink` a path while the item has a public page now, else `null`; `title` is `""` for an item without a public page when the caller may not read content, e.g. a `comments:read` token without `content:read`; M2b-2 Task 3) | M2a-2b (built); `item`, `counts` M2b-2 | moderation `moderate_comments` + `comments:read`/`comments:write`; the submit: none, or a token with `comments:write` |
| Themes | `GET /themes`, `POST /themes` (raw ustar bundle, `?activate=true`), `POST /themes/{name}/activate`, `GET/PATCH /themes/{name}/settings` (no delete, M2a-2 P30); manifest facts in `ThemeDto` (M2b-3) | M2a-2a, M2b-3 | reads `manage_themes` + `extensions:read` (`require_reader`); writes `manage_themes` + `extensions:write` |
| Plugins | `GET /plugins`, `GET /plugins/{name}`, `POST /plugins/inspect` (multipart upload → permission report, nothing installed), `POST /plugins` (install with `approve` = the report's fingerprint), `POST /plugins/{name}/enable`, `POST /plugins/{name}/disable`, `DELETE /plugins/{name}` (`?purge=true`), `GET/PATCH /plugins/{name}/settings`; `GET /plugins/retained`, `POST /plugins/{name}/purge` (M2b-3 P47) | M2a-2a, M2b-3 | reads `manage_plugins` + `extensions:read` (`require_reader`, so editors see no plugin list); writes `manage_plugins` + `extensions:write` |
| Plugin metrics | `GET /plugins/{name}/metrics` (24 h of failures per hook and hour, every node's; the answering node's per-hook call stats; 05 §8) | M2a-2b (built) | `manage_plugins` + `extensions:read` |
| System | `GET /system/status` (versions, mode, backends, schema, the active theme and the nodes serving a fallback, job counts, node counts), `GET /system/nodes`, `GET /system/audit?actor&action&target_kind&target_id&limit&cursor`, `GET /system/jobs?status&kind&limit&cursor`, `POST /system/jobs/{id}/retry`, `GET /system/metrics/summary`, `POST /system/mail/test` (see **System** below) | M2a-2b (built); audit filters, `mail`, typed node `meta` and the test mail M2b-3 | `manage_system` (admins) + `system:read`; retry and test mail `system:write` |
| Import | `POST /import/wordpress` (upload WXR + optional uploads zip → job), `GET /import/{job}` | M3 | — |

**Versions and `If-Match` (P11).** Only content has a `version`: `PATCH`, `DELETE`, the
lifecycle `POST`s and revision restore need `If-Match: "<version>"`. Users, terms, menus,
redirects and settings have no version column and are last-write-wins (two admins saving
one menu at once lose one edit). `PATCH` keeps omitted members; `slug: null` is refused;
`parent_id: null` moves an item to the top level; an absent `excerpt` keeps the stored one
and `excerpt: null` derives it from the body (F20). A list with `q` is ordered by
relevance, and `q` combined with `author`, `term` or `parent` is `422` (F19). Revision
restore restores title, excerpt, body and fields, not meta (P13). Field values are
validated against the type's schema on every content write; a type whose `fields` has no
`fields` member (the built-ins' `{}`) accepts any object, and `featured_image` (UUID string
or null) is always allowed (F17).

**Autosave (M2b P15; F11, F12).** Each user has at most one autosave per item
(`autosaves`, migration `0007`), kept apart from the item and its revisions. `PUT
/content/{type}/{id}/autosave` takes `{ base_version, title, excerpt?, slug?, parent_id?,
menu_order?, terms?, body, fields, seo? }` and stores it (`200` with `AutosaveDto`, the same
members plus `saved_at`); there is no `If-Match` and the last write wins. `excerpt: null`
means "derive from the body"; `parent_id` is the working copy's parent and `null` (or
absent) means top level, so the SPA always sends it (ruling R4); `slug`, `menu_order` and
`terms` absent or `null` mean "unchanged" (F11), and so does `seo` (the editor's SEO
patch, `{title?, description?}`, `autosaves.seo` since migration `0008`, M2b-2 Task 1:
checked as a content write checks it, then stored as sent, `null` members included). Text
is checked as a content write checks it: no NUL anywhere, and no control characters in
the slug (`422` with `errors[0].field`); the body must be a document (`type` `doc`,
field `body`); `terms` holds at most 200 ids. The field schema, the parent and the
terms are not checked: a draft may be incomplete. A trashed item takes no autosave (`422`, "restore first"). `GET` returns it
(`204` with no body when there is none, M2b-1 Task 16: the editor asks on every open, and
a `404` there would log a failed request each time); `DELETE` is `204` whether or not one existed. Writing needs
`can_edit` on the item as stored now (with `content:write`); reading needs
`content:read` and a role that may edit the item, as revisions do (R26), so a read-only
token reads its owner's slot; without such a role the answer is `403`, where revisions
answer `404`, because the item was already shown to be readable and the slot is the
caller's own, so `403` reveals nothing (and `PUT`/`DELETE` answer `403` alike). The item itself must be readable (`404` otherwise). An
autosave writes no revision, event, action job, search entry or cache invalidation. A
real save of the item by the same user (`PATCH`, revision restore) deletes that user's
slot in the same transaction; other users' slots stay. Deleting the item or the user
deletes their slots; there is no retention job.

**Redirects (R20, R21, R26).** `POST /redirects` creates the redirect for `source` or
changes the one stored for it; `PATCH /redirects/{id}` refuses a `source` another row
holds, exactly or with its trailing slash toggled (F24), and creating a new source
(`POST` or import) whose toggled variant is stored is refused the same way (`422`,
`errors[0].field` = `source`; R26). A source whose first segment (percent-decoded, any
case) the router owns is refused on write: a `slug::RESERVED` word (`admin`, `api`,
`feed`, `_media`, `sitemap.xml`, `robots.txt`, `healthz`, …), any word starting with
`_`, or a sitemap shard name `sitemap-*.xml` (R21, R26); stored rows keep resolving.
`POST /redirects/import` takes `text/csv` (else `415`) of at most 1 MiB (`413
too-large`) and 10 000 rows, `source,target,code` with an optional header row; every row
is checked first and nothing is written if any fails (`422 validation`, one `errors`
entry per bad line, at most 100). `GET /redirects/export` writes the same format; a
field starting with `=`, `+`, `-`, `@`, a tab or CR gets a leading `'`, so a spreadsheet
does not run it as a formula (R20).

**Media (M2a-2b P3–P10, R1; 03 §3–§5).** `MediaService` (`ferrite_core::media`) holds the
rights and the records; `ferrite_http::media::Uploader` stages the bytes outside any `Tx`.
- **Upload** (`POST /media`, `multipart/form-data`): parts `file` (required), `title`
  (≤ 200 characters) and `alt` (≤ 1 000), in any order. The caller is authorized and the
  `Idempotency-Key` claimed before any of the body is read; a declared body over
  `media.max_upload` + 1 MiB is `413` at once, and the file is cut at `media.max_upload`
  (default 256 MiB, at most 1 GiB; an SVG at most 2 MiB) while it streams. The file name
  must be 1–255 bytes without control or format characters (`422`, checked before the
  file is read). The type comes from the content's first 8 KiB (03 §3 "Accepted upload
  types"); anything else is `415` (`errors[0].field = file`), an SVG that cannot be made
  safe `422`. At most 8 uploads stream at once per node (`503 busy`), and an upload that
  receives nothing for `media.upload_idle_timeout` (default 30 s) is aborted (`400`). The
  answer is `201` with `Location`, `ETag` and `MediaDto`; the item is published at once
  (P5) and raster images get a `media.variants` job.
- **`MediaDto`**: `id`, `title`, `alt`, `mime` (decided from the content), `bytes`,
  `width`/`height` (set when the variants are made), `original_name`, `url`, `variants`
  (`name`, `url`, `width`, `height`, `mime`; empty until made), `sanitized`, `status`,
  `author_id`, `author` (`{id, login, display_name}`, the uploader, as content's
  `author`: one `users().get_many` per page or item through `MediaService::authors`,
  `media:read`, only for items the actor already read; M2b-2 Task 7), `created_at`,
  `updated_at`, `version`. Every response that carries a `MediaDto` has it (list, get,
  upload `201` and its idempotent replay, `PATCH`, restore, a `412`'s `current`).
- **List and edit**: `GET /media` lists every status but trash, newest change first,
  optionally by `author`; a role without `edit_others` sees its own items and public ones.
  `trash=true` lists only trashed items instead (same visibility rule). `kind`
  (`image`, `video`, `audio`, `document`) keeps the items whose `media.mime` starts with
  `image/` (SVG included), `video/` or `audio/`, `document` being every other type; an
  unknown value is `400 malformed-query`. It is a sub-select on `media` in the store's
  list, so pages and cursors stay exact (M2b-2 Task 2). `q` searches the search index,
  which holds a media item's **title** only (its body is empty; the original file name
  is not indexed, so `q` never matches it), best match first, with the search cursor;
  blank `q` is no search. `q` with `author` or with `trash=true` is `422` with
  `errors[0].field` `q` (trashed items leave the index, so a trash search could find
  nothing); `kind` combines with `q`, but is applied to the loaded hits, so a search page
  can be shorter than `limit` while `next_cursor` is set. A list cursor and a search
  cursor are not interchangeable (`400`, field `cursor`).
  `POST /media/{id}/restore` (`If-Match`, `200 MediaDto` with `ETag`) restores a trashed
  item in one `Tx` through the content lifecycle's restore (R8: `edit_own` for the
  author, `edit_others` otherwise, never contributors; indexed for search again) and
  then, unless it was a draft when it was trashed, publishes it, so it ends `published`
  (the status uploads have, P5) with its `published_at` kept and its `/_media` URLs
  answer again (M2b-3 P53). The trash writes the item's status before it into
  `content_meta` `media.trashed_from` (`"published"`, or `"draft"` for any other status)
  in its `Tx`; the restore reads and deletes it. An item trashed without the row (before
  M2b-3, or through the content lifecycle) is published (R23). The publish needs
  `publish_own`/`publish_others`; a caller without it gets the item back as a draft, as
  a content restore does; `status` in the response says which. Each step emits its own
  `invalidate` event, and the publish queues the `post.published` action jobs again, as
  an upload does. A non-trashed item is `422`, a stale version `412` with `current`.
  `PATCH` changes `title` and `alt` (`If-Match`; a trashed item is `422`); `DELETE`
  (`If-Match`, `204`) trashes (its `/_media` URLs stop answering, since only public items
  are served) and `?force=true` deletes a trashed item and its `media` row. The blobs stay for M4's GC (03 §6).

**Comments (M2a-2b P11–P14, R2; F18–F22).** `CommentService` (`ferrite_core::comments`)
holds the rules for both submit paths (this endpoint and the theme form `POST /_comment`).
Only public `post` items take comments, while `comments.enabled` is on (default `true`);
anything else is `404`. Text is plain and stored as typed: body 1..=10 000 characters
(`\r\n` folded to `\n`, no control characters but `\n`/`\t`), name 1..=100, e-mail ≤ 254
bytes (required for guests), URL empty or `http`/`https` with a host and no user info, ≤ 200
characters (`javascript:` and the like are `422`, `errors[0].field = author_url`); output is
escaped by core (`render_body`: `<p>` paragraphs, `<br>` line breaks, no links). Replies go
to an approved comment of the same post, at most 5 deep (`parent_id`).
- **Submit** (`POST /content/{type}/{id}/comments`, JSON ≤ 64 KiB): credentials are optional
  (`MaybeAuthed`): a bearer token needs no origin but `comments:write` (`401` if bad); without
  one the request must come from the site's origin (`403 csrf`), and a session cookie that
  resolves needs its `X-CSRF-Token` (`403 csrf`) — a cookie that does not resolve is a guest.
  With an account the name and e-mail are the account's. No client address is `503 busy`.
  Per client address (IPv6 /64) 3 a minute and 20 an hour, counted in the database first, so
  the limit holds across nodes (`429` + `Retry-After`); a signed-in commenter whose role
  holds `moderate_comments` (session, or a token with `comments:write`) is not counted, so
  replies from the moderation queue are never refused (decided in core; M2b-2 Task 14).
  Bidi controls (U+202A–U+202E,
  U+2066–U+2069) are refused in names (R13; also in account display names). `201` =
  approved, `202` = held; a filled honeypot (`website`) on input that passes every check
  also answers `202`, with the same headers and body as a held comment under an id that is
  never stored (F18); invalid input is `422` either way. A discard returns faster (no
  lookups, no insert): timing can tell it apart, which is accepted, as the field is no
  secret and only traps bots that fill every field. The response (`CommentPublicDto`) never carries the e-mail or address hash.
- **Status**: a commenter whose role holds `moderate_comments` is approved. Under
  `comments.moderation = "known"` (default, R2) a comment with at most two links (`http://`,
  `https://`, `www.`) is approved when its author has an approved comment — a guest by the
  same e-mail (ASCII case folded) and trimmed name on an approved *guest* comment, never when
  the e-mail belongs to an account, and never through comments an account wrote before it was
  deleted (`comments.by_account`); a signed-in commenter by an approved comment of the account
  (F19). `"all"` holds every comment but a moderator's. **Spoofing risk:** under `known`
  anyone who knows a regular guest's exact name and e-mail posts unmoderated as them (the
  e-mail is never shown publicly, but it is not a secret either); this is WordPress's
  behaviour, and sites that cannot accept it set `"all"`. Account comments are not
  affected (an account's e-mail never vouches for a guest).
- **Moderation**: `GET /comments` (newest first, filters `status`, `item`) answers
  `CommentDto` with every column, the e-mail and the keyed `ip_hash` (HMAC under the site key,
  label `ferrite:comment-ip:v1`) included, plus `item` (`CommentItemDto`): core reads the
  page's items with one `get_many` and the paths of those with a public page (published,
  `published_at` reached, not media) through the permalink rules; a comment whose item is
  missing (the foreign key makes that a race with a purge) is a `500`, never a made-up item.
  `PATCH` answers the same shape. `GET /comments/counts?item` counts per status in one grouped
  `SELECT` (an unknown item counts zero; same checks as the list). `PATCH` sets the status; `DELETE` removes a spam or
  trashed comment and its replies (else `422`, field `status`). An approved insert and every
  `PATCH`/`DELETE` write `invalidate([post:<item>])` in the same `Tx`. Not audited (P14).
  Settings `comments.enabled`, `comments.moderation` and `comments.per_page` show `null`
  until set (defaults are core constants, F21).
- **Public pages (M2b-2 P34)**: the public site pages a post's thread by top-level threads,
  `comments.per_page` (integer 1..=100, default 50) a page, oldest first, each with all its
  approved replies; a reply whose parent is not approved starts its own thread. The page
  is `?cpage=N` (in the page cache key; past the last shows the last), the theme's count is
  every approved comment, and the theme form's `303` for an approved comment goes to
  `?cpage=N#comment-<id>` on the page that holds it (06 §4). The store reads one page with
  a recursive query (`CommentRepo::approved_page`: every root of the page, replies cut
  past 2,000 rows), plus
  `approved_totals` and `root_position`.

**Extensions (M2a-2a; rulings in [`m2a2.md`](../superpowers/rulings/m2a2.md)).** Handlers
authorize through `ExtensionAdmin` (`authorize_plugins` / `authorize_themes`) before any of
an upload's body is read, so the OpenAPI sweep's unauthenticated and CSRF cases hold for
the upload routes too; the node's one `Installer` checks and compiles uploads (two
compiles and four uploads at once per node; a fifth upload is `503 busy` with
`Retry-After`). Every write takes the `extensions` lock (30 s, then `503 busy`) and
appends an audit row (`plugin.install|enable|disable|delete|purge|settings`,
`theme.install|activate|settings`; never setting values). No `If-Match` (P11).

- **Plugin uploads** are `multipart/form-data` with the parts `manifest` (UTF-8 TOML,
  ≤ 64 KiB), `component` (≤ 32 MiB) and, for `POST /plugins`, `approve` (≤ 128 bytes);
  the route body limit is 33 MiB, and a part over its own limit is `413 too-large` naming
  it before more of it is buffered. An unknown or repeated part, or a bad boundary, is
  `400`; a missing part `422` naming it; a refused manifest is `422 validation` with one
  `errors[]` entry per problem, `field` = the manifest path (`capabilities.http`, …; a
  manifest that is not UTF-8: `manifest`), a refused component `422` with `field =
  component` (a host fault while compiling it, an engine or host-configuration error,
  is `500 internal`); a limit above its ceiling is refused at inspection. `POST /plugins/inspect` answers
  `{ plugin, version, artifact, items[], added[], limits[], fingerprint, update,
  description, publisher, license, homepage }` (05 §2; the last four from the upload's
  `[plugin]` table, `null` when unset;
  `limits` lists only the limits the manifest requests); `POST /plugins` with an `approve` that is not that upload's
  fingerprint is `422 validation`, `errors[0].field = "approve"`, with the upload's
  `report` in the problem, and changes nothing (R1). Install answers `201` with `Location`
  (an update too); a new plugin is installed disabled, a replaced one keeps its state.
- **`PluginDto`**: `name`, `version`, `enabled`, `disabled_reason`, `installed_at`,
  `updated_at`, `description`, `publisher`, `license`, `homepage` (from the stored
  manifest's `[plugin]` table, `null` when unset; author text: shown as text, the
  publisher unverified; M2b-3; `homepage` is always an `http(s)://` URL or `null`: an
  upload with another scheme is refused as `plugin.homepage`, and a row stored before
  that rule keeps loading but shows `null`), `report` (the row's report against its own grant, so `added` is empty for
  a row installed with approval; `null` if the stored manifest or grant no longer parses)
  and `nodes` (each node with a heartbeat in the last 30 s: `node`, `name`, `version`,
  `status` = `loaded|failed|disabled`, `error`: the node's diagnostic, scrubbed of paths,
  endpoints, addresses and credentials and cut to 1 KiB (see **System** below); admins
  only).
- **Retained plugin data (M2b-3 P47)**: a delete without `purge` keeps the plugin's KV and
  settings for 30 days behind the option `plugin.<name>.deleted_at` (05 §2).
  `GET /plugins/retained` (`manage_plugins` + `extensions:read`) answers `{ items: [{ name,
  deleted_at, purge_after }] }`, one per such marker whose plugin is not installed again,
  by name (`PluginRepo::retained`; `purge_after` = `deleted_at` + 30 days, when the
  delete's purge job erases the data). `POST /plugins/{name}/purge` (`extensions:write`)
  erases it now under the `extensions` lock: the KV, every stored
  `plugin.<name>.settings.*` option (with `options.changed`) and the marker, audited
  `plugin.purge {name}`; `204`. A plugin of that name installed is `422 validation`,
  `errors[0].field = "name"` (delete it with `purge=true`); no marker is `404`. A delete
  with `purge=true` keeps its marker until its final sweep (`PLUGIN_PURGE_GRACE`, 2
  minutes), so it is listed for those minutes. **`retained` is a reserved plugin name**
  (the R9 rule: `/plugins/retained` is a static route, so a manifest naming a plugin
  `retained`, like `inspect`, is refused as `plugin.name`). **Known window:** this purge
  deletes the marker, so the delete's job no longer sweeps. A node that has not yet
  reloaded after the delete (it reloads on `plugins.changed`, normally within a second)
  can still write KV after an immediate purge. That KV then has no marker, nothing erases
  it, and a later install of the same name finds it. `DELETE ?purge=true` has no such
  window: its job sweeps again after `PLUGIN_PURGE_GRACE`. Purging a few minutes after the
  delete avoids it.
- **Theme uploads** are the raw deterministic ustar bundle (`Content-Type:
  application/x-tar`, else `415`; at most `ferrite_theme::MAX_BUNDLE_BYTES`, else `413`),
  unpacked in memory and compiled before anything is stored; a theme that does not load is
  `422` with one `errors[]` entry per problem (`templates/<file>:<line>: …`).
  `POST /themes/{name}/activate` loads the stored artifact first (fetched, hash-checked,
  unpacked, compiled): one that does not load is `422` and nothing changes (no
  `theme.changed`); a row replaced meanwhile is `409 conflict` (R10); an embedded theme
  without a row is installed from the embedded files and activated. `GET /themes` lists
  the installed themes and the embedded ones without a row; the active entry (the active
  row, else the embedded `blog`) carries `nodes`: per live node `node`, `node_name`,
  `node_version`, the served `name` and `hash`, `origin`, `error` and `fallback` (`true`
  when the node does not serve the active row's artifact: a startup fallback or a failed
  hot swap; 06 §1); `error` is scrubbed like a plugin's. Every entry also carries the
  manifest facts (M2b-3), from the row's stored manifest or the embedded `theme.toml`:
  `author`, `license`, `homepage` (`http(s)://` only, 06 §1), `has_settings` (a non-empty
  `[settings]`), `menus` (`{ name, label }`, by name), `images` (`{ name, width, height,
  crop }`, the `[images]` table by name; `height` `null` when unset) and `hash` (the row's
  artifact, `null` for an embedded theme without a row). A row stored before a key
  existed reads as without it.
- **Settings**: `GET/PATCH /themes/{name}/settings` and `/plugins/{name}/settings` answer
  `{ schema, values }` (plugins also `secrets` and `invalid`, 05 §4c); `PATCH` takes an
  object of keys (`null` resets or deletes one), checks every entry first (an undeclared
  key or a bad value is `422 validation` with `errors[0].field` naming the key) and writes them in one
  `Tx` with `options.changed`. Secrets are write-only.

**System (M2a-2b P15, P18; Review Focus 5).** `SystemService` (`ferrite_core`) checks
`manage_system` (a new admin-only capability, scope `system:write`) and `system:read`
before it reads anything, so an editor or a token without the scope learns nothing (not
the version, the mode or the migration state); the unauthenticated liveness probe stays
`GET /healthz`, which reports none of it.
- `GET /system/status` answers `version`, `site_url`, `mode` (`solo`/`cluster`),
  `database` (`sqlite`/`postgres`), `media` (`local`/`s3`), `schema` (`db_version`,
  `binary_version`, `pending` migration versions, `verdict` =
  `up_to_date|needs_up|newer_schema_ok|incompatible`, 02 §8), `theme` (`active`: the active
  row's name or `null`; `fallback_nodes`: names of live nodes with
  `nodes.meta.theme.fallback`), `jobs` (counts per status), `nodes` (`live`, `stale`: by
  heartbeat within 30 s of the database clock), `node` (`id`, `started_at` of the node
  that answered) and `mail` (`configured`: `[mail]` has `smtp_url` and `from`; `from`:
  `mail.from` or `null`; the SMTP URL is never shown, M2b-3 P49).
- `GET /system/nodes` lists every registered node, live and stale (`live` flag), by id,
  with its `meta`; unpaged (`next_cursor` always `null`, at most 1 000 rows). `meta` is
  typed (`NodeMetaDto`, M2b-3): `theme` (`name`, `hash`, `origin` = `"embedded"`,
  `"folder"` or `{ "artifact": <hash> }`, `error`, `fallback`), `plugins` (by name:
  `version`, `hash`, `status`, `error`), `plugins_error`, `permalinks_error`, `job_kinds`,
  `site_id`, `site_url`, `mode`, `clock_skew_ms`. Every member is optional and read
  leniently, for rolling deploys: unknown members are dropped and a member of the wrong
  type is `null` (a plugin entry that is not an object is left out) instead of failing
  the list.
- `GET /system/audit?actor&action&target_kind&target_id&limit&cursor` lists audit rows
  newest first with cursor paging (`limit` 1..=200, default 50; a cursor of another list
  is `400`), each with `actor` = `{ id, login }` (`null` for the operator path,
  unauthenticated flows and deleted users). Filters (M2b-3), combined with AND: `actor` (a
  user id), `action` (exact, or a prefix when it ends in `.`, so `user.` matches
  `user.create` but not `users.purge`; compared literally and case-sensitively, by
  `substr`/`length` on SQLite and `starts_with` on Postgres, no new index),
  `target_kind`, and `target_id` (only with `target_kind`, else `422` with
  `errors[0].field = "target_id"`). A malformed `actor`, or an empty or over-long
  `action`/`target_kind` (1..=100 characters) or `target_id` (1..=255), or one with
  control characters, is `400` naming the field. The cursor of a filtered page continues
  the same filters. `details` never hold secrets or setting values (P22: setting writes
  list keys only; plugin installs list the grant, never settings).
- `GET /system/jobs?status&kind&limit&cursor` lists jobs newest first (`limit` 1..=200,
  default 50; an unknown `status`, a malformed `kind` or a cursor of another list is
  `400`) **without payloads** (P19), `last_error` scrubbed like node diagnostics (F16),
  and `handled` = some live node lists the kind in `nodes.meta.job_kinds`.
  `POST /system/jobs/{id}/retry` (`system:write`) puts a `dead` job back (`queued`, due
  now, `attempts = 0`, `last_error` kept) and appends `job.retry` (`{id, kind}`) in the
  same transaction; any other status is `422` (`errors[0].field = "status"`), a missing
  job `404`. A job no node handles is retried too (`handled = false` in the answer).
- `GET /system/metrics/summary`: the answering node's `renders`, `page_cache` (`entries`,
  `bytes`, `hits`, `misses`), `plugins` (`calls`, `failures`) since it started, and the
  site's `jobs` per status (P20; 09 §6).
- `POST /system/mail/test` (`manage_system` + `system:write`; M2b-3 P49) queues one mail to
  the caller's own address and answers `202 { "to" }`. Without `[mail]` it is `422`
  (`errors[0].field = "mail"`); an account without a usable address `422` (`email`); 3
  per 10 minutes per user (bucket `mail-test:user:<id>`, `429` with `Retry-After`); a full
  or stopping mail queue `503 busy`. Subject `Test e-mail from <site title>`; the
  plain-text body names the site URL, the caller's login and the UTC time and says it was
  sent from the admin to check the mail settings. Audited `mail.test` (target the user,
  empty `details`). The send runs in the background: an SMTP failure is in the node's
  log, not in the answer.
- **Node diagnostics are scrubbed** (`ferrite_core::diagnostics::scrub`): every error text a
  node puts in `nodes.meta` (`theme.error`, `plugins.<name>.error`, `plugins_error`,
  `permalinks_error`) is scrubbed where it is written (`NodeStatus`) and again where it is
  read (`GET /plugins`, `GET /themes`, `GET /system/nodes`), so a row written by an older
  node in a rolling deploy leaks nothing either. Over-scrubbing is accepted, a leak is
  not (R16). Per whitespace-separated token: a URL (`<scheme>://…`) or `user:password@host`
  becomes `<url>`; the value of a credential key (`password`, `secret_key`, `access_key`,
  `token`, `key`, `bucket`, `X-Amz-Signature`, `Authorization` with the credential after
  `Bearer`/`Basic`, XML such as `<AWSAccessKeyId>`/`<BucketName>`), a bare AWS key id
  (`AKIA…`/`ASIA…`) and a quoted name after `bucket`/`NoSuchBucket` become `<secret>`; a
  path (`/a/b…` in any script, `//server/…`, `./`, `../`, `~/`, `X:\…`, `\\server\…`), also
  after a prefix such as `path=` or an ANSI escape, becomes `<path>` and runs on across
  spaces up to a quote, `)`, a line break, `: `, `, ` or the end; an IPv4 address (with
  `/prefix` or `:port`), an IPv6 address (bracketed or not) or `host:port` becomes `<addr>`
  (`file.html:3`-style template lines are kept); the result is cut to 1 KiB on a character
  boundary, in linear time over at most 16 KiB of input. Manifest and template problems
  stay readable. The full text is in the node's log only.

**Token scopes (P6).** A scope is `<area>:<read|write>` for the areas `content`, `media`,
`terms`, `menus`, `comments`, `users`, `settings`, `redirects`, `extensions`, `import`,
`system`; `write` implies `read`. `Actor` carries the scopes of the credential: a session
holds every scope, a token the ones it was created with (never more than its creator's
own). `require(actor, capability)` checks the role, then the capability's write scope;
`require_read(actor, area)` checks `read_admin`, then `<area>:read`. A refusal is `403
forbidden` whose detail names the missing capability (`missing publish_own`) or scope
(`missing content:write`). So an admin's `content:read` token lists drafts but cannot
create or publish, and a contributor's full-scope token still cannot publish.

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
| manage users, settings, themes, plugins, import, redirects, system | ✓ | — | — | — |

As implemented in `ferrite-core` (`src/actor.rs`, `src/content/lifecycle.rs`; M1c-1, with
the HTTP endpoints above since M2a-1):

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
- **Reading content (M2a-1 R17)**: every admin read needs `read_admin` and the
  `content:read` scope, then `can_read`: items publicly visible now (status published and
  `published_at <= now`, the public site's predicate; R23) are readable by every role,
  own items in any status by their author, and others' draft, pending, scheduled,
  private, trashed and future-dated published items only by roles with `edit_others`
  (editor, admin; it also stands for WordPress's `read_private_posts`). An item the actor
  may not read is `404`, never `403`. Lists and `q` search apply the same rule in the
  query, so pages and cursors stay exact. `publish` also accepts a scheduled item
  ("publish now"). `GET /content/{type}/counts` (M2b P14) counts with the same rule
  (one `count` per status with the list's `readable_by`), so an author's draft count
  never includes another author's drafts; an unknown type is `404`. Every item, list
  entry and revision names its author (`author`: id, login, display name; one
  `get_many` per response, M2b P13): only authors of what the actor already read, and
  logins and display names are public on author archives anyway.
- **Allowed actions (M2b P13)**: every item and list entry carries `allowed`, what the
  caller may do with the item now. `ferrite_core::content::allowed_actions` computes
  each flag as the permission check of the matching method (role, then the token's
  `content:write` scope) and the transition being legal from the item's status, with the
  same predicates the lifecycle calls (`content/allowed.rs`), so they cannot drift:
  `edit` = `can_edit` and not trashed; `publish`/`unpublish`/`schedule` per the rules
  above and the transition table; `submit` and `trash` per `can_edit`; `restore` and
  `delete` per R8 and only on trashed items. A `true` flag can still fail on a stale
  version, a schedule time in the past or (`delete`) children. The admin shows only the
  actions `allowed` names and never decides on its own.
- **Reading revisions (M2a-1 R26)** needs more than reading the item: the actor's role
  must be able to edit it (`role_can_edit`, the role half of the edit rule above, so a
  `content:read` token of such a role still reads them). Revisions keep text that was
  removed from the item, so a contributor or author gets `404` for the revisions of
  another author's published post, and a contributor for those of its own published
  post. WordPress likewise requires `edit_post` on the parent.

## 2. Auth

As built in M2a-1 (`ferrite_core::auth`, `ferrite_core::users`, `crates/ferrite-http/src/api/auth.rs`;
rulings P7–P9, P14–P17 in [`m2a.md`](../superpowers/rulings/m2a.md)).

- **Passwords** (`auth/password.rs`, P15): argon2id v0x13 with the `argon2` crate's
  defaults (m = 19 456 KiB, t = 2, p = 1), stored as PHC strings; rehashed on login when
  the stored parameters differ. Policy: at least 12 characters and at most 1024 bytes.
  Hashing and verifying run on tokio's blocking pool behind one process-wide semaphore of
  `max(2, available_parallelism)` permits, never inside a store `Tx`. An unknown login, a
  wrong password, an account without a password and a disabled account all cost one full
  verify (a dummy hash for unknown users) and give the same `401 invalid-credentials`.
  Imported WP users get a forced password reset (WXR exports contain no password hashes);
  verifying WP hashes (`legacy_hash`) is **later**, only if a direct WP-database user
  import is added. An account with `must_reset` gets a session that may only call `GET
  /auth/me`, `POST /auth/logout` and `POST /auth/password` (else `403
  password-reset-required`). `GET /auth/me` (and the login answer's `user`) carries
  `mail_configured` (M2b-3): whether `[mail]` is set, for roles holding `manage_users` or
  `manage_system` whatever the credential's scopes, `null` for other roles. Its API tokens are not restricted (R26): they are
  independent credentials that a reset keeps (R22), so a bot account keeps working
  after an admin resets its password.
- **Logins** (P14, R3): `^[a-z0-9][a-z0-9._-]{0,59}$` after trimming and ASCII
  lowercasing, immutable after creation, so `/author/<login>/` never moves. A login input
  containing `@` is looked up as an e-mail. E-mails are unique by the ASCII-lowercased
  `email_key`, so non-ASCII case variants (`É@x`, `é@x`) are distinct accounts.
- **Sessions** (P7; DB-backed, [04 §5](04-cluster.md#5-sessions)): the session cookie
  holds 32 random bytes as base64url (43 characters); the database stores only its SHA-256
  hex. Cookie attributes: `HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000` (30 days),
  plus `Secure` **only when `site.url` is `https://`**, so local `http://localhost`
  development keeps the cookie (an unconditional `Secure` would drop it). The name follows
  the same rule (M2a-2b P23): `__Host-ferrite_session` on an `https` site (browsers accept a
  `__Host-` cookie only with `Secure`, `Path=/` and no `Domain`, so a sibling subdomain or
  a plain-http response cannot plant or overwrite it), `ferrite_session` on an `http` one.
  The scheme is `site.url`'s, not the request's, so a TLS-terminating proxy changes
  nothing. Only the site's name is read (the API and the public site alike): there is no
  transition, so sessions issued under the old name on an `https` site log in once more
  after the upgrade.
  Idle timeout 7 days, absolute 30 days, `last_seen_at` written at most every 5 minutes. No
  per-node cache: every request reads its session row, so a revocation applies on every
  node at once. Sessions are deleted on logout, on a password change (the API then issues a
  new cookie for the caller), on a role change, disable and delete, by `ferrite user
  reset-password`, and when TOTP is enabled (all but the confirming session, R26). A
  cookie sent with a successful login is deleted server-side (no session fixation).
  A user manages their own sessions (M2b-3 P45; cookie sessions only: a bearer request is
  `400 bad-request`, as for logout; `users:write`, which a session always holds):
  `GET /auth/sessions` → `{ items: [{ id, created_at, last_seen_at, current }] }`, newest
  first, the newest 100 read and those past their end or idle limit left out;
  `DELETE /auth/sessions/{id}` → `204` (the current session `422`, sign out instead; an id
  that is none of the user's sessions `404`); `POST /auth/sessions/revoke-others` →
  `{ revoked: n }`, keeping the current one. The `id` is the first 16 hex characters of
  SHA-256 over the stored key (itself the SHA-256 of the cookie), computed on read (no
  column, no migration): it names a session to its owner without revealing the stored key
  or the cookie. No user agent or address is shown, so sessions are told apart by time.
  Both revocations are audited `session.revoke` (target the user; `details.session` = the
  id, or `details.count`; a revoke-others that ended none writes no row).
- **CSRF and origin** (P8): a cookie-authenticated request with a method other than GET or
  HEAD needs `X-CSRF-Token` = base64url(HMAC-SHA256(session `csrf_secret`,
  `"ferrite-csrf:v1:" + id_hash`)) and an `Origin` equal to the `site.url` origin (without
  `Origin`, a same-origin `Referer`; neither: refused; `Origin: null` never matches), else
  `403 csrf` before the body is read. The token comes with the login response
  (`csrf_token`), `GET /auth/me` and a password change. `POST /auth/login`,
  `/auth/login/totp` and the two password-reset endpoints check the origin too (login
  CSRF); they are the only unsafe operations open without credentials, which the route
  sweep test enforces (M2a-2 P28). Bearer requests are exempt.
  There are no CORS headers: the admin is same-origin and headless clients use tokens.
- **API tokens**: `Authorization: Bearer fer_<43 base64url characters>` (32 random bytes),
  stored as SHA-256 hex, shown once at creation, scoped (§1 "Token scopes"), optional
  `expires_at`, `last_used_at` written at most every 5 minutes, no CSRF. A user holds at
  most 50 tokens; the cap is counted inside the creating `Tx` after locking the owner's
  row (`get_for_update`), so concurrent creates for one user queue behind each other (F31
  had accepted a soft cap; with the lock the count cannot be overshot, though no
  concurrency test covers it). A request with an
  `Authorization` header never falls back to the cookie. Tokens are independent
  credentials (like WordPress application passwords): a password change or `ferrite user
  reset-password` keeps them (R22); disabling or deleting the user removes them, and they
  are revoked one by one over the API.
- **Rate limiting** (P5, `auth/limits.rs`): fixed one-minute windows counted in the
  `rate_limits` table by one upsert on the database clock, so every node shares the count.
  Login: 10/min per IP hash and 5/min per login input; TOTP codes (login step, enrollment
  confirm): 5/min per user; unknown or expired bearer tokens: 10/min per IP hash.
  Over the limit: `429 rate-limited` with `Retry-After` (seconds to the window end on the
  database clock, 1..=60), checked before the password, so a limited client learns nothing.
  IPv6 addresses are limited per /64. The address is the TCP peer unless the peer is in
  `server.trusted_proxies` ([09 §5](09-operations.md#5-tls)): **behind a reverse proxy that
  is not listed there, the per-IP limits are one bucket for the whole site**, so 10 login
  attempts a minute from anyone block every login. A request with `X-Forwarded-For` from
  an untrusted peer logs a warning, once per peer address (R26). Login buckets hold a
  keyed HMAC of the trimmed, ASCII-lowercased input, never the text; a login and its
  e-mail address are **separate budgets**, so one account takes at most 2 × 5 = 10
  password guesses a minute (keying on the resolved user id would reveal whether the
  account exists). The current-password checks of `POST /auth/password` and TOTP
  confirm/disable go through the same per-login bucket (R11, R15). Password-reset requests
  (M2a-2 P19): 5/min per IP hash, then 3 **per hour** per input (keyed like the login
  buckets, own label `ferrite:reset-bucket:v1`), before any lookup. Fixed windows allow up
  to 2× the limit across a window edge. On
  SQLite the clock is the process clock: if it jumps back, a bucket stays in its (now
  future) window until time catches up and keeps counting meanwhile; `Retry-After` stays
  within one window. Windows older than 1 hour are pruned by `sessions.prune`.
- **TOTP** (P9): RFC 6238 SHA-1, 6 digits, 30 s steps, skew 1, 20-byte secrets. `POST
  /auth/totp/setup` returns the secret (base32), an `otpauth://` URL and a sealed
  `enrollment` (valid 10 minutes, nothing stored); `POST /auth/totp/confirm` with the
  enrollment, a code and the current password stores the secret sealed under the site
  key and deletes the user's other sessions in the same `Tx` (R26; the confirming
  session is kept, through a token every session ends, API tokens are kept); a bad,
  someone else's or expired enrollment is `422` with `errors[0].field` = `enrollment`, a
  wrong or replayed code `422` with `field` = `code`, and TOTP already on at confirm a
  `422` without a field; setup while TOTP is on is `422` (disable first). A correct password for an account with TOTP
  answers `{ "status": "totp_required", "challenge" }` (a sealed blob, valid 5 minutes,
  bound to the current password hash) instead of `{ "status": "session", "user",
  "csrf_token" }`; `POST /auth/login/totp` finishes it. A code's step must be newer than
  `users.totp_last_step` (one conditional update), so a code cannot be replayed. The
  issuer is the site title at setup time (the `site.title` option, else `ferrite.toml`;
  a blank or whitespace-only option counts as unset; M2b-3 P51) without `:`
  (`FerriteCMS` when empty). There are no recovery codes: an admin turns another user's
  TOTP off with `POST /users/{id}/totp/disable` (`manage_users` + `users:write`, never
  oneself, their sessions kept, audited `user.totp_disable`; M2b-3 P43), or an operator
  runs `ferrite user disable-totp --login <login>`.
- **Site key** (P4, R1; [01 §4](01-architecture.md#4-configuration), §8):
  `auth.secret_key[_file]`, or in solo mode `<data>/secret.key`; cluster mode refuses to
  start without a configured key. It seals TOTP secrets, login/enrollment challenges and
  password-reset tokens and keys the IP and login/reset-bucket hashes. Sessions and CSRF
  tokens use per-session secrets, so a lost or changed key logs nobody out but makes
  stored TOTP secrets unreadable (users then re-enroll after `ferrite user disable-totp`)
  and outstanding reset links invalid.
- **Zeroization** (M2a-2b P24, `zeroize`): `SiteKey` (every clone) and its subkeys, the
  HMAC key state (`hmac` and `sha2` with their `zeroize` features), the prepared HMAC key
  block, opened TOTP secrets, a new enrollment's random secret, totp-rs's copies of the
  secret (`Totp`, its builder; `totp-rs`'s `zeroize` feature) and `Secret` (password hashes,
  TOTP secrets, a new account's password) are wiped when dropped. API password fields (login, password change, reset
  confirm, TOTP confirm and disable, user create, an admin's password set) are
  `api::dto::Password`: deserialised through a `String` held in `Zeroizing`, redacted in
  `Debug`, `format: password` in the OpenAPI document; hashing and verifying wipe the
  copies they hand to the blocking pool. **Not wiped**: serde_json's copy of the request
  body (and its scratch buffer for escaped strings), hyper's buffers, argon2's working
  memory, the CLI's password strings, a TOTP enrollment's secret as it is shown and sealed
  (`secret_base32`, `otpauth_url`, the sealed-enrollment JSON), and copies the allocator
  made when a `String` grew.
- **Passkeys** (`webauthn-rs`): deferred past M2 (P3; the newest `webauthn-rs` is a
  pre-release, and the ceremony needs the SPA). There is no `passkeys` table.
- **Audit log** (P22): user, token, TOTP, session revocation (`session.revoke`, M2b-3),
  type/taxonomy and settings writes, extension
  writes through the API (plugin install/enable/disable/delete/settings, theme
  install/activate/settings; M2a-2a) and a completed password reset
  (`user.password_reset`, no actor) append a row to `audit_log` in the same `Tx` (no
  secrets or setting values in `details`); failed logins, reset requests and the operator
  CLI's extension commands are not audited. There is no retention job and no read
  endpoint yet (`GET /system/audit` is M2a-2b).
- **Password reset by e-mail** (M2a-2a, P19; `auth/reset.rs`), needs `[mail]`:
  - `POST /auth/password-reset { login }` (login or e-mail) answers `202` with no body
    whether the input names an active account, a disabled one or none, after the rate
    limits above (`429`); without `[mail]` it is `503 mail-unavailable`. Only an active
    account gets a mail, to its own address, queued on the node's mail queue (16 sends
    in flight, 10 s each), so the response never waits for SMTP; a queue or SMTP failure
    is logged, never returned. Requests are not audited.
  - The mail names the site by its title at request time (the `site.title` option, else
    `ferrite.toml`; M2b-3 P51): subject `[<title>] Password reset`.
  - The mail links to `<site.url>/admin/reset-password#token=<token>`: the token is in the
    **fragment**, which browsers send to no server and leave out of `Referer`. That page
    is the admin SPA's (M2b); until then the link is a `404` and the token is posted to
    the confirm endpoint directly (F33, accepted).
  - The token is stateless: `seal(site key, "password-reset", { user, exp: now + 1 h, fp
    })`, `fp` = 32 hex of HMAC(subkey `ferrite:reset-pw:v1`, the current password hash),
    compared in constant time. Any password change (the reset itself included) changes
    `fp`, so a token works once and dies with a change made another way.
  - `POST /auth/password-reset/confirm { token, password }` → `204`: the password policy
    first (`422`, `errors[0].field = "password"`), then the token (`422`,
    `errors[0].field = "token"`, "the reset link is invalid or has expired": bad, expired,
    used, or of a disabled or deleted account). The new password is hashed outside any
    `Tx`; then one `Tx` locks the user row, re-checks status and `fp` there (two
    concurrent confirms with one token cannot both pass, F31), writes the hash, clears
    `must_reset`, deletes every session and appends one `user.password_reset` audit row
    without an actor or the token. No session is issued.
  - `fp` covers the password hash, not the account status, which is checked when the
    link is used: a link issued before the account was disabled works again if the
    account is re-enabled within the link's hour (accepted; the password is unchanged
    meanwhile, and a re-enabling admin can reset it to kill the link).
  - Confirm has no rate limit on purpose: tokens are AEAD-sealed under the site key and
    cannot be guessed, a bad, expired or used token is refused before any password work,
    and argon2 runs only for a token that is valid for the current password hash.
  - API tokens are kept (R22) and **TOTP is kept**: a reset never bypasses or disables the
    second factor; a lost authenticator still needs an admin (`POST
    /users/{id}/totp/disable`) or `ferrite user disable-totp`.

  An operator can also reset a password with the CLI: `ferrite user reset-password --data
  DIR --login <login> [--password-stdin]` ([09 §8](09-operations.md#8-cli-summary)); it
  ends the user's sessions and keeps their API tokens.

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

- `admin/` builds to `admin/dist/` (Vite `base: "/admin/"`). `crates/ferrite-http`
  embeds it with `rust-embed` (workspace pin 8.13, `debug-embed`):
  `ferrite_http::admin::EmbeddedAdmin` (`#[folder = "../../admin/dist/"]`,
  `#[allow_missing = true]`) behind the `AdminAssets` trait. Tests swap the set with
  `AppState::with_admin_assets` (`test-seams`, 10 §2).
- `build.rs` does **not** run pnpm (keeps `cargo build` hermetic); CI builds the admin
  first. It only tells Cargo what to watch: `admin/dist` when it exists, else `admin`
  when that exists (a new `dist/` changes it), else only `build.rs` — a missing
  watched path would make Cargo rebuild the crate on every invocation (M2b-1 F1); if
  neither existed at the last build, a later `admin/` needs `touch
  crates/ferrite-http/build.rs` once. A
  missing `dist/` compiles an empty set, and every page path then answers a static
  placeholder page ("FerriteCMS admin not built", naming
  `pnpm -C admin install && pnpm -C admin build` and a rebuild; no script).
- Routes (`admin_routes`, in `build_site` and `build_app` inside the shared layers, in
  front of the public fallback: admin paths never reach redirect rows or the page
  cache; `admin` is in `slug::RESERVED`, so no redirect row can be written under it):
  - `GET|HEAD /admin` → `308` to `/admin/` (query kept). Other methods on any admin
    path → `405` with `Allow: GET, HEAD`.
  - The rest of the path is percent-decoded once; a path that is not UTF-8 or holds a
    `.`/`..` segment, a backslash or a NUL is a plain `404` (F3).
  - Under `assets/`: the file or a plain `404`. Elsewhere: the file if it exists and
    its last segment has an extension (`favicon.svg`, `robots.txt`); a missing path
    whose extension is in the content-type table (`html`, `js`, `css`, `woff2`, `svg`,
    `png`, `ico`, `json`, `txt`, `webmanifest`) is `404`; anything else is
    `index.html`, the SPA fallback (the placeholder when none is embedded).
- Caching: `assets/*` `Cache-Control: public, max-age=31536000, immutable`;
  `index.html`, the placeholder and other root files `no-cache`. Every file has a weak
  `ETag: W/"<first 16 hex of SHA-256>"` (the compression layer may re-encode text
  formats, F2), `If-None-Match` (weak comparison) → `304`, and `Vary: Accept-Encoding`.
  Text formats go through the shared compression layer; fonts, images and unknown
  types (`application/octet-stream`) are not compressed.
- Headers on every admin response (errors and the redirect included):
  `Referrer-Policy: no-referrer` (the password-reset link carries its token in the
  fragment; P24), `X-Frame-Options: DENY`, `Cross-Origin-Opener-Policy: same-origin`,
  `X-Content-Type-Options: nosniff`,
  `Permissions-Policy: camera=(), microphone=(), geolocation=()`,
  `X-Robots-Tag: noindex`, and the CSP (`admin_csp`):
  `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src
  'self' data: blob: https:[ <media origin>]; font-src 'self'; connect-src 'self';
  frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self';
  object-src 'none'; worker-src 'self'`. `<media origin>` is the origin of
  `[media] s3.public_base_url` (Direct S3 mode; `HttpConfig::media_origin`, set by
  `serve`), else absent. `https:` covers external body images (F5; `no-referrer` keeps
  the admin path from the remote host). `'unsafe-inline'` styles are needed by the
  `<style>` elements sonner, Radix and the shadcn chart inject (P8); there is no
  inline script and no `eval` (Zod runs jitless).
- Dev: `pnpm dev` (Vite on :5173) proxies `/api` and `/_preview` to `ferrite serve` on
  :8080.

### Screens (M2)

| Screen | Status | Notes |
|---|---|---|
| Sign-in and account recovery | Built (M2b-1) | **As built (M2b-1 Task 9):** sign in (login or e-mail, the host of `site_url` on the card, F22), TOTP code (`/login/totp`; a wrong or expired code is a 401, F21), the forced password change of a `must_reset` session (the only screen it reaches), forgot password and the reset page (`#token=…` read once and stripped with `history.replaceState` before anything renders or is sent; every admin response has `Referrer-Policy: no-referrer`, P24); `?next=` only for same-origin paths under `/admin/` |
| Account | Built (M2b-3) | **As built (M2b-3 Task 6):** `/admin/account`, every role (a `must_reset` session still reaches only the forced change), from the user menu and the palette ("Account"). Profile: display name and e-mail (`PATCH /users/{me}`); a different e-mail asks for the current password (P44; `403` shows "Wrong password" at the field, `429` the wait); login, role, created and last sign-in read-only (`GET /users/{me}`); `qk.me` refreshed after a save. Password: current, new and repeat (`changeSchema`); the session is rotated and the new CSRF token kept; the toast says other sessions were signed out. Two-factor: Set up opens a two-step dialog (the QR code of `otpauth_url` drawn in the browser, the key in groups of four with Copy; then the six-digit code and the password; a wrong code or password inline, an expired enrolment offers Start again; success says other sessions were signed out); Turn off asks for the password. API tokens: a table of name, scopes, created, last used and expiry; New token offers only the scopes the session holds as one None / Read / Write row per area and Never / 30 / 90 / 365 days (`expires_at` in RFC 3339 UTC); the value is shown once with Copy; Revoke confirms; the server's `422` (50 tokens at most) shows in the dialog. Sessions: created and last active, "This session", Sign out per other row and "Sign out all other sessions". The token value, the TOTP secret and every password are sent with plain calls (never TanStack mutations) and held only in the dialog's or form's state, so none reaches a URL, `history`, storage or the query and mutation caches (E2E `token_value_is_shown_once`, `totp_secret_not_kept`) |
| Shell, palette, shortcuts | Built (M2b-1) | **As built (M2b-1 Tasks 10–11; 11 §6, §7):** sidebar from `me.capabilities`, top bar with the New menu and the account menu (theme, density, sign out), route transitions ≤ 200 ms, not-found / forbidden / error states, the `?` shortcut sheet, and the `⌘K` palette (navigation, creation, preferences, Recent, live content search debounced 150 ms with up to 8 hits per type); the styleguide at `/admin/_styleguide` behind login (P12) |
| Dashboard | Built (M2b-1) | **As built (M2b-1 Task 12; P16 shows only data that exists):** post count tiles (Published, Drafts, Pending review, Scheduled from `GET /content/post/counts`, each linking to the filtered list; Published with a 30-day sparkline and the number published in that window); publishing activity (posts and pages published per day over 30 days, bucketed in the browser's time zone from at most 1 000 published items per type, `status=published&order=published&limit=200`, ≤ 5 pages; a note when the cap is hit); the actor's 5 latest drafts of posts and pages (`author=<me.id>`); needs attention (pending review with `edit_others`; dead jobs, a schema verdict other than `up_to_date` and theme fallback nodes with `manage_system`; plugins disabled after failures with `manage_plugins`); site health with `manage_system` (page-cache hit ratio, node-local and labelled "This node only"; jobs by status; plugin failures per hour over 24 h stacked by plugin, at most 10 enabled plugins, labelled "All nodes"). Each card loads and fails on its own, and a card the actor may not see is never requested. **Later:** page views (core lightweight counter, privacy-friendly; full analytics is a plugin) move to M4 with the metrics time series; plugin hook p95 latency per plugin with the Plugins screen (M2b-3) |
| Posts / Pages / custom types | Built (M2b-1) | **As built (M2b-1 Task 13; 11 §7):** status tabs with counts (`GET /content/{type}/counts`), search, Mine (`author=<me.id>`), a term filter per taxonomy and sort in the URL; a table with the slug under the title, parent (hierarchical types), status, author and date; hover actions from `allowed`; 50 a page with "Load more" and auto-load; bulk publish/restore/trash/delete fanned out 4 at a time with per-item `If-Match` and a failure report; trash with Undo (on the item's own type) that re-applies the previous status, or a confirm first when nothing trashed could be restored by the actor (`trash_undoable`); delete permanently otherwise the only confirm; animated row insert/remove |
| Editor | Built (M2b-1); media, SEO and custom fields Built (M2b-2) | **Side panel and publishing as built (M2b-1 Task 15):** a right panel (a sheet below 1024 px; `mod+.`) with status, publish/schedule time and author, and actions only from `allowed` (Publish, Schedule, Update, Submit for review, Save draft; Unpublish/Unschedule, Preview); a publish dialog (`mod+shift+p`) with a summary and "Now"/"Later" (calendar + 24 h time on the browser's clock, its zone and the UTC instant shown, sent as RFC 3339 UTC; past times refused); slug with the resulting permalink and copy link; excerpt; one section per taxonomy (checkbox tree or tag combobox, inline creation with `manage_terms`); parent (`?q=` search) and `menu_order` for hierarchical types; Move to trash with Undo. Panel, title and body are one draft: a save PATCHes only the changed members, and a transition with unsaved changes PATCHes first and then transitions with the returned version (F40). **Excerpt limitation (F39):** the server stores a derived excerpt as plain text, so the admin cannot tell a derived excerpt from a written one; the field shows the stored text, "Derive from body" sends `excerpt: null`, and a later body edit keeps the old derived text until it is derived again (a "derived" flag would be an API change). **Editor safety as built (M2b-1 Task 16):** autosave every 10 s while the draft changes into the user's slot (`PUT …/autosave`, at most one request in flight, flushed when the tab is hidden or the screen is left, retried after going offline; an explicit save waits for a running `PUT` and none starts until its `PATCH` is done); the save indicator says Saved / Autosaved / Unsaved / Saving / Offline / Not saved; a slot that differs from the item opens as the working draft with a restore banner (Discard, with Undo); one made from an older version says so and its save goes with that version, so the server answers 412; on a 412 a dialog puts both versions side by side (title, excerpt, body as a word diff, address, terms, parent, order) with Keep mine (re-`PATCH` with `If-Match` = their version), Take theirs (mine stays in the slot, offered back) and Copy my text; a transition that waited for the refused save does not run and the dialog says so; leaving with unsaved changes asks Save / Discard / Stay (router navigation) and the browser asks on close or reload only while the slot does not hold them yet (F38); a `401` keeps the draft in memory, goes to sign in with `next` and back, and the draft is written to the slot. **Follow-ups as built (M2b-2 Task 14):** while a kept copy (Take theirs) holds the slot, new unsaved edits are backed up in this browser's `localStorage` (`ferrite:backup:<item>:<user>`, JSON with `saved_at`, at most once a second; a failed write is a `console.warn` and the page still holds the edits), cleared on a successful save, on Discard (the kept copy's, the restored draft's or the leave dialog's), on Restore of the kept copy or a revision, and on sign-out (every backup of the account); on opening, a backup that differs from both the item and the slot is offered in a second restore banner, "Unsaved edits from this browser, <time>", with Restore edits (what the slot or the restored draft holds stays kept, the backup becomes the working draft under the kept banner; without a slot it is written there) and Discard (R13, P40). `mod+shift+p` on `/new` saves the new item (or joins the save in flight) and the hand-over to `/$id` carries `openPublishDialog`, so the publish dialog opens there. Relation fields, menu targets and the comments queue's item chip name items by id through `GET /content/refs` (lookups started together go out as one request, any type; an item of a type the sidebar does not show is still named). **Revisions as built (M2b-1 Task 17):** a drawer (`mod+alt+r`, or "Revisions (n)" in the panel when `allowed.edit`) lists up to 50 revisions newest first, grouped by day, with author and `restored from …` notes; the selected one is shown against the current item (or the previous save) as a word diff, underlined = brought back, struck through = removed, with visually hidden "added:"/"removed:" text (R17); Restore sends `If-Match`, asks first only over unsaved edits, goes through the 412 flow, and its toast offers Undo (restores the revision before the restore's own snapshot, not offered when someone saved in between); phones get a list/detail view. **Media, fields and SEO as built (M2b-2 Task 8):** a media picker dialog (`openMediaPicker`, mounted once in the shell) with a Library tab (search, the library grid as a single-select listbox, 50 a page with Load more; Enter or a double click picks) and an Upload tab (with `upload_media`; the library's queue, 3 at once, one `Idempotency-Key` per file; closing it while uploads run asks first); a Featured image section in the panel for every type (`fields.featured_image`, a UUID or `null`; images only) with Replace and Remove; inline images from the slash menu's Image or the bar's image button (hidden below `sm`), stored as `image { src: "ferrite:media/<id>", alt }` and rendered by the site with its alt text, from the original `/_media/<hash>/<name>` until M2b-3 and since then as a responsive image with `srcset` (06 §4 "Inline images"); image files dropped on the body upload through the queue into a placeholder that follows edits and becomes the image when done (leaving while they run asks); an Alt text popover on each image ("Decorative" when empty). Custom fields from the type's schema (text, rich text, number, date and time, yes/no, choice, link to item, media, repeater up to 200 rows and two levels), in the panel or, on a wide screen with more than 4 fields, as a card under the body; core's rules checked before a save (a refused save opens the panel or card and focuses the first invalid input); numbers take only `.` as the decimal separator (R30); server `422` messages at their path; keys the schema no longer has, at any repeater depth, listed ("Not in the type any more: …, removed when you save") and dropped on save (P39, R29); a type without a schema shows no fields section (P36); a relation searches every type the nav shows (P37). An SEO section: SEO title and description (hard limits 200 and 500, counters against 60 and 160), a search-result preview and Reset to defaults (`seo`, P28). Fields and SEO are part of the draft, autosave, the 412 dialog (fields listed by name) and the revisions drawer (each changed field as "name: before → after", `null` differs from absent; SEO is not kept in revisions). |
| Media library | Built (M2b-2) | `/admin/media`: grid (default, the viewer's last choice remembered) and list, search (titles only; none in Trash), kind chips, Mine, infinite paging (50 a page); the whole page takes dropped files; an upload tray (at most 3 uploads at once per tab, each with a fixed `Idempotency-Key`; a `503 busy` or a lost connection retried once by itself after `Retry-After`, else 2 s; a failed row says why in plain words with the server's detail under it, and offers Retry for a lost connection, `429`, `503`, other `5xx` and any unrecognised failure, Dismiss for `403`, `413`, `415` and `422`), leaving during an upload asks; a detail sheet (`?id=`) with preview, Title and Alt text saved on blur and `mod+s` with `If-Match` (a `412` shows the server's value), facts, URL copy, variants (polled every 3 s for up to 60 s); bulk trash with Undo; the Trash view with Restore and Delete permanently; actions by capability (`MediaDto` has no `allowed`), "Upload media" in the ⌘K palette; the uploader from `MediaDto.author`; the grid is a multi-select listbox (`aria-selected`, Space or `x`) |
| Content types | Built (M2b-2) | **As built (M2b-2 Task 9):** `/admin/structure/types` (`manage_settings`; palette "New content type"): every type but `media`, built-ins first; label with the name in mono, hierarchy, archive, field count ("Any" without a schema); "New type" opens a dialog (label, a name derived until typed, `[a-z][a-z0-9_-]{0,31}`, `refs` reserved (R36), hierarchical, archive) and goes to the type. `/admin/structure/types/$name`: Settings (label, archive; name and hierarchy fixed), the field builder and Delete type (confirm; the server's refusal, built-in or has items, shown inline). The builder is a sortable list (drag handle, keyboard, or the row menu's Move up/down) of rows with kind icon, label, name, kind, Required and New/Kind changed badges; a row opens an editor in place: label, name (follows the label while new), kind (text, rich text, number, date and time, yes/no, choice, link to item, media, repeater; a kind change warns), required, a sortable option list for a choice (Enter adds the next, at most 100), sub-fields for a repeater (two levels). Core's limits (100 fields, 64-character names, 200-character labels) are checked before the save; a save that removes, renames, newly requires or changes the kind of a field lists those first and says stored values stay until each item is saved; then the stale guard (P31), then one `PATCH` (`fields` only when the schema changed); `mod+s` saves, leaving with unsaved changes asks. |
| Taxonomies | Built (M2b-2) | **As built (M2b-2 Task 10):** `/admin/structure/taxonomies` (`manage_settings` or `manage_terms`): label, Built-in badge, name, Nested/Flat, the content types, a term count; "New taxonomy" (`manage_settings` only; label, derived name, nesting, content types, never `media`). `/admin/structure/taxonomies/$name`: Settings (label and content types, through the stale guard; Delete taxonomy, whose confirm explains and offers no delete while it is built-in or holds terms) and New term (`manage_terms`). A nested taxonomy is a tree (siblings by name, P35, no manual order): drag (pointer or keyboard) or Move to… changes only the parent with a `PATCH`, applied at once and rolled back with a toast on failure (a cycle has its own message); row menu Edit, Add sub-term, Move to…, Delete; up to 64 levels. A flat taxonomy is a table with a client-side filter. Terms are edited in a sheet (name, slug, description, parent) through the stale guard; a term with sub-terms cannot be deleted (the dialog names the count and points to Move to…, R31), a leaf is deleted after a confirm. Without `manage_terms` the tree is read-only. |
| Menus | Built (M2b-2) | **As built (M2b-2 Task 11):** `/admin/structure/menus` (`manage_menus`; palette "New menu"): the theme's locations (`GET /menus` `locations`) with the menu each shows and Edit, or "Create menu" for an empty one; other menus below; New menu (label, a name derived until typed and checked like core, an existing name refused before the `PUT`). `/admin/structure/menus/$name`: Add items (Content: search over the types the nav shows, 8 per type, multi-select; Terms: per taxonomy with a filter; Custom link: `http(s)://` or a site path, no `mailto:`, R32) and a sortable tree (drag, keyboard, row menu Edit, Move up/down, Indent/Outdent, Remove) up to 16 levels and 500 entries; each row names its target (through `GET /content/refs` and `GET /terms/{id}`) or "Missing". Remove moves children up and offers Undo inside the unsaved draft. The whole tree is saved with one `PUT` after the stale guard (`mod+s`; Discard; leaving with unsaved changes asks); Delete menu confirms (naming the location it empties) and offers Undo (re-`PUT`, P38). |
| Comments | Built (M2b-2) | **As built (M2b-2 Task 12):** `/admin/comments` (`moderate_comments`; `g c`; a pending-count badge on the nav item, refreshed every 60 s and after each moderation; a "Comments" check on the dashboard's Needs attention): status tabs Pending (default), Approved, Spam, Trash with counts (`GET /comments/counts`), an item filter chip (`?item=`, named through `GET /content/refs`), 50 a page. Each row shows author (Account badge), e-mail, website (a link only for `http(s)`, `rel="noopener noreferrer nofollow ugc"`), the first 8 characters of the address hash, time, "Reply to …", the body as text (Show more past 6 lines), the item (editor link, View on the site). Actions per status (Approve, Unapprove, Reply, Spam, Not spam, Trash, Restore, Delete permanently) on a row, on a selection (fanned out 4 at a time, failures listed) or by key (`a`, `u`, `s`, `#`); changes are optimistic with Undo for 10 s; Delete permanently confirms (replies go too). Reply ("Approve and reply" on pending) posts through the public submit as the signed-in moderator, who is not rate limited (R34). |
| Users | Built (M2b-3) | **As built (M2b-3 Task 7):** `/admin/users` (`manage_users`; Administration in the sidebar, `g u`; the palette's "New user"). List: avatar, display name and login, e-mail, role, status (Active, Disabled, "Must change password"), a two-factor badge and the last sign-in; 50 a page with Load more; search `?q=` (250 ms after the last key) and the role and status chips in the URL. New user (`?new=true`): login, e-mail, display name and role; "Set a password" sends it with `must_reset: true` (never shown back), "Send a reset e-mail" (only when `me.mail_configured`) creates the account with `must_reset` and no password, then `POST /auth/password-reset` for its login and says where the link went. The sheet (`?id=`): profile (display name, e-mail), role and status, each saved through `staleGuard` (P31: a change by someone else asks Reload / Overwrite); a role change and a disable or enable confirm first and say that sessions end and (disable) tokens are revoked (H-A4); the server's refusals, such as the last admin (H-A2), show in that dialog; Set password (`POST /users/{id}/password`: they must change it and their sessions end); Turn off two-factor (P43, confirmed); their API tokens with Revoke (new tokens only in Account); Delete. One's own row (H-A1) has no role, status, password, two-factor or delete controls and links to Account. Delete confirms, then `DELETE`; a `422` with field `reassign` turns the same dialog into "This user owns content. Reassign it to:" with a picker of the other active users (`GET /users?status=active&q=`) and retries with `?reassign=<id>` (P46); other refusals show their detail there. Passwords go out with plain awaited calls and live only in the open dialog's state |
| Themes | Planned (M2b-3) | Upload, activate, settings form generated from `theme.toml [settings]` |
| Plugins | Planned (M2b-3) | List with status/failures; **install flow shows the permission report** ("This plugin can: read posts and pages; write meta `seo.*`; connect to `api.indexnow.org`") with high-risk items highlighted; per-plugin metrics chart |
| Redirects | Built (M2b-2) | **As built (M2b-2 Task 13):** `/admin/structure/redirects` (`manage_redirects`; palette "New redirect"): search (`?q=`, source or target), a table of From, To, code, hits, last hit and origin (Manual, Import, Slug change), 50 a page (a visit shows in hits and last hit only after the node's next hit flush, every 10 s, 06 §6); New redirect / Edit in a dialog (core's source and target rules, codes 301, 302, 307, 308 and 410; an existing source shows a Replace notice; edits through the stale guard); Delete with Undo for 10 s (re-created as manual, hits start at 0, P38). Import: a CSV dialog (at most 1 MiB and 10 000 rows, checked before sending) with a preview of the first 5 rows and header detection; the server's line errors shown as text; Export is a download of `GET /redirects/export`. |
| Import | Planned (M3, with the import API; M2b P25) | WXR upload, progress (job polling), link to import report |
| System | Planned (M2b-3); backups M4 (M2b P25) | Versions, schema status, backends, nodes (cluster), jobs (retry dead), backups (solo) |
| Settings | Built (M2b-3) | **As built (M2b-3 Task 8):** `/admin/settings/<group>` (`manage_settings`; Administration in the sidebar, `g s`; `/admin/settings` opens General; an unknown group is the not-found screen). One page with a rail of groups (a scrolling row of tabs on phones). **General**: site title, tagline, language (empty: the `ferrite.toml` value, sent as `null`; core's limits, 200 and 1000 characters without control characters and a 1..=35 character `[A-Za-z0-9_-]` tag, checked first). **Reading**: the home page shows "Latest posts" or "A page", picked by searching published pages (`GET /content/page?status=published&q=`) and named through `GET /content/refs` (a page that is not published or no longer exists says so); posts per page 1..=100 (empty: 10); discourage search engines, with a hint naming `robots.txt`. **Permalinks**: post presets "Post name" `/%slug%/`, "Month and name" `/%year%/%month%/%slug%/`, "Category and name" `/%category%/%slug%/` and Custom, with token chips; the page structure (must hold `%parent_path%`); a live preview of a sample post and sub-page URL (`settings/permalink-preview.ts`, core's parse rules and encoding); a change confirms first that URLs move and no redirects are created (P58). **Comments**: allow comments; guest moderation `known` / `all`, each explained; threads per page 1..=100 (empty: 50). These four save only their own changed keys with one `PATCH /settings`, through the stale guard over those keys (P31, P58); `mod+s` saves, leaving a group with unsaved changes asks, and a `422` shows at its field. **Media sizes** (read-only, P50): the active theme's `images` (name, width × height, "auto" without a height, cropped or scaled, sizes the processor skips marked "Not made"), "Set by the theme <name>", and a note that changing sizes means changing the theme; needs `manage_themes` to read. **Mail** (read-only, P49): set up or not and the From address (`GET /system/status`; without `manage_system`, `MeDto.mail_configured`); "Send test e-mail" to my own address (`manage_system`); without mail, a `[mail]` example for `ferrite.toml` and a restart note. |

### UX principles

The design language (fonts, colour tokens, density, motion, keyboard map, components
and the review checklist) is [11-admin-ui.md](11-admin-ui.md); the principles below
are the summary.

- Every destructive action has undo (trash, revision restore) rather than confirm
  dialogs where possible.
- Motion is used for meaning (list reorder, panel open, page transitions ≤ 200 ms) and
  respects `prefers-reduced-motion`.
- Dark mode via shadcn theme tokens (`admin/src/styles/tokens.css`, contrast-gated; 11 §3).
- Keyboard: command palette (`⌘K`, shadcn `command`) for navigation and "new post".
