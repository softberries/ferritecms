# M5 exit: AI assistants (MCP)

**Criterion** ([ROADMAP](../ROADMAP.md), M5; design [12](../design/12-mcp.md)): every site
serves an authenticated MCP endpoint at `<site.url>/mcp`, so a user can connect an AI
assistant (Claude, ChatGPT, Claude Code, Cursor, VS Code or any MCP client), sign in once,
and manage the site with it — as themselves, with their role, audited — and the website,
docs and README present this as the product's headline feature.

**Result: done** (2026-11-02, branch `m5-mcp`). `v0.1.0` has no MCP code; `v0.2.0` is the release that carries it (the
release commit dropped the "coming in 0.2.0" caveat from the docs, the landing page and the
README; the tag follows when that commit is merged).

## What was built

| Area | Delivered |
|---|---|
| Storage | migration `0013_oauth` on both backends (`oauth_clients`, `oauth_requests`, `oauth_codes`, `oauth_grants`, `api_tokens.oauth_grant_id`), contract tests on both; OAuth rows stay out of exports, `import-site` and `db copy`; pruned by `sessions.prune` |
| Authorization server | `ferrite_core::oauth::OAuthService`: Client ID Metadata Documents (SSRF-checked fetch, 16 KiB, cached 24 h, stale copy up to 7 d, 4 fetches per node) and dynamic registration (10/IP/h); PKCE S256; codes 60 s single use (reuse revokes the grant); grants 30 days sliding with refresh rotation and replay revocation; access tokens 1 h with their own prefix `fera_`, refused by `/api/v1` (audience binding, also on pre-M5 nodes); `users:write`, `import:write`, `system:write` never grantable; disabling a user ends their grants and pending codes; audit `oauth.connect` / `oauth.disconnect` |
| HTTP | `/.well-known/oauth-protected-resource[/mcp]`, `/.well-known/oauth-authorization-server` (+ path-inserted forms), `/oauth/authorize|token|register|revoke` (escaped error page, linear form parsing, CORS on the public endpoints), the consent API under `/api/v1/oauth/` (sessions + CSRF only), the `mcp.enabled` off switch (404 everywhere when off) |
| MCP endpoint | `rmcp` 3.8, Streamable HTTP, stateless (`/mcp`, POST only); guard: off switch → 405 → Origin → exactly one bearer → `401` with `resource_metadata`; 120 calls per user per minute (tools, resources, prompts); 9 MiB body; `no-store`; server instructions with the prompt-injection caution |
| Tools | 56 tools in 13 areas (site, content, media, taxonomies, menus, content types, redirects, comments, settings, appearance, plugins, users, system), annotated (read-only / destructive / idempotent / open-world), Markdown bodies in and out (`ferrite_core::blocks::markdown`, the importer's `markdown_to_body`, `ferrite-block` fences), optimistic versions with the current item on conflict, media upload from base64 or an `https` URL (6 MiB, own fetch slots), every successful write audited as `mcp.<tool>` with the client's name; 2 resources + 1 template; prompts `write_post`, `review_seo`, `site_health`, `tidy_media`; nothing for the human-only actions (12 §5.4), pinned by tests |
| Admin | `/admin/connect` consent screen (Off/Read/Write per area, verified vs self-registered, logos only for verified clients, safe redirects), Account → Connected apps, Users → a user's apps, Settings → AI assistants (switch, URL, per-client steps), the dashboard card, the palette entry |
| Docs and website | guide "AI assistants (MCP)", the MCP tools reference (kept in step with the server by `mcp_reference.rs`), mentions in Introduction / First site / Concepts / REST API, an AI-first landing page (hero, illustrated conversation from a real run, "Run your site from your AI assistant" section, tour tab), README section, design 00/07/12, BRIEF, ROADMAP, `site/VERIFIED.md` rows |

Rulings: [`rulings/m5.md`](rulings/m5.md). Plan:
[`plans/2026-11-01-m5-mcp.md`](plans/2026-11-01-m5-mcp.md). Ten build tasks, each with a
task review (security-focused for the auth and tool tasks) and fix rounds, then a three-way
final review (auth and security; server and tools; admin and docs), one fix pass with a
scoped re-review, and a last hardening commit.

## Verified

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`: clean.
- `cargo test --workspace` with Postgres 17: 4 195 passed, 0 failed, 0 SKIP (at the final
  fix pass); after the hardening commit the five touched crates: 2 472 passed, 0 failed.
- Admin: typecheck, lint, Vitest 827/827, bundle budget; E2E in Chromium on SQLite and
  Postgres: 399 of 400 passed, the one failure the known `failed_backup_restore…` flake under
  load, which passed on rerun against the final binary (44/44 for the editor-followups, mcp,
  account and users specs). `e2e/mcp.spec.ts` runs the whole OAuth flow in a browser
  (registration, PKCE, sign-in, consent narrowed to read-only, redirect with `code`, `state`
  and `iss`, a read-only token, disconnect) with axe on the four new screens.
- Real client: MCP Inspector CLI 0.17.5 against the demo site (personal token):
  `tools/list` 56 tools, all annotated; `site_overview`; `settings_update` refused with
  "Forbidden: missing settings:write"; `content_list`. Claude Code (in an isolated config
  directory) connected with a token, and without one reported that it needs authentication.
- Docs: `docs_site` test, the static export with the brand layer and Pagefind, a link check
  (29 pages, 69 URLs, 0 broken), the landing page in light, dark and at 390 px.

## Not verified

- The hosted clients themselves (Claude custom connectors, ChatGPT developer mode): they
  connect from the vendors' clouds and need a public HTTPS site. Their steps in the docs are
  written against the vendors' documentation (ChatGPT's against a third-party walkthrough).
- Claude Code's interactive OAuth sign-in (the same flow ran scripted and in the E2E).
- Firefox and WebKit runs of the new E2E spec; the consent path for an account that must
  change its password first.
- The rolling-upgrade window in a real cluster (old and new nodes side by side): covered by
  the `fera_` prefix reasoning and a test against v0.1.0's `resolve_token` logic, not by a
  run.
- Everything ran on macOS (arm64) with local Postgres 17; nothing on CI yet.

## Known limits (follow-ups in the ROADMAP)

- A failed audit append after a committed write only logs (the write may go unaudited).
- `content_preview_link` needs the human's admin session (no signed preview links).
- `site_overview` counts with several queries per content type; `terms_list` loads the
  whole taxonomy (as REST does); media in custom `media` fields is not checked for existence.
- Unused `oauth_clients` rows are not pruned.

## For the user

- **Release:** `v0.2.0` is the release that carries M5. It is tagged after the release commit
  (`release: 0.2.0 — AI assistants over MCP`) is merged; the release gate of M4-2 applies (the
  tag ruleset lets only admins create `v*` tags).
- **Social preview:** upload `docs/brand/social-preview.png` under the repository's
  Settings → Social preview.
- **Hosted clients:** once a site runs on public HTTPS, try Claude's custom connector and
  ChatGPT's developer mode against it and tell us what the menus say.
