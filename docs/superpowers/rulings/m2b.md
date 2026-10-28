# M2b admin SPA — plan and controller rulings

Decisions made while planning and executing the M2b sub-plans (subagent-driven). M2b-1:
docs/superpowers/plans/2026-10-25-m2b1-foundation-and-writing.md (18 tasks); M2b-2 and M2b-3
append to this file. Format: ruling — reason — cost if wrong. P = plan rulings, F = pre-flight
findings (all accepted as proposed by R1; one line each), R = controller rulings during
execution. Where the code differs from a plan ruling's text, the line says "as built". The
full text of every F-finding and ruling is in the M2b-1 ledger
(`.superpowers/sdd/2026-10-25-m2b1-foundation-and-writing/progress.md`), not committed.

## M2b-1 plan rulings

- Plan ruling P1: M2b is three sub-plans: M2b-1 foundation and writing (18 tasks), M2b-2 media and content modelling, M2b-3 administration, extensions and the M2 exit — writing first exercises the hardest shared parts (API client, conflict handling, editor) — cost: M2b-1 ships an admin without media, users or settings screens.
- Plan ruling P2: shadcn on Radix (`shadcn init -b radix`), not Base UI — 07 §3 names Radix, and it is the mature library — cost: a later move to Base UI is a component-by-component re-add.
- Plan ruling P3: Inter Variable (with optical sizes) for UI, Geist Mono Variable for code and identifiers, OFL-1.1, fontsource 5.3.0, self-hosted hashed woff2 with unicode-range subsets — cost: about 0.8 MB of fonts in the binary (measured: 20 woff2 files, 781 088 bytes; M2b-1 exit).
- Plan ruling P4: the accent is "ember" (oklch hue 40–52) on cool graphite neutrals; `warning` sits at hue 70–80 with its own icon and label — cost: orange accents read as "warning" to some users.
- Plan ruling P5: TypeScript 5.9.3, because openapi-typescript 7.13.0 peers `^5.x` (typescript-eslint 8.71.1 would accept 6.0.x; corrected by F41) — cost: TS 7's faster checker waits for a later bump.
- Plan ruling P6: openapi-typescript + openapi-fetch; drift is a regenerate-and-diff (`pnpm api:check`) — cost: request bodies are not runtime-validated (the server validates).
- Plan ruling P7: Recharts 3 (pinned 3.10.1) through the shadcn `chart` component for every chart, in lazy route chunks — cost: Recharts' weight (measured: the `chart` chunk is 361 961 bytes, 104 240 with `gzip -9`; M2b-1 exit).
- Plan ruling P8: the admin CSP allows `style-src 'unsafe-inline'` but no inline script and no eval; Zod runs jitless, TipTap with `injectCSS: false` — cost: CSS injection through a compromised dependency is not blocked.
- Plan ruling P9: our own theme provider, no `next-themes`; the shadcn `sonner` wrapper is edited — cost: one edited generated file (11 §7).
- Plan ruling P10: `routeTree.gen.ts` is committed — cost: generated-file diffs in route-adding commits.
- Plan ruling P11: screenshots are review artifacts (a contact sheet checked against 11 §8), not pixel baselines — cost: no automatic visual regression.
- Plan ruling P12: the styleguide ships as a lazy chunk at `/admin/_styleguide`, behind login and unlinked — cost: a small chunk any signed-in user can open (it holds no data).
- Plan ruling P13: author references and allowed actions are added members on content and revision DTOs, computed in core — cost: one `users().get_many` per list page.
- Plan ruling P14: status counts are a separate `GET /content/{type}/counts` — cost: one more request per list view.
- Plan ruling P15: autosave is a per-(item, user) slot (`0007_autosaves`), not a revision, deleted by the owner's next real save; no retention job — cost: an abandoned slot lingers until that user saves or discards the item.
- Plan ruling P16: the dashboard shows only data that exists: no page views (M4); activity bucketed client-side from at most 1 000 published items; site health node-local — cost: no traffic chart until M4.
- Plan ruling P17: dashboard states a seeded site cannot produce (dead jobs, no activity) are component-tested with a stubbed API — cost: not exercised against the real server in M2b-1.
- Plan ruling P18: body headings are levels 2–6; the title is the h1; a stored level-1 heading opens read-only — cost: the M3 importer must demote imported h1 blocks.
- Plan ruling P19: the editor's schema is exactly the core renderer's node and mark set; anything else opens read-only; no underline — cost: no underline, highlight, callouts or embeds until the renderer gains them.
- Plan ruling P20: bulk actions are client-side fan-out (4 at a time, own `If-Match`, failures listed) — cost: bulk trashing 200 items is 200 requests.
- Plan ruling P21: undo over confirm (trash, revision restore, bulk trash); only permanent delete confirms — cost: an undo is lost if the toast is missed. As built amended by R9.
- Plan ruling P22: one authenticated E2E browser (Chromium) against a release build on SQLite, plus a Postgres smoke project; Firefox, WebKit and every spec on Postgres move to M2b-3 — cost: Safari-only bugs are found manually until then.
- Plan ruling P23: the SMTP sink is a ~100-line `node:net` server in the E2E support code — cost: no SMTP extensions are exercised.
- Plan ruling P24: `Referrer-Policy: no-referrer` on every admin response — cost: the public site does not see the admin as a referrer.
- Plan ruling P25: screens with no API yet are not stubbed (import M3, backups M4) — cost: the sidebar grows as sub-plans land.
- Plan ruling P26: i18n from the start (`t()` everywhere, `en` only), a missing key fails tests — cost: slightly slower UI tasks.

