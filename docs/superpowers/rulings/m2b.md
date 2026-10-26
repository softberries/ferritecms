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
- Ruling R18: shortcuts match the physical key while Alt is held only on Apple platforms and never with AltGraph — cost: a macOS browser reporting AltGraph for Option would break `⌘⌥R` (unverified).
- Ruling R19: exit check 3 is automated with Playwright against a release build; Safari is listed as not verified, for the user — no human hands in an autonomous run — cost: a Safari-only bug found later.
- Ruling R20: the top-bar "New" split button is global chrome (11 §7's shell has it on every screen), not a view's primary action; it is rendered as a secondary (outline) control with an ember plus, so each view keeps one filled primary ("New entry" on a list, "Publish…" in the editor) — cost: a slightly quieter global New.
