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
- OpenAPI 3.1 generated from code with `utoipa` (6.x) at `/api/v1/openapi.json`; the
  admin's TypeScript client will be generated from it (`openapi-typescript` +
  `openapi-fetch`) and checked in CI for drift (M2b).

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
  3.1.0 with security schemes `session` (cookie `ferrite_session`) and `bearer`; the
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
| Auth | `POST /auth/login`, `POST /auth/login/totp`, `POST /auth/logout`, `GET /auth/me`, `POST /auth/password`, `POST /auth/totp/setup`, `POST /auth/totp/confirm`, `POST /auth/totp/disable` | M2a-1 | own account |
| Auth: password reset by e-mail | `POST /auth/password-reset`, `POST /auth/password-reset/confirm` (no credentials; origin-checked like login) | M2a-2a (needs `[mail]`) | none (the sealed token is the credential) |
| Content | `GET/POST /content/{type}`, `GET/PATCH/DELETE /content/{type}/{id}` (`DELETE` trashes; `?force=true` deletes a trashed item), `POST /content/{type}/{id}/publish`, `…/unpublish`, `…/submit`, `…/schedule`, `…/restore` (from trash); type `media` is 404 here (uploads are `/media`) | M2a-1 | `read_admin` + `content:read` and `can_read`; writes `edit_own`/`edit_others`/`publish_*` + `content:write` |
| Revisions | `GET /content/{type}/{id}/revisions`, `GET …/revisions/{rid}`, `POST …/revisions/{rid}/restore` | M2a-1 | reads as content plus a role that may edit the item (R26, §1 "Reading revisions"); restore as a content update |
| Content types | `GET/POST /types`, `GET/PATCH/DELETE /types/{name}` | M2a-1 | reads `content:read`; writes `manage_settings` + `settings:write` |
| Taxonomies & terms | `GET/POST /taxonomies`, `GET/PATCH/DELETE /taxonomies/{name}`, `GET/POST /taxonomies/{taxonomy}/terms`, `GET/PATCH/DELETE /terms/{id}` | M2a-1 | taxonomy reads `content:read`, writes `manage_settings` + `settings:write`; term reads `terms:read`, writes `manage_terms` + `terms:write` |
| Menus | `GET /menus`, `GET/PUT/DELETE /menus/{name}` (`PUT` replaces the whole tree) | M2a-1 | reads `menus:read`; writes `manage_menus` + `menus:write` |
| Users & tokens | `GET/POST /users`, `GET/PATCH/DELETE /users/{id}` (`DELETE ?reassign=<id>` moves the user's content), `POST /users/{id}/password`, `GET/POST /users/{id}/tokens`, `DELETE /users/{id}/tokens/{tid}` | M2a-1 | others: `manage_users` + `users:read`/`users:write`; oneself: own e-mail, display name and tokens (`users:read` to list tokens, `users:write` to change); own password via `POST /auth/password` |
| Redirects | `GET/POST /redirects`, `GET/PATCH/DELETE /redirects/{id}`, `POST /redirects/import` (CSV), `GET /redirects/export` (CSV) | M2a-1 | `manage_redirects` (admins) + `redirects:read`/`redirects:write` |
| Settings | `GET/PATCH /settings` (the allowlist `ferrite_core::settings::SETTINGS`) | M2a-1 | reads `settings:read`; writes `manage_settings` + `settings:write` |
| Media | `POST /media` (multipart, streaming), `GET /media`, `GET/PATCH/DELETE /media/{id}` | M2a-2b | — |
| Comments | `GET /comments?status&item&limit&cursor`, `PATCH /comments/{id}` (`{ status }`), `DELETE /comments/{id}` (spam or trash only), public `POST /content/{type}/{id}/comments` (rate-limited, honeypot; see **Comments** below) | M2a-2b (built) | moderation `moderate_comments` + `comments:read`/`comments:write`; the submit: none, or a token with `comments:write` |
| Themes | `GET /themes`, `POST /themes` (raw ustar bundle, `?activate=true`), `POST /themes/{name}/activate`, `GET/PATCH /themes/{name}/settings` (no delete, M2a-2 P30) | M2a-2a | reads `manage_themes` + `extensions:read` (`require_reader`); writes `manage_themes` + `extensions:write` |
| Plugins | `GET /plugins`, `GET /plugins/{name}`, `POST /plugins/inspect` (multipart upload → permission report, nothing installed), `POST /plugins` (install with `approve` = the report's fingerprint), `POST /plugins/{name}/enable`, `POST /plugins/{name}/disable`, `DELETE /plugins/{name}` (`?purge=true`), `GET/PATCH /plugins/{name}/settings` | M2a-2a | reads `manage_plugins` + `extensions:read` (`require_reader`, so editors see no plugin list); writes `manage_plugins` + `extensions:write` |
| Plugin metrics | `GET /plugins/{name}/metrics` | M2a-2b | — |
| System | `GET /system/status` (versions, schema, nodes, backends, the theme fallback flag), `GET /system/jobs`, `POST /system/jobs/{id}/retry`, `GET /system/metrics/summary` (for dashboard charts), `GET /system/audit` | M2a-2b | — |
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
  the limit holds across nodes (`429` + `Retry-After`). `201` = approved, `202` = held; a
  filled honeypot (`website`) also answers `202` in the same shape under an id that is never
  stored (F18). The response (`CommentPublicDto`) never carries the e-mail or address hash.
- **Status**: a commenter whose role holds `moderate_comments` is approved. Under
  `comments.moderation = "known"` (default, R2) a comment with at most two links (`http://`,
  `https://`, `www.`) is approved when its author has an approved comment — a guest by the
  same e-mail (ASCII case folded) and trimmed name on an approved *guest* comment, never when
  the e-mail belongs to an account, and never through comments an account wrote before it was
  deleted (`comments.by_account`); a signed-in commenter by an approved comment of the account
  (F19). `"all"` holds every comment but a moderator's.
- **Moderation**: `GET /comments` (newest first, filters `status`, `item`) answers
  `CommentDto` with every column, the e-mail and the keyed `ip_hash` (HMAC under the site key,
  label `ferrite:comment-ip:v1`) included. `PATCH` sets the status; `DELETE` removes a spam or
  trashed comment and its replies (else `422`, field `status`). An approved insert and every
  `PATCH`/`DELETE` write `invalidate([post:<item>])` in the same `Tx`. Not audited (P14).
  Settings `comments.enabled` and `comments.moderation` show `null` until set (defaults are
  core constants, F21).

**Extensions (M2a-2a; rulings in [`m2a2.md`](../superpowers/rulings/m2a2.md)).** Handlers
authorize through `ExtensionAdmin` (`authorize_plugins` / `authorize_themes`) before any of
an upload's body is read, so the OpenAPI sweep's unauthenticated and CSRF cases hold for
the upload routes too; the node's one `Installer` checks and compiles uploads (two
compiles and four uploads at once per node; a fifth upload is `503 busy` with
`Retry-After`). Every write takes the `extensions` lock (30 s, then `503 busy`) and
appends an audit row (`plugin.install|enable|disable|delete|settings`,
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
  `{ plugin, version, artifact, items[], added[], limits[], fingerprint, update }` (05 §2;
  `limits` lists only the limits the manifest requests); `POST /plugins` with an `approve` that is not that upload's
  fingerprint is `422 validation`, `errors[0].field = "approve"`, with the upload's
  `report` in the problem, and changes nothing (R1). Install answers `201` with `Location`
  (an update too); a new plugin is installed disabled, a replaced one keeps its state.
- **`PluginDto`**: `name`, `version`, `enabled`, `disabled_reason`, `installed_at`,
  `updated_at`, `report` (the row's report against its own grant, so `added` is empty for
  a row installed with approval; `null` if the stored manifest or grant no longer parses)
  and `nodes` (each node with a heartbeat in the last 30 s: `node`, `name`, `version`,
  `status` = `loaded|failed|disabled`, `error`: the node's diagnostic, verbatim up to 1 KiB,
  which can name storage paths or endpoints; admins only).
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
  hot swap; 06 §1).
- **Settings**: `GET/PATCH /themes/{name}/settings` and `/plugins/{name}/settings` answer
  `{ schema, values }` (plugins also `secrets` and `invalid`, 05 §4c); `PATCH` takes an
  object of keys (`null` resets or deletes one), checks every entry first (an undeclared
  key or a bad value is `422 validation` with `errors[0].field` naming the key) and writes them in one
  `Tx` with `options.changed`. Secrets are write-only.

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
| manage users, settings, themes, plugins, import, redirects | ✓ | — | — | — |

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
  ("publish now").
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
  password-reset-required`). Its API tokens are not restricted (R26): they are
  independent credentials that a reset keeps (R22), so a bot account keeps working
  after an admin resets its password.
- **Logins** (P14, R3): `^[a-z0-9][a-z0-9._-]{0,59}$` after trimming and ASCII
  lowercasing, immutable after creation, so `/author/<login>/` never moves. A login input
  containing `@` is looked up as an e-mail. E-mails are unique by the ASCII-lowercased
  `email_key`, so non-ASCII case variants (`É@x`, `é@x`) are distinct accounts.
- **Sessions** (P7; DB-backed, [04 §5](04-cluster.md#5-sessions)): the cookie
  `ferrite_session` holds 32 random bytes as base64url (43 characters); the database stores
  only its SHA-256 hex. Cookie attributes: `HttpOnly; SameSite=Lax; Path=/;
  Max-Age=2592000` (30 days), plus `Secure` **only when `site.url` is `https://`**, so local
  `http://localhost` development keeps the cookie (an unconditional `Secure` would drop it).
  Idle timeout 7 days, absolute 30 days, `last_seen_at` written at most every 5 minutes. No
  per-node cache: every request reads its session row, so a revocation applies on every
  node at once. Sessions are deleted on logout, on a password change (the API then issues a
  new cookie for the caller), on a role change, disable and delete, by `ferrite user
  reset-password`, and when TOTP is enabled (all but the confirming session, R26). A
  cookie sent with a successful login is deleted server-side (no session fixation). The
  cookie has no `__Host-` prefix yet (M2a-2b).
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
  session is kept, through a token every session ends, API tokens are kept); setup while
  TOTP is on is `422` (disable first). A correct password for an account with TOTP
  answers `{ "status": "totp_required", "challenge" }` (a sealed blob, valid 5 minutes,
  bound to the current password hash) instead of `{ "status": "session", "user",
  "csrf_token" }`; `POST /auth/login/totp` finishes it. A code's step must be newer than
  `users.totp_last_step` (one conditional update), so a code cannot be replayed. The
  issuer is the configured `site.title` (`ferrite.toml`, read at startup) without `:`
  (`FerriteCMS` when empty). There are no recovery codes: an operator runs `ferrite user
  disable-totp --login <login>`.
- **Site key** (P4, R1; [01 §4](01-architecture.md#4-configuration), §8):
  `auth.secret_key[_file]`, or in solo mode `<data>/secret.key`; cluster mode refuses to
  start without a configured key. It seals TOTP secrets, login/enrollment challenges and
  password-reset tokens and keys the IP and login/reset-bucket hashes. Sessions and CSRF
  tokens use per-session secrets, so a lost or changed key logs nobody out but makes
  stored TOTP secrets unreadable (users then re-enroll after `ferrite user disable-totp`)
  and outstanding reset links invalid.
- **Passkeys** (`webauthn-rs`): deferred past M2 (P3; the newest `webauthn-rs` is a
  pre-release, and the ceremony needs the SPA). There is no `passkeys` table.
- **Audit log** (P22): user, token, TOTP, type/taxonomy and settings writes, extension
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
    second factor; a lost authenticator still needs `ferrite user disable-totp`.

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
