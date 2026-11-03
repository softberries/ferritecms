# 12 — AI assistants: the MCP server

Status: **design for M5** · Date: 2026-11-01

FerriteCMS is managed from AI assistants through the Model Context Protocol (MCP). Every
site serves an MCP endpoint at `<site.url>/mcp`. A user connects Claude, ChatGPT, Cursor,
VS Code or any other MCP client to it, signs in once, and can then ask the assistant to
write and publish posts, organise media, edit menus, fix redirects, moderate comments,
change settings or check the site's health. The assistant acts as that user: the same
role, the same capability checks and the same audit trail as the admin and the REST API.

This is a headline feature, not an add-on: the docs site, the README and the admin say so
(§9).

## 1. Goals and non-goals

Goals:

1. **Nothing to install.** The endpoint is part of the binary and on by default; the only
   setup is "paste this URL into your assistant".
2. **Standard auth.** Clients that support the MCP authorization spec (2026-07-28 revision)
   connect with a browser sign-in and a consent screen, no copied secrets. Clients that
   take a header use a personal API token.
3. **The whole site.** Tools cover what an editor or administrator does in the admin,
   except the few actions that must stay human (§5.4).
4. **Same rules as everywhere.** Tools call the core services with the caller's `Actor`.
   No tool checks permissions itself; core does (CLAUDE.md conventions).
5. **Both profiles.** Solo and cluster. The endpoint keeps no state between requests, so
   any node answers.

Non-goals (this milestone): an MCP *client* inside FerriteCMS, AI features inside the admin
(generation buttons), a stdio transport (`ferrite mcp`; Claude Desktop and Claude Code
both speak HTTP), plugins adding MCP tools (later: a WIT extension, see §10).

## 2. Protocol and transport

- MCP over **Streamable HTTP** at `POST /mcp` (JSON-RPC), using the official Rust SDK
  `rmcp` (3.8, features `server`, `macros`, `schemars`, `transport-streamable-http-server`).
- **Stateless**: `legacy_session_mode = false`, `NeverSessionManager`,
  `json_response = true`. Each request is authenticated and answered on its own. `GET /mcp`
  (a server-initiated SSE stream) answers `405`; FerriteCMS sends no unsolicited messages.
  Older protocol versions that the SDK still negotiates are served statelessly too.
- **Origin check** (DNS rebinding, MCP security best practice): a request carrying an
  `Origin` header that is not the site's origin is refused (`403`). Requests without
  `Origin` (server-side clients) pass.
- **Body limit** 9 MiB (a 6 MiB media upload as base64 plus the JSON envelope, §5.2);
  responses are JSON. Structured values a tool writes (`body_blocks`, `fields`, `menu_set`
  items, the settings maps of `settings_update`, `theme_settings_update` and
  `plugin_settings_update`) are refused with a field error above the REST API's 4 MiB JSON
  cap, so MCP writes nothing `/api/v1` would not take.
- Server info: name `FerriteCMS`, the binary version, and `instructions`: a short text that
  tells the model what the site is (title, URL, content types), how content is written
  (Markdown in, Markdown out), that changes are live, and to ask before destructive steps.
  The title and the content types' names and labels are an admin's text: control and
  format characters (bidi overrides, zero-width), newlines and line separators become
  spaces, and they are cut (title 200, names and labels 80 characters).
- **Off switch**: the site option `mcp.enabled` (default `true`, Settings → AI
  assistants). When off, `/mcp`, the OAuth endpoints and both metadata documents answer
  `404`, and existing grants stay (they work again when it is turned back on).

## 3. Authentication

### 3.1 Bearer tokens

Every `/mcp` request needs `Authorization: Bearer <token>`. Two kinds are accepted, both
resolved by `AuthService::resolve_token` (one path, one rate limit for bad tokens):

1. **Personal API tokens** (`fer_…`, Account → API tokens, M2a-1), with their scopes.
   This is how Claude Code (`claude mcp add --transport http … --header`), Cursor, VS Code
   and scripts connect.
2. **OAuth access tokens** issued by the built-in authorization server (§3.2). They are
   rows of `api_tokens` too (sha-256 hashed), tied to an OAuth grant
   (`api_tokens.oauth_grant_id`), valid for **1 hour**, with their own prefix: `fera_` +
   43 base64url characters. `resolve_token` requires the prefix to match the row (`fera_`
   only for a row with a grant, `fer_` only for one without); a mismatch is an invalid
   token.