## M2b-1 pre-flight findings (accepted by R1)

- F1: `build.rs` watches `admin/dist` if present, else `admin`, else only itself (a missing watched path rebuilds every time).
- F2: admin files carry a weak `ETag` (the compression layer re-encodes), weak `If-None-Match`, `Vary: Accept-Encoding`.
- F3: a decoded path with a `.`/`..` segment, backslash or NUL is `404`; a missing path with a known extension is `404`; only extension-less paths fall back to `index.html`.
- F4: `crates/ferrite/tests/serve/site.rs` is in Task 1's files.
- F5: the admin CSP's `img-src` includes `https:` (external body images).
- F6: content summaries have no `permalink`; the list and palette show the slug.
- F7: `allowed.edit` is false on trashed items; the matrix test seeds private items through the store.
- F8: several cargo test filters go after `--`.
- F9: the hard-coded schema version 6 in the CLI and migration tests moves to 7.
- F10: `0007_autosaves` column types follow `0006` on each backend.
- F11: the autosave slot also holds `slug`, `parent_id`, `menu_order` and `terms` (amended by R4 for `parent_id`).
- F12: reading one's slot needs `require_read(Content)` + `role_can_edit`; writing needs `can_edit`.
- F13: after every shadcn CLI run the pins are restored and `next-themes` and Geist Sans removed; `cn` pinned at 0.4.0.
- F14: pinned peers `react-is` 19.3.0, `@testing-library/dom` 10.4.2, `@floating-ui/dom` 1.8.0.
- F15: no `exactOptionalPropertyTypes` (generated components fail it).
- F16: `.prettierignore` and ESLint ignores for generated and output files; two ESLint rules off for generated `components/ui`.
- F17: `"noEmit": true` in each referenced tsconfig.
- F18: `missing_key_fails` lives in `src/test/setup.test.ts`.
- F19: route files: `_app/[_]styleguide.tsx`, a minimal `_app.tsx` guard from Task 9, `/` at `_app/index.tsx`.
- F20: `/login/totp` is `login_.totp.tsx`; the not-found screen is the router's `defaultNotFoundComponent`.
- F21: a wrong or expired TOTP code is a `401`, not a `422`.
- F22: the sign-in screens show the host of `site_url` (no site title on `/meta`).
- F23: the reset and logout tests use their own users and sessions; shared storage states are never ended.
- F24: E2E logins send distinct `X-Forwarded-For` addresses through a trusted loopback proxy.
- F25: one smoke file, `admin/e2e/shell.smoke.spec.ts`.
- F26: the CI admin job reads pnpm from `admin/package.json`, caches on `admin/pnpm-lock.yaml` and frees disk.
- F27: `.dockerignore` excludes `admin/node_modules`, `dist`, `test-results`, `e2e/.auth`.
- F28: Motion's `domMax` (layout animations), loaded lazily.
- F29: reduced motion also zeroes CSS animation and transition durations.
- F30: `no_raw_colours_outside_tokens` allowlists the generated chart's Recharts selectors.
- F31: English strings in generated components go through `t("common:…")`.
- F32: TipTap's link `target`, `trailingNode` and default heading level are configured so load → save does not rewrite content.
- F33: TipTap's and prosemirror-tables' CSS is copied into `src/styles/editor.css`.
- F34: `author` filters take the user's id; the "this node only" label sits on the page-cache card.
- F35: tests comparing counts with lists create their own content type.
- F36: Undo after trash re-applies the previous status where `allowed` permits.
- F37: the `401` handler navigates with the router and `ignoreBlocker`, keeping the draft in memory.
- F38: `beforeunload` is set only while the slot does not yet hold the changes.
- F39: the excerpt field shows the stored text; "Derive from body" sends `excerpt: null`.
- F40: a transition with unsaved changes PATCHes first, then transitions with the returned version.
- F41: P5's reason corrected (openapi-typescript is the binding limit).

