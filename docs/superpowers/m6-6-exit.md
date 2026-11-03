# M6-6 exit: outgoing webhooks

Branch `m6-6-webhooks` (from `m6` at 1cbef9c). Design: [18](../design/18-webhooks.md). Plan:
[`plans/2026-11-02-m6-6-webhooks.md`](plans/2026-11-02-m6-6-webhooks.md). Ruling P2
([`rulings/m6.md`](rulings/m6.md)) applied: scopes `webhooks:read`/`webhooks:write` and the
`manage_webhooks` capability after `system`; `release.*`, `change_set.reverted` and
`health.completed` are left for wave B (the catalogue is the one place they are added).

## 1. What was built

- **Storage** (`0016_webhooks`, both backends, expand, not-lossy down): `webhook_endpoints`,
  `webhook_events`, `webhook_deliveries`, `webhook_attempts`; `WebhookRepo` with
  `active_for_emit` (enabled and the database's `install_id`, `FOR KEY SHARE` on Postgres),
  `get_endpoint_for_update` (`FOR NO KEY UPDATE`), the conditional `record_attempt`, keyset
  paging of the log, counts, `deliveries_since`, `fail_stale_pending` and `prune_events`.
  `webhook_endpoints` is exported (sealed secrets travel as stored); the log tables are
  skipped.
- **Core** (`ferrite_core::webhooks`): the event catalogue and pattern matching, Standard
  Webhooks v1 signing (known answer from the spec's library tests), `whsec_` secrets sealed
  per endpoint, URL rules, the 8-step schedule with jitter and `Retry-After`, the
  `WebhookTransport` trait (+ `NoTransport`), `WebhookService` (CRUD, rotation with a 24 h
  overlap, test deliveries limited to 10/user/min, the log, redelivery, the attempt state
  machine: `410` and 5-day + 10-failure auto-disable, paused copies, unreadable secrets,
  abandoned deliveries) and `emit` / `emit_content` / `emit_comment` called in the write
  `Tx` of content (create, update incl. revision restore, transitions, scheduled publishing,
  delete), media (upload, update, trash, restore, delete), comments (submit, a real status
  change, delete), users (create on both paths, update, delete) and imports (finish).
  Capability `manage_webhooks` (admins), scopes never in `Scopes::OAUTH`.
- **HTTP**: `webhook_send::WebhookClient` over the egress checked resolver (every resolved
  address checked, peer re-checked, IP literals, no redirects, no proxy, `https` only unless
  `local_targets`, 15 s, 4 KiB read, 1 KiB excerpt, `Retry-After`, class-only errors, host-only
  logs); `is_local_target_ip`; `[webhooks] local_targets` (+ env, warning, `config check`
  `webhooks=off|local`); `/api/v1/webhooks` (12 routes, tag `webhooks`, OpenAPI and
  `schema.d.ts` regenerated).
- **Binary**: `webhook.deliver` (one job per attempt, `max(1, workers-1)` slots, `Busy`
  refunds, 30 s), hourly `webhooks.prune` (abandoned pendings failed after 6 h, then events
  older than 30 days in batches within 240 s), the auto-disable mail to active admins;
  doctor's `webhooks.endpoints` (26 checks).
- **Admin**: `/admin/webhooks` (`g w`, palette "New webhook") and `/admin/webhooks/$id`;
  token dialog area, audit texts. E2E `e2e/webhooks.spec.ts` with a local sink.
- **Docs**: guide `site/content/02-guides/09-webhooks.md`, REST/ops/reference pages,
  `VERIFIED.md` rows, design 00/01/07/09/11/18, CLAUDE.md, roadmap.

## 2. Verified

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`: pass.
- `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace`:
  see §5 (the final run after the review fixes).
- Store contract tests on both backends (10 webhook tests); core service (18 × 2 backends)
  and emit (10 × 2) tests; transport hostile tests (12); API tests (8 × 2); node tests
  (5 × 2, real runner + local receiver); cluster `two_nodes_deliver_each_attempt_once`
  (100 events, each `webhook-id` once); doctor tests.
- Admin: `pnpm -C admin typecheck`, `lint`, `test` (845), `build`, `bundle:check`: pass.
- E2E `webhook_end_to_end` on `chromium-sqlite`, `chromium-pg`, `firefox-sqlite`,
  `webkit-sqlite`: pass; the whole `chromium-sqlite` suite with `local_targets` on: pass.
- Manual run on a release build (§4): signatures verified with the guide's own Node and
  Python code on `media.uploaded`, `user.created`, `content.created`, `content.published`,
  `comment.created`; retry attempt 1 at +0.5 s and 2 at +6.5 s with the same `webhook-id`;
  `410` → disabled `gone`, audit with host only; doctor warnings; `export` + `import-site` →
  doctor reports the copied endpoint paused.

## 3. Execution rulings (R-M6-6-n)

- **R-M6-6-1** `0016` down is not lossy (as 0011–0013): the log is transient and endpoints
  are configuration an admin registers again.
- **R-M6-6-2** Catalogue entries appended at the end of `TABLES` (FK-valid, merge-friendly),
  not after `users`.
- **R-M6-6-3** The trait lives in `ferrite-store/src/webhook.rs`; added `attempts()`,
  `DeliveryListed` (event type for the log), no `NewWebhookEndpoint` (insert takes the row);
  the snapshot test derives the two newest versions (the 14/15 gap).
- **R-M6-6-4** `get_endpoint_for_update` so the attempt `Tx` and admin edits never overwrite
  each other (after review: `FOR NO KEY UPDATE`, and `active_for_emit` `FOR KEY SHARE`).
- **R-M6-6-5** A test delivery gets one attempt, no retries (it still feeds health; a `410`
  to a ping disables an enabled endpoint, which is what the receiver asked).
- **R-M6-6-6** Redeliver refuses a disabled or paused endpoint (`422`); `redeliver_since`
  refuses `pending`.
- **R-M6-6-7** `NoTransport` lives in core next to the trait; `Delivered` gains
  `Skipped(reason)`; state precedence disabled > paused > failing > active.
- **R-M6-6-8** Content events are emitted from the public `create` / `transition` wrappers,
  `update_in` and `delete_in`, so `seed demo` and uploads (which use `create_in` /
  `publish_in`) emit none; a media permanent delete becomes `media.deleted` in
  `emit_content`; `emit_comment` reads the item type only on a match.
- **R-M6-6-9** Transport tests use a plain-HTTP receiver under `local_targets` for positive
  cases, a map resolver with the strict rule for hostile ones, a 500 ms timeout seam.
- **R-M6-6-10** Serve-level tests run in-process (`tests/node/webhooks.rs`, `NodeOptions`
  seams for schedule, transport and mailer) rather than the child-process `tests/serve`
  harness, which has no seams.
- **R-M6-6-11** "OAuth token refused" is covered by a token holding every `Scopes::OAUTH`
  scope (`403 webhooks:read`) plus the existing REST audience test; an unknown status filter
  is `400 malformed-query`.
- **R-M6-6-12** Doctor check in `doctor/webhooks.rs`; the seven planned cases in three tests.
- **R-M6-6-13** Admin tasks 8 and 9 in one commit (the list routes into the detail);
  `webhook-form.tsx`, `webhook-dialogs.tsx`, `deliveries.tsx` hold what the plan split into
  more files.
- **R-M6-6-14** (review fix) Deliveries whose job gave up stay `pending` for good; the hourly
  prune fails `pending` deliveries whose next attempt was due more than 6 h ago ("the
  delivery job gave up"), so their events prune and they can be redelivered.

## 4. Manual run outputs

Scripts: `manual.sh`, `manual2.sh`, `receiver.py`, `verify.py`, `verify.mjs` (session
scratchpad). Key lines:

```
secret prefix whsec_ state active
--- GET never shows the secret:  0
media.uploaded 200 sigs=1 [True] -
user.created 200 sigs=1 [True] -
content.published 200 sigs=1 [True] http://127.0.0.1:8197/hello-webhooks/
content.created 200 sigs=1 [True] None
headers: user-agent 'ferrite/0.2.0 webhooks', webhook-id, webhook-timestamp, webhook-signature 'v1,…', content-type 'application/json'
content.created 503 at +0.5s 01a19ea7-4051-…
content.created 503 at +6.5s 01a19ea7-4051-…
delivery pending attempts 2 next 2026-11-02T23:50:30Z err status 503
state disabled reason gone
[('webhook.auto_disabled', '127.0.0.1'), ('webhook.test', '127.0.0.1'), ('webhook.create', '127.0.0.1')]
ok     config.load  … webhooks=local
warn   webhooks.endpoints  [webhooks] local_targets is on: …; "Local" (127.0.0.1) was disabled automatically (gone)
comment.created pending Ann False {'id': '01a19ea8-…', 'type': 'post'}   (no e-mail in data)
--- doctor on the copy: warn webhooks.endpoints "Comments" (127.0.0.1) is paused: this database was copied or restored; …
```

## 5. Not verified

- The auto-disable mail through a real SMTP server (a recording mailer in the node test; the
  E2E SMTP sink was not wired to a webhook failure).
- The official `standardwebhooks` JS and Python libraries (not installed); the guide's
  by-hand checks were run against real deliveries.
- Real Netlify / Vercel / Cloudflare deploy hooks, a real CDN purge, a public request
  inspector over the Internet, IPv6-only receivers, real TLS receivers in the manual run
  (TLS is covered by the plugin egress tests that share the client builder).
- The retry schedule beyond the first two steps end to end (5 min … 14 h by unit tests);
  the 5-day auto-disable end to end (by unit test with a back-dated `failing_since`).
- `restore` and `db copy` pausing endpoints (by the M4-1 install-id rule; only
  `import-site` was run).
- A rolling upgrade with mixed versions (documented gap, design 18 §10.4).
- The Postgres race between an emitting write and an endpoint delete (fixed by `FOR KEY
  SHARE` after review; no test reproduces the interleaving).
- Admin screens in a real Safari and on mobile devices; review screenshots.

## 6. For the merge

- Migration number 16 leaves a gap at 14/15 until M6-1 and M6-2 merge; tests that list
  versions were moved to `…, 13, 16]`; the controller adds 14/15 when merging.
- M6-1 adds `locale` / `translation_group` to the content payload (design 18 §3.2) in
  `webhooks/emit.rs::content_data`.
- Shared files touched additively: `Repos` (`webhooks()` last), both stores' `repos/mod.rs`,
  `catalogue.rs` (end), `actor.rs` (area, scopes, capability appended), `ApiParts`/`ApiState`
  (`webhooks` last), `serve.rs`, `jobs.rs`, admin nav/palette/tokens/audit-text, i18n.
- Wave B: add `release.*`, `change_set.reverted`, `health.completed` to `EVENT_TYPES` and
  call `webhooks::emit` in their write `Tx`.