A missing or bad token answers `401` with
`WWW-Authenticate: Bearer resource_metadata="<site>/.well-known/oauth-protected-resource/mcp"`
(and `error="invalid_token"` when one was sent), which is how an MCP client discovers
the sign-in flow.

**Audience binding** (RFC 8707, MCP "token audience validation"): OAuth access tokens are
for `/mcp` only. The REST API refuses a token that belongs to an OAuth grant (`401
invalid_token`), so a token handed to an AI client cannot be replayed against
`/api/v1`. Personal API tokens work in both places, as they always have. The `fera_`
prefix keeps the rule in a mixed-version cluster and after a binary rollback: a node
older than M5 looks a bearer up only when it starts with `fer_` (v0.1.0's
`resolve_token`) and knows nothing of grants, so it refuses an OAuth token before any
lookup instead of accepting it on `/api/v1`.

Sessions (cookies) are never accepted on `/mcp`.

### 3.2 The authorization server

FerriteCMS is its own OAuth 2.1 authorization server, for public clients only (no client
secrets): authorization code with **PKCE S256** (required), refresh tokens with rotation.

| Endpoint | Spec | Notes |
|---|---|---|
| `GET /.well-known/oauth-protected-resource/mcp` (and without the `/mcp` suffix) | RFC 9728 | `resource` `<site>/mcp`, `authorization_servers` `[<site>]`, `scopes_supported`, `bearer_methods_supported ["header"]`, `resource_name` (site title), `resource_documentation` (the docs page) |
| `GET /.well-known/oauth-authorization-server` | RFC 8414 | issuer `<site>`, the endpoints below, `response_types_supported ["code"]`, `grant_types_supported ["authorization_code","refresh_token"]`, `code_challenge_methods_supported ["S256"]`, `token_endpoint_auth_methods_supported ["none"]`, `scopes_supported`, `client_id_metadata_document_supported true` |
| `GET /oauth/authorize` | OAuth 2.1 | validates, stores a pending request, redirects (303) to the admin consent page |
| `POST /oauth/token` | OAuth 2.1 | `authorization_code` and `refresh_token`; form-encoded; JSON answers; `Cache-Control: no-store` |
| `POST /oauth/register` | RFC 7591 | dynamic client registration, public clients only |
| `POST /oauth/revoke` | RFC 7009 | revokes a refresh token (the whole grant) or an access token |

`<site>` is `site.url` without a trailing slash and is the issuer. When `site.url` has a
path, the metadata is also served at the RFC 8414 path-inserted address
(`/.well-known/oauth-authorization-server/<path>`).