## M2b-1 controller rulings

- Ruling R1: every pre-flight proposal F1–F41 is accepted as written — each was checked against the code or a scratch install — cost: rework visible in review.
- Ruling R2: P3/P4 (fonts, ember accent), P15 (autosave slot) and P16 (no page-view chart until M4) are accepted as controller rulings the user may override — cost: tokens and doc 11 only.
- Ruling R3: `allowed.delete` ignores children (documented; the admin handles the `422`) — cost: a delete button that can fail.
- Ruling R4: the autosave `parent_id` has snapshot semantics (`null` = top level; the SPA always sends it) — smallest fix, no schema change — cost: an old client that omits it clears the parent in the slot.
- Ruling R5: the `<Toaster>` mounts in the `_app` shell, not in `main.tsx` — avoids a double 429 message on the auth screens — cost: no toasts on auth screens (none needed).
- Ruling R6: no `AnimatePresence` around routes; a keyed enter-only `m.div` — the exit was already instant, and it double-mounted each route — cost: no exit animation.
- Ruling R7: the skeleton pulse is the one looping animation allowed (11 §5), for loading placeholders — cost: none (reduced motion stops it).
- Ruling R8: `tiles_match_counts` compares the tile to `GET …/counts` with polling after a reload, and the dashboard spec runs serially — the tiles show post counts, so the test cannot use its own type — cost: an occasional retry.
- Ruling R9: when nothing trashed could be restored by the actor (`allowed.trash_undoable` false), trash asks first and toasts without Undo — undo over confirm needs an undo to exist — cost: one extra dialog for contributors.
- Ruling R10: `allowed.trash_undoable` (may trash and may restore) is computed in core — capabilities stay in core — cost: one more additive DTO member.
- Ruling R11: a child's permalink that contains an unreadable draft parent's slug is not a leak — the published child is served at that path — cost: one slug visible to staff.
- Ruling R12: `GET …/autosave` answers `204` without a body when the caller has no slot; `404` stays "item unreadable" — cost: one status code in the spec.
- Ruling R13: after Take theirs, new edits are guarded only by `beforeunload` and the banner until a save or discard (one slot per user and item) — cost: a crash in that window loses them; follow-up: a local backup (M2b-2).
- Ruling R14: the leave dialog's Discard while a kept copy exists discards only the new edits and keeps the kept copy — cost: one extra Discard on the banner.
- Ruling R15: on return after that Discard, the kept copy restores as the working draft under the standard restore banner — follows the load rule, nothing lost — cost: kept mode is not remembered across navigation.
- Ruling R16: a second Take theirs replaces the first kept copy — cost: multi-slot would be needed to keep both.
- Ruling R17: `<ins>`/`<del>` in diffs carry no `aria-label` (prohibited by ARIA 1.2 on those roles); visually hidden "added:"/"removed:" text instead — cost: re-add labels if a screen reader needs them.
- Ruling R18 (as built, updated in M2b-3 Task 13): on Apple platforms a letter or digit pressed with Option is matched by its physical key (`e.code`), so `⌘⌥R` runs as `mod+alt+r`; with ⌘ held this holds whatever the browser reports for AltGraph (a ⌘ chord types nothing), and Option alone with AltGraph reported is left to type; elsewhere Alt and AltGr keep typing their letters (unit test `mac_option_reported_as_altgraph_still_runs_cmd_option_r`, a real `KeyboardEvent` with `modifierAltGraph`) — cost: none known; a real Mac browser that reports AltGraph for Option was never available, so `⌘⌥R` on a real Mac stays a manual check with the user's Safari pass. Superseded M2b-1 text: shortcuts match the physical key while Alt is held only on Apple platforms and never with AltGraph — cost: a macOS browser reporting AltGraph for Option would break `⌘⌥R` (unverified).
- Ruling R19: exit check 3 is automated with Playwright against a release build; Safari is listed as not verified, for the user — no human hands in an autonomous run — cost: a Safari-only bug found later.
- Ruling R20: the top-bar "New" split button is global chrome (11 §7's shell has it on every screen), not a view's primary action; it is rendered as a secondary (outline) control with an ember plus, so each view keeps one filled primary ("New entry" on a list, "Publish…" in the editor) — cost: a slightly quieter global New.

## M2b-2 plan rulings

Plan: docs/superpowers/plans/2026-10-26-m2b2-media-and-content-modelling.md (15 tasks).

- Plan ruling P27: M2b-2 is 15 tasks, five Rust API tasks first — every screen needs them — cost: screens wait for the API tasks.
- Plan ruling P28: SEO is `content_meta` `seo.title`/`seo.description` exposed as a `seo` member on content DTOs; an SEO-only write is a full update; `seo.title` replaces the whole `<title>` — the head, SEO plugins and the WP importer share those keys — cost: a revision per SEO-only save; revisions do not restore SEO (P13).
- Plan ruling P29: `@dnd-kit` classic (core 6.3.1, sortable 10.0.0, utilities 3.2.2), not `@dnd-kit/react` 0.x; every drag has a button/keyboard alternative — cost: a later migration to the new API.
- Plan ruling P30: no `allowed` on media DTOs; media actions show by capability and ownership, the server's `403` decides — cost: a button that can fail with a clear message.
- Plan ruling P31: last-write-wins resources get a client-side `staleGuard` re-read before writes, not version columns — cost: a race inside the check-then-write window loses one edit.
- Plan ruling P32: uploads use `XMLHttpRequest` (progress) with a client queue of 3 and one `Idempotency-Key` per file across retries — cost: a second request path next to `openapi-fetch`.
- Plan ruling P33: media get `POST /media/{id}/restore` and `GET /media?trash=true` — the content routes 404 for `media` and the library's Undo and Trash need them — cost: two more endpoints.
- Plan ruling P34: comment pages are pages of top-level threads (roots oldest first, `comments.per_page` default 50), `?cpage=N` in the cache key, past the end → last page; a reply under an unapproved parent is a root — cost: one huge thread makes a long page; as built: amended by R25 (page row budget).
- Plan ruling P35: terms have no manual order (siblings by name; a drag changes only the parent); term and type deletes confirm instead of Undo — cost: no custom category order.
- Plan ruling P36: types without a schema show no fields section (raw object kept; only `featured_image`) — cost: such types' fields are API-only.
- Plan ruling P37: relation fields search every type the nav shows (`FieldDef` has no target type) — cost: a relation can point at an unintended type.
- Plan ruling P38: menu and redirect deletes offer Undo (re-PUT / re-POST; redirect hits restart at 0); type deletes confirm — cost: an undone redirect loses its hits.
- Plan ruling P39: on save the editor drops only top-level field keys the schema no longer has, after listing them — cost: that field's stored value is deleted with the save.
- Plan ruling P40: the R13 local backup is `localStorage` per item and user, only while a kept copy holds the slot, cleared on save, discard and logout — cost: unsaved text stays in the browser until then.

## M2b-2 pre-flight findings

- F-A: screen tasks register their own nav entries; Task 5 builds only `navFor`'s section definitions and capability rules (no entry for a route that does not exist yet).
- F-B: the SEO tests assert whatever status a `PATCH` without `If-Match` already answers; no behaviour change.
- F-D: Task 8 (picker, featured and inline images, fields, SEO, revisions) stays one task but may land in several commits, reviewed as one range.

## M2b-2 controller rulings

- Ruling R21: core's `body` root check (`"type": "doc"`) applies only to a body the request sends or restores (create, `PATCH` with `body`, autosave, revision restore), never to a stored body a `PATCH` leaves alone — the brief refuses a sent body and never rewrites stored ones; blocking SEO-only edits of legacy items would be a gap — cost: a legacy non-`doc` body stays in place (the editor opens it read-only).
- Ruling R22: `GET /media` with `q` and `trash=true` is `422` (field `q`); trashed items stay out of the search index — no lifecycle change for a rarely needed search — cost: no search inside the media Trash view.
- Ruling R23: media restore is the content lifecycle's restore (trash → draft, R8) plus a publish in one `Tx`, `published_at` kept — reuses the lifecycle — cost: plugins see a second `post.published` for a restored upload, and an item unpublished before it was trashed comes back published.
- Ruling R24: media `kind` with `q` is applied to the loaded search hits (the list path filters in the store) — the search path has no store-side kind filter — cost: a search page can be shorter than `limit` while `next_cursor` is set.
- Ruling R25: a comment page always holds every one of its roots; the page's row budget is 2 000 (`MAX_PAGE_ROWS`), and past it only replies are cut (roots first, then descendants in thread order) — Review Focus 5 is binding over P34's "cut after a whole root", which could leave later roots on no page — cost: on a page whose threads exceed 2 000 rows the latest replies are not shown, and a reply's `303` lands on a page without its anchor only then.
- Ruling R26: Media and Comments are items of the Content section; Structure is its own section (Content types, Taxonomies, Menus, Redirects) — WordPress/Ghost grouping, one line to change — cost: none material.
- Ruling R27: `MediaDto` gains `author` (`AuthorRefDto`, as content's P13), so the library names uploaders without `manage_users` — same reasoning as P13 — cost: one `get_many` of users per media page.
- Ruling R28: inline images render the original `/_media/<hash>/<name>`, as the core renderer does; the plan's "variant URL" was wrong and the renderer is unchanged in M2b-2 — cost: no responsive `srcset` for inline images until the renderer gains it (follow-up).
- Ruling R29: P39 extends to repeater rows: a save drops sub-keys a row's schema no longer has, at any depth, listed beforehand in the stale notice as `chapters[].extra` — same reasoning as P39 — cost: those sub-values are deleted on save.
- Ruling R30: number fields accept only `.` as the decimal separator; a comma is refused with a field message — the admin is English-only and no wrong value is stored silently — cost: users type 1500, not 1,500.
- Ruling R31: term delete follows core: a term with sub-terms cannot be deleted; the dialog names the count and points to Move to… — core is the authority; the brief assumed WordPress's "children move up" — cost: users move sub-terms first.
- Ruling R32: the menu editor's depth limit is the public renderer's `MAX_MENU_DEPTH` (16 levels; core has none), and custom links follow core's `is_allowed_url` (no `mailto:`) — core and the site renderer are the authority — cost: no `mailto:` links until core allows them.
- Ruling R33: `GET /content/refs?ids=…` names items by id across types (readable items only, `{id, type, title, status, permalink}`), used by relation fields, menu targets and the comments queue's item chip — removes the type-by-type `404` probing in three screens — cost: one more endpoint.
- Ruling R34: a signed-in commenter whose role holds `moderate_comments` is exempt from the per-address comment rate limit (decided in core) — a moderator replying to a batch from the queue would hit `429` — cost: a compromised moderator session can post comments without that limit (moderators' comments are approved anyway).
- Ruling R35: free-text search parameters (`q`) stay strings app-wide (a router-level codec), so a search for `2024` or `531038e6` survives a reload — TanStack Router JSON-decodes search values — cost: one shared codec to keep.
- Ruling R36: `refs` is a reserved type and taxonomy name — the static `/content/refs` route would shadow a type's list — cost: a pre-existing type or taxonomy named `refs` has no migration path (follow-up).

## M2b-3 plan rulings

Plan: docs/superpowers/plans/2026-10-27-m2b3-administration-and-m2-exit.md (16 tasks).

- **P41:** M2b-3 is 16 tasks.
  - Four Rust API tasks come first, because the screens consume them.
  - Two follow-up tasks, then the E2E matrix, the bundle budget and the exit.
  - Cost if wrong: the screens wait for the API.
- **P42: Theme upload.** The server keeps its canonical bundle format (no wider untrusted parser).
  - The admin packs a picked or dropped folder into the canonical ustar in the browser. The packer is a TS port of `bundle::pack`, pinned byte for byte against a fixture the Rust packer wrote.
  - A `.tar` file can also be sent as it is; the server's refusal is shown.
  - `ferrite theme pack --dir DIR --out FILE` gives API users a valid file.
  - Cost: two packers to keep equal (the fixture test pins them).
- **P43: An admin can switch off another user's TOTP** (`POST /users/{id}/totp/disable`).
  - Not for oneself (`/auth/totp/disable` with a password).
  - The target's sessions are kept; audited.
  - There are still no recovery codes.
  - Cost: an admin account can remove a second factor without the owner (it can already set their password).
- **P44: Changing one's own e-mail needs `current_password`**, because the e-mail is the reset channel.
  - An admin changing someone else's e-mail does not need it.
  - Cost: one more field in the account form.
- **P45: Sessions:** `GET /auth/sessions`, `DELETE /auth/sessions/{id}`, `POST /auth/sessions/revoke-others`.
  - A session is shown by an opaque id (not the cookie value or its stored hash), created and last-seen times, and `current`.
  - No user agent is stored, so none is shown. No migration in M2b-3.
  - Cost: sessions are told apart only by time.
- **P46: User list filters** `q` (login, display name or e-mail substring), `role`, `status`.
  - Delete asks for a reassignment only when the server's `422` says the user owns content (field `reassign`). There is no count endpoint.
  - Cost: one refused request before the reassign picker shows.
- **P47: Plugin data retention becomes visible.**
  - `GET /plugins/retained` lists deleted plugins whose data is still kept, with the purge date.
  - `POST /plugins/{name}/purge` erases it early. A `422` (field `name`) while a plugin of that name is installed.
  - Cost: one store query on both backends.
- **P48: Hook latency is shown as a p95 upper bound** from the existing six buckets ("p95 ≤ 25 ms"), labelled "This node, since <start>".
  - No host change; cluster-wide series stay M4.
  - Cost: a coarse number.
- **P49: Mail stays config-only.**
  - `GET /system/status` gains `mail: { configured, from }` (never the SMTP URL).
  - `POST /system/mail/test` sends one mail to the caller's own address (`manage_system`; 3 per 10 minutes per user; audited `mail.test`; `422` when mail is not configured).
  - Settings shows a read-only Mail panel with that test.
  - Cost: no mail settings in the UI.
- **P50: Media sizes are the active theme's `[images]`, shown read-only** in Settings from `ThemeDto.images`.
  - No size setting; regenerating variants stays M4.
  - Cost: changing sizes needs a theme change.
- **P51: The TOTP issuer and the password-reset mail read the `site.title` option at use time**, falling back to `ferrite.toml`. This closes the split between Settings and the startup value.
  - Cost: one option read per enrolment or mail.
- **P52: Relation fields gain `types: string[]`** in `FieldDef` (empty = any type, the M2b-2 behaviour).
  - Core refuses an item of another type on write (`422` at the field's path).
  - The field builder and the picker use it.
  - Cost: one item read per relation value on write (batched per save).
- **P53: A trashed media item remembers its status** in `content_meta` `media.trashed_from`, written in the trash `Tx`.
  - Restore re-applies it: `published` only if it was published, else `draft`.
  - Items trashed before this change restore as published (R23 behaviour).
  - Cost: one meta row per trashed media item.
- **P54: R34 is closed by ruling.** Moderators keep no comment rate limit: they are trusted, audited accounts, and a compromised one is disabled.
  - **R36 is closed by ruling.** No released site predates M2b-2, so no migration path for a `refs` type is built; the ROADMAP says so.
  - Cost: none for existing sites.
- **P55: E2E matrix.** CI runs every spec in four parallel jobs: `chromium-sqlite`, `chromium-pg`, `firefox-sqlite`, `webkit-sqlite`.
  - Design snapshots (`snap()` and the contact sheet) run only in `chromium-sqlite`.
  - A spec or step that cannot run in an engine is skipped with `test.skip(browserName === …, "<reason>")`, and every skip is listed in 10 §3.
  - Linux WebKit is not Safari: a manual Safari pass stays in the exit.
  - Cost: about four times the E2E CI minutes, in parallel wall-clock.
- **P56: Bundle budget.** `admin/bundle-budget.json` holds gzip -9 limits per group (entry, entry plus modulepreloads, main CSS, editor chunk, chart chunk, largest other lazy chunk).
  - Each limit is the M2b-3 size measured in Task 15, plus 10 %, rounded up to the next KiB.
  - `pnpm -C admin bundle:check` fails CI over a limit. Raising a limit is a reviewed edit of that file.
  - Cost: a deliberate step for growth.
- **P57: The explicit-theme flash is fixed server-side.**
  - The admin writes a cookie `ferrite_admin_theme=light|dark` (`Path=/admin`, `SameSite=Lax`, one year, `Secure` on https; removed for `system`). As built (Task 13): the path is `/admin`, not `/admin/` as first written; by RFC 6265 path-match (§5.1.4) `/admin` covers `/admin` itself and every `/admin/…` path, and nothing else (`/administrator` does not match).
  - Serving `index.html` adds `class="dark"` or `class="light"` to `<html>` from it, only for exactly those values, as a fixed string (never the cookie's text). As built: `index.html` sends `Vary: Accept-Encoding, Cookie`, and a rewritten page gets the ETag of its own bytes.
  - No inline script is added, so the CSP is unchanged.
  - Cost: `index.html` is rewritten per request (it is `no-cache` anyway).
- **P58: Settings screens are last-write-wins** (`staleGuard`, P31).
  - A permalink change confirms first, saying every post or page URL moves and no redirects are created (M3).
  - Cost: a dialog on a rare change.

## M2b-3 pre-flight findings

- Ruling A: `MeDto` gains `mail_configured: boolean | null` (a boolean for actors holding `manage_users` or `manage_system`, else `null`), added in Task 3; the Users screen offers "Send a reset e-mail" only when it is true — `/system/status` stays `manage_system`-only and the public reset answers `202` regardless — cost: one more `MeDto` member.
- Ruling B: Task 13 does not touch `admin/src/structure/types/**` or `admin/src/editor/**` (Task 12's); the type builder's schema-change minors move to Task 12 — cost: Task 12 slightly bigger.
- Ruling C: the E2E jobs keep the example plugin's `wasm32-wasip2` build (as built in Task 14: built once in the `admin` job and uploaded as the `admin-e2e-plugin` artifact) — cost: none.

## M2b-3 controller rulings

- Ruling R37: a bearer request to `/auth/sessions` (list, revoke, revoke others) answers `400`, like `logout` (session-only routes) — consistency — cost: API clients see `400`, not `401`/`403`.
- Ruling R38: `theme.toml` gains the optional `homepage` key (`author` and `license` already existed) — `ThemeDto` needs a source for its facts — cost: a manifest format addition (06 §1).
- Ruling R39: during a mixed-version rollout an older node refuses a `theme.toml` with the new key — no release exists yet — cost: the fallback theme on old nodes until they are upgraded.
- Ruling R40: three Task 3 review minors are accepted as they are: the `mail.test` audit row is written after the mail is queued; the configured site title is copied in both `ApiState` and `AuthService`; mail configuration is per node — cost: an audit row for a mail that later fails to send (the failure is in the node log), and one more copy to keep in step.
- Ruling R41: an inline image's `srcset` keeps only variants with the original's aspect ratio (crop variants are left out) — a cropped variant would change the picture — cost: fewer `srcset` entries for themes whose sizes all crop.
- Ruling R42: a type named in another type's relation `types` cannot be deleted (`422`) — the other schema would dangle and its next save fail — cost: delete order matters.
- Ruling R43: the redirect `q` search stays a table scan (no index in M2b-3; redirect tables are admin-sized; a substring `LIKE` cannot use a b-tree index and a trigram index is Postgres-only) — cost: slower search on very large redirect sets; carried to M4.
- Ruling R44: `MediaKindParam` stays (`ferrite-domain` has no `utoipa`; DTO mirrors of domain enums are the API convention) — cost: one `From` impl to keep in step.
- Ruling R45: the comment helper's `prev_url`/`next_url` carry no `#comments` (the anchor is theme markup; `blog` adds it, R4) — cost: other themes add their own anchor.
- Ruling R46: comment totals and the page are read outside one `Tx` (no read-only `Tx` exists; one would take SQLite's writer on a public path; the page cache's staleness guard covers the race) — cost: a transient off-by-one count under concurrent moderation.
- Ruling R47: media restore's `!may_publish` restore-as-draft branch stays untested (unreachable with the built-in roles) — cost: none today.
- Ruling R48: `SiteReader::set_image_width` is node-wide (one `AppState` per reader in production) — cost: only tests could see a mix.
- Ruling R49: the Account link in the user menu and the palette land with the Account screen (Task 6), not the kit (Task 5) — the route exists only there — cost: none.
- Ruling R50: Plugins is `g l`, because `g p` is Posts — no collision — cost: a less mnemonic chord (11 §6).
- Ruling R51: the stale dialog names the resource (`staleKey`) — cost: none.
- Ruling R52: secrets (a new token, the TOTP enrolment, plugin secret settings, passwords) never live in TanStack's mutation cache: plain awaited calls held in component state; the E2E checks `queryClient.getMutationCache()` too — cost: these calls do not use the shared mutation helpers.
- Ruling R53: the E2E reaches the query caches through React's fiber tree (`e2e/support/app-state.ts`), not a test hook — a hook would need a second build, and the E2E tests the shipped bundle — cost: tied to React internals.
- Ruling R54: user delete's refusals carry `errors[0].field` `reassign` (core `invalid_field("reassign")` at four sites, Task 7) — the reassign dialog needs a field to switch on; status and slug unchanged — cost if wrong: one field at four error sites.
- Ruling R55: one's own row in Users has no profile form either; Account owns the own e-mail change with `current_password` — consistent with H-A1 — cost: one link click.
- Ruling R56: the Users screen's "Send a reset e-mail" path is covered by a unit test only (the E2E checks the option shows) — cost: that wiring is untested end to end in the browser (the reset mail itself reached a real SMTP sink in the M2b-1 exit, `m2b1-exit.md` §3).
- Ruling R57: the role-change confirm says tokens stay (they are not revoked): a role's capability is checked before a token's scope, so a demoted user's tokens are capped by the new role (core `users.rs`, 07 §1) — cost if wrong: one string.
- Ruling R58: `/admin/settings` opens General, and `/admin/system` opens Status — cost: none.
- Ruling R59: the permalink presets are "Post name", "Month and name" and "Category and name" (WordPress's "Plain" is `?p=`, which has no token here) — cost: copy only.
- Ruling R60: `settings/permalink-preview.ts` repeats core's parse rules; the server's `422` is the backstop — cost: the preview can drift from core.
- Ruling R61: an emptied General field sends `null` (the `ferrite.toml` value applies) — the settings `null` semantics — cost: a tagline set in `ferrite.toml` cannot be blanked from the admin.
- Ruling R62: site-wide E2E specs (`settings`, `themes`, `system`) run apart from the parallel ones (as built in Task 14: each project's `-site-wide` teardown, one worker) — they change settings, the active theme and the job queue — cost: a serial tail per project.
- Ruling R63: Themes: when the activation poll (20 s) ends before every node reports, a non-error note shows; `bundle::pack` is the authority over the plan's prose for the browser packer; a homepage is a link only when it parses as `http(s)`; the E2E restores the active theme in `finally` — cost: none.
- Ruling R64: the theme E2E proves the packer's bytes by the stored theme's hash = SHA-256 of `blog-min.tar` — equivalent to a byte compare — cost: none.
- Ruling R65: a `.tar` whose `theme.toml` name cannot be read gives no client-side replace warning; the server decides — rare — cost: a missing warning for an odd `theme.toml`.
- Ruling R66: the theme settings sheet and the plugin settings ask before discarding unsaved changes, as Settings does — consistency — cost: one dialog.
- Ruling R67: a `purge=true` plugin delete writes `plugin.<name>.erased_at` beside the deletion marker, so it is not listed as retained; the marker is written after the immediate erase succeeds and is cleared on reinstall and by the final sweep — cost: an extra option row until then.
- Ruling R68: plugin metrics refetch every 30 s only while the page is visible — cost: none.
- Ruling R69: the audit screen maps 29 known actions to words with no core-sync test (a regex over core's strings also matches job kinds, events and hooks) — cost: a new action shows its raw code until mapped.
- Ruling R70: follow-ups that turn out not to be bugs are proven by a test or a written explanation, not changed (Task 12: the editor route's `${id}:0` key) — cost: none.
- Ruling R71: the editor-fields E2E's relation names its own target types, so a parallel test's deleted type is never searched (the M2b-2 exit's race) — cost: the "any type" search is covered by the palette and the menu editor instead.
- Ruling R72: CI is not run per branch: no push in M2b-3; the workflow runs on the final develop→main pull request (or a manual `workflow_dispatch`); `ci.yml`'s `on:` is develop's — cost: CI findings arrive late.
- Ruling R73: CI does not cache Playwright's browsers (Playwright advises against it) — cost: slower E2E jobs.
- Ruling R74: four Task 14 review minors are accepted: the WebKit reload retry acts only on "Frame load interrupted"; filtered local runs skip the site-wide ordering (documented); the wrapper's argv sizing is slower, not broken; one fix is red by reasoning, not by a run — cost if wrong: a hidden WebKit reload bug or local flakes.
- Ruling R75: in the bundle budget, `entry_preloads` includes the entry (`entry` is a subset) — by design (P56) — cost: none.
- Ruling R76: the M2b-3 exit is split: part A (docs, the exit flows over the REST API on a release build, the workspace tests on both backends, the gates) runs now; part B (the four E2E projects twice with flake counts, the contact sheet, `docker build` serving `/admin/`, the Linux Chromium run, actionlint) runs after a host reboot, because no browser engine or Docker daemon starts on the host (Task 14) — cost: the exit documents are edited twice.