**Clients** are known in two ways (MCP client registration, in the spec's order):

1. **Client ID Metadata Documents** (preferred by current clients): the `client_id` is an
   `https` URL with a path. The server fetches it with the SSRF-checked egress client
   (public addresses only, no credentials, 5 s, at most 16 KiB, at most 3 redirects, all
   `https`), requires valid JSON whose `client_id` equals the URL exactly and that has
   `client_name` and `redirect_uris`, and caches it in `oauth_clients` for 24 hours (a
   fetch failure keeps a cached copy for up to 7 days).
2. **Dynamic registration**: `POST /oauth/register` with `client_name`, `redirect_uris`
   and optionally `client_uri`, `logo_uri`; `token_endpoint_auth_method` must be absent or
   `none`; `grant_types` ⊆ the two above. The answer is `201` with a random `client_id`
   (`ferc_` + 22 base64url characters). Limited to 10 per IP per hour.

**Redirect URIs** must be `https`, or `http` on a loopback host (`localhost`,
`127.0.0.1`, `[::1]`, any port; RFC 8252 native apps), have no fragment and match a
registered one exactly (a loopback URI's port may differ, RFC 8252 §7.3). Custom schemes
(`cursor://…`, `vscode://…`) are allowed for registered clients, never `javascript:`,
`data:` or `file:`.

**Authorization request** (`GET /oauth/authorize`): `response_type=code`, `client_id`,
`redirect_uri`, `code_challenge` + `code_challenge_method=S256`, `state` (kept as given),
`scope` (optional, space-separated area scopes, §3.3), `resource` (optional; when given it
must be `<site>/mcp`, a trailing slash tolerated). A client or redirect URI that cannot be
validated is answered with an error page (never a redirect); other errors redirect back
with `error=` per RFC 6749 §4.1.2.1. A valid request is stored as a pending
`oauth_requests` row (10 minutes) and the browser goes to `/admin/connect?request=<id>`.

**Consent** (admin, §8): the signed-in user sees the client's name and logo, the redirect
host (with a non-default port: `app.example:8443`), whether the client is verified by a metadata document or self-registered, and the
requested permissions as area rows (read / read and write, limited to what the user's
role can do). They can narrow the permissions and approve or deny. Approval creates an
authorization code (60 seconds, single use, sha-256 stored, bound to the client, redirect
URI, PKCE challenge, resource and the approved scopes) and the admin navigates to
`redirect_uri?code=…&state=…&iss=<site>` (RFC 9207). A user with no area beyond reading
cannot grant write scopes; a contributor still can connect (their role limits them).

**Token endpoint**:

- `authorization_code`: checks the code, `client_id`, `redirect_uri`, the PKCE verifier
  (S256, 43–128 characters) and `resource` (if sent, must match). Creates an
  `oauth_grants` row (the refresh token, 30 days, sliding) and an access token (an
  `api_tokens` row named after the client, scopes = the grant's, 1 hour). Answers
  `access_token`, `token_type "Bearer"`, `expires_in 3600`, `refresh_token`, `scope`.
  A reused code revokes the grant made from it (OAuth 2.1 §4.1.3). A code presented
  with a wrong client, redirect URI or verifier is burnt with no grant made; presenting
  it again is "the code is unknown or expired".
- `refresh_token`: rotates — the old refresh token stops working, a new one and a new
  access token are issued, older access tokens of the grant are deleted. A refresh token
  used after rotation revokes the grant (replay detection). `scope` may narrow, never
  widen.
- Errors are RFC 6749 §5.2 JSON (`invalid_grant`, `invalid_client`, `invalid_request`,
  `unsupported_grant_type`, `invalid_scope`), `400`.
- Rate limit: 30 requests per IP per minute.

**Grants** belong to a user; the user sees them under Account → Connected apps (client,
permissions, first connected, last used) and can disconnect one, which deletes the grant
and, through the foreign key, its access tokens. An administrator can see and revoke all
grants on Users → user. A user who is disabled or deleted loses their grants: a disable
deletes them in the same transaction (one `oauth.disconnect` audit row per grant, reason
`user_disabled`), so re-enabling the user revives no connection; a delete cascades.
`resolve_token` refuses inactive users either way.

### 3.3 Scopes

OAuth scopes are the existing area scopes (`content:read` … `system:read`), so a grant is
exactly like a personal token's scope list, except that the writes no MCP tool uses are
never granted: `users:write`, `import:write` and `system:write` (defence in depth; users,
imports and the system are read-only to assistants, §6). That leaves 19 scopes
(`Scopes::OAUTH`), which is what the metadata's `scopes_supported` lists. A request
without `scope` asks for all of them; one naming an ungrantable write is offered its read.
The consent screen then preselects the areas the user's role can use, and users, import
and system go no higher than read. The token's effective rights are always `role ∩
scopes` (core, unchanged).

## 4. Storage (migration `0013_oauth`)

Expand-only, both backends, contract tests on both.

- `oauth_clients(id TEXT PK, kind TEXT 'metadata'|'registered', name TEXT, redirect_uris
  JSON, client_uri TEXT NULL, logo_uri TEXT NULL, created_at, fetched_at NULL,
  last_used_at NULL)`.
- `oauth_requests(id PK (uuid v7), client_id FK→clients cascade, redirect_uri, scopes JSON,
  state TEXT NULL, code_challenge, resource TEXT NULL, created_at, expires_at)`.
- `oauth_codes(code_hash TEXT PK, client_id FK cascade, user_id FK→users cascade,
  redirect_uri, scopes JSON, code_challenge, resource NULL, created_at, expires_at,
  grant_id NULL)` (the grant made from it, for reuse detection).
- `oauth_grants(id PK (uuid v7), client_id FK cascade, user_id FK cascade, scopes JSON,
  refresh_hash TEXT UNIQUE, previous_hash TEXT NULL, created_at, last_used_at, expires_at)`.
- `api_tokens.oauth_grant_id NULL` → `oauth_grants(id)` `ON DELETE CASCADE`, indexed.

Personal-token rules ignore OAuth rows: the 50-token limit, the Account token list and
`GET /users/{id}/tokens` count and list only rows without a grant. `sessions.prune` also
deletes expired requests, codes, grants and OAuth access tokens.

## 5. Tools, resources and prompts

### 5.1 Conventions

- Names are `snake_case`, `<area>_<verb>`. Each has a one-paragraph description written for
  a model (what it does, what it returns, when to use it), a JSON Schema for its input
  (schemars) and annotations: `readOnlyHint` for reads, `destructiveHint` for trash,
  delete, unpublish and settings writes, `idempotentHint` where it holds,
  `openWorldHint` only for `media_upload` from a URL.
- Results are **structured content** (JSON) plus a short text summary; lists are paged
  (`page`/`per_page` ≤ 100 with the total, or a `cursor`/`next_cursor` where core pages
  by cursor).
- **Bodies are Markdown.** Reads return `body_markdown` (CommonMark plus tables, from the
  item's blocks, §5.3) and the rendered page URL; writes accept `body_markdown` (converted
  with the Markdown importer's converter) or, for exact control, `body_blocks` (the
  editor's block JSON). Inline images are `![alt](ferrite:media/<id>)` both ways.
- **Optimistic concurrency**: updates take the `version` the model read; a stale version
  is an error that includes the current item, so the model can re-apply its change (the
  412 rule of the REST API).
- **Errors** are tool results with `isError: true` and the problem's title, detail and
  field errors (from `Problem`), never protocol errors, so the model can correct itself.
  Forbidden says which capability or scope is missing.
- **Audit**: every successful write tool appends an audit row `mcp.<tool>` with the
  target and `{ "client": <name> }` (the grant's client, or "API token <name>").
  Content and media changes also keep their revisions and events as usual.
- **Rate limit**: 120 tool calls per user per minute (`mcp:user:<id>`), on top of the
  per-IP bad-token limit.

### 5.2 Tool list

| Area | Tools |
|---|---|
| Site | `site_overview` (title, URL, version, active theme, content types, taxonomies, menus, counts, the caller's role and scopes), `site_search` (full-text over content) |
| Content | `content_list`, `content_get`, `content_create`, `content_update`, `content_publish`, `content_unpublish`, `content_schedule`, `content_submit_for_review`, `content_trash`, `content_restore`, `content_revisions`, `content_restore_revision`, `content_preview_link`, `content_translate` (M6-1) |
| Media | `media_list`, `media_get`, `media_upload` (from an `https` URL fetched with the SSRF-checked client, or base64 data up to 6 MiB), `media_update` (title, alt), `media_trash`, `media_restore` |
| Taxonomies | `taxonomies_list`, `terms_list`, `term_create`, `term_update`, `term_delete` |
| Menus | `menus_list`, `menu_get`, `menu_set` (replace the item tree) |
| Content types | `content_types_list`, `content_type_get`, `content_type_create`, `content_type_update` |
| Redirects | `redirects_list`, `redirect_create`, `redirect_update`, `redirect_delete` |
| Comments | `comments_list`, `comment_moderate` (approve, spam, trash), `comment_reply` |
| Settings | `settings_get`, `settings_update` |
| Appearance | `themes_list`, `theme_activate`, `theme_settings_get`, `theme_settings_update` |
| Plugins | `plugins_list`, `plugin_enable`, `plugin_disable`, `plugin_settings_get`, `plugin_settings_update` (secrets write-only, as in the API) |
| Users | `users_list`, `user_get` (no tokens, sessions or TOTP data) |
| System | `system_status`, `jobs_list`, `audit_list` |

### 5.3 Markdown out

A new `ferrite_core::blocks::markdown` serializer turns a body's blocks into CommonMark:
headings, paragraphs with marks (bold, italic, code, strike, links), lists (nested),
quotes, code blocks with language, horizontal rules, tables, images
(`![alt](ferrite:media/<id> "caption")`). Embeds, raw HTML, merged-cell tables and
anything else become a fenced ```` ```ferrite-block ```` JSON block, so nothing is lost on a
round trip. A leading `# H1` in `body_markdown` is the title when the call gives none; equal
to the given title it is dropped; different, the tool asks the model to move it.

### 5.4 What stays human

No tool installs, uninstalls or purges plugins or themes (installation needs a person to
read the permission report and approve its fingerprint), creates, deletes or changes users,
roles or passwords, manages tokens, sessions, TOTP or OAuth grants, changes `mcp.enabled`,
runs imports, backups or restores, or retries jobs. The assistant can point the user to the
admin screen for these (`site_overview` returns the admin URL).

### 5.5 Resources and prompts

- Resources: `ferrite://site` (the overview as Markdown), `ferrite://content/{type}/{id}`
  (an item as Markdown with front matter; template resource), `ferrite://guide/writing`
  (the site's writing conventions: the theme's supported blocks, image sizes, SEO fields).
- Prompts: `write_post` (topic, tone → a draft for review), `review_seo` (an item → title,
  description and heading suggestions), `site_health` (status, failing jobs, plugin
  failures → a summary), `tidy_media` (find images without alt text); M6-1: `translate` (an item and a locale →
  `content_translate`, a translated draft for review).

## 6. HTTP wiring

A new module `ferrite-http::mcp` with `router(Arc<ApiState>) -> Router`, merged in
`build_site` next to the API router:

- `mcp::server` — the `rmcp` `ServerHandler` (`FerriteMcp`) built per request with the
  caller's `Authenticated`, the services from `ApiState`, and the client label.
- `mcp::auth` — middleware in front of the `StreamableHttpService`: the off switch, the
  Origin check, bearer resolution (via `AuthService::resolve_token`), the 401 challenge,
  the per-user rate limit; it puts `Authenticated` into the request extensions where the
  tools read it.
- `mcp::tools::*` — one file per area; each tool is a thin adapter from schemars input
  types to the existing core services and DTOs.
- `oauth` — the endpoints of §3.2 (outside `/api/v1`, form-encoded/JSON as OAuth needs),
  and `api::oauth` — the session endpoints the consent page uses:
  `GET /api/v1/oauth/requests/{id}`, `POST …/{id}/approve` (`{scopes}` → `{redirect_to}`),
  `POST …/{id}/deny`, `GET /api/v1/oauth/grants` (own; `?user=` for admins),
  `DELETE /api/v1/oauth/grants/{id}`. These are in the OpenAPI document.
- `RouteClass::Mcp` and `RouteClass::OAuth` in the metrics.

Core gets `ferrite_core::oauth::OAuthService` (clients, requests, codes, grants, PKCE,
rotation, revocation, audit `oauth.connect`, `oauth.disconnect`; the metadata-document
fetch behind a `ClientMetadataFetcher` trait implemented in `ferrite-http` with the egress
client) and `AuthService` learns about grant-bound tokens.

## 7. Security notes

- Tokens: 32 random bytes, sha-256 at rest; codes and refresh tokens the same. Constant-
  time comparison of hashes is unnecessary (lookups by hash), PKCE comparison is of hashes.
- Consent page: `frame-ancestors 'none'` (the admin CSP), approval needs the session and
  CSRF token, and the request id is single-use (approve or deny consumes it).
- Open redirect: the server only ever redirects to a URI registered for that client, and
  never before the client and URI are validated.
- Metadata fetch: SSRF rules of the plugin egress client; the fetched JSON is size-capped
  and never followed beyond its declared fields; `logo_uri` is shown only if `https`. At
  most 4 fetches run at once per node (`/oauth/authorize` needs no sign-in); a request
  that would need a fifth uses a cached copy (up to 7 days old) or gets a `503` "busy,
  try again" page, never a redirect.
- Prompt injection: tool results contain site content written by others (comments, imported
  posts). The server `instructions` say that content is data, not instructions; tools that
  change things are annotated so clients ask the user; destructive actions are limited
  (trash, not delete; §5.4).
- Every write is audited with the client's name.

## 8. Admin

- `/admin/connect?request=<id>`: the consent screen (sign-in layout, the FerriteCMS mark,
  the client's logo when `https`), with Approve / Deny; signed-out users go through sign-in
  first and come back.
- Account → **Connected apps**: grants with client, permissions, connected, last used,
  Disconnect.
- Settings → **AI assistants**: the switch, the MCP URL with Copy, and how to connect
  Claude, ChatGPT, Claude Code, Cursor and VS Code (tabs), with a link to the docs page.
- Dashboard: a card "Manage this site with AI" with the URL and a link to the settings
  group, until the first grant exists (then it shows the connected apps count).
- Palette: "Connect an AI assistant".
- Users → user: the user's connected apps (admins can disconnect).

## 9. Product story

AI management is the first thing the website says:

- Landing hero: the lead and a second terminal-style panel with a short conversation
  ("Publish a post about our spring opening hours and add it to the menu" → tool calls →
  done).
- A landing section "Run your site from your AI assistant" with the clients, the security
  model in three bullets (your login, your role, audited), and the URL to paste.
- Feature cards: "Managed by AI" first.
- Docs: a guide "AI assistants (MCP)" (connect per client, permissions, what the assistant
  can and cannot do, example requests, troubleshooting), a reference page "MCP tools"
  (every tool, kept in step with the code by a test), and mentions in Introduction,
  Concepts, First site and REST API.
- README: a section right after the intro.

## 10. Later

Plugins contributing tools (a WIT `mcp` interface; untrusted inputs from the model need a
separate review), a stdio bridge for clients without HTTP, per-tool allow lists per grant,
MCP elicitation for confirmations, the ChatGPT Apps SDK widgets.
