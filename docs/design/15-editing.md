# 15 — Editing: live preview, releases and approval workflows

Status: **built (M6-3, see §19 and `docs/superpowers/m6-3-exit.md`)** · Date: 2026-11-02 · Roadmap: "M6-3 — Editing" · Migration
`0017_releases_workflows` · Wave B (assumes M6-1's locale model and M6-6's webhooks)

Three features that turn the editor from "write, then publish" into an editorial process:

1. **Live preview beside the editor**: the item's unsaved state rendered by the active theme,
   in a pane next to the text, refreshed on every autosave.
2. **Releases**: a named set of changes (staged new content, publish, unpublish) applied
   together in one transaction, now or on a schedule, with a **whole-site preview** that
   shows the site as it will look once the release is applied: menus, listings, the page
   tree, feeds and the sitemap.
3. **Approval workflows**: per content type, ordered review steps with required approvals
   before anything of that type reaches the public site; reviewer assignment and mail
   notifications. It extends the existing submit-for-review (`draft → pending`).

It also closes the M5 follow-up "no signed, expiring preview links" (roadmap M5
follow-ups, Task 6 ruling): **preview links** are revocable, expiring tokens that open an
item or a release preview without a session.

Read with: 02 (storage), 04 (cluster: jobs, outbox), 06 §5 (render pipeline), 07 (API and
admin), 11 (admin UI rules), 12 (MCP). Where this document and the roadmap disagree on the
*how*, this document wins; numbered decisions are **D1…D40** (§18).

## 1. Goals and non-goals

Goals:

1. **Nothing reaches the public site by accident.** A release is applied all-or-nothing; a
   workflow-gated type publishes only approved content or an audited admin override.
2. **Same paths as today.** Releases apply through the existing core write paths
   (`ContentService::update_in`, `publish_in`, the lifecycle `transition_item`), so
   permalinks, redirects, revisions, search, plugin action jobs and cache keys behave
   exactly as a hand-made edit would.
3. **The preview is the site.** The whole-site preview renders through the real
   `SiteReader`, theme and router, not a reimplementation.
4. **Both profiles.** Solo (SQLite) and cluster (Postgres, N nodes). Everything shared
   lives in the database; previews keep no node state.
5. **Capabilities in core.** No handler or MCP tool checks rights; services do.

Non-goals (M6-3): staging menus, terms, settings, redirects or theme changes in a release
(content only, D2); real-time co-editing; comments on reviews beyond one comment per
decision; per-locale workflows; auto-publish on final approval (D38); visual diffs of
rendered pages; plugin hooks for releases or reviews (D32).

## 2. Concepts

| Term | Meaning |
|---|---|
| Working copy | The caller's autosave slot (`autosaves`, M2b P15), never a revision. |
| Staged change | New content for an item held in a release entry (`release_items.changes`): what the item will look like after the release. Not a revision until applied (D1). |
| Release | A named set of entries, `open` → `scheduled` → `applied` (or `failed`). |
| Entry | One item in one release: optional staged change + a transition `none`, `publish` or `unpublish` (D2). |
| Base version | The item's `version` when the entry was added or last rebased; an entry conflicts when the item's version moved (D4). |
| Workflow | A content type's ordered review steps (`workflows`). |
| Review | One round of approval of a *target*: an item's current revision, or a release entry's staged change (D18). |
| Preview link | A revocable, expiring token (`ferp_…`) that opens a preview without a session (§7). |
| Sandbox | A database transaction in which a release is applied, the site rendered through it, then rolled back (§5). |

## 3. Live preview beside the editor

### 3.1 What it renders

The editor's preview pane loads `/_preview/<id>?working=1` in an iframe. The handler is
today's `/_preview/<id>` (`ferrite-http/src/app/handler.rs` `preview`, the session cookie
only, `SiteReader::preview`'s `can_read` rule R17), with one change: with `working=1`,
`SiteReader::preview_working(actor, id)` reads the actor's own autosave slot of the item
(`AutosaveRepo` through `ContentService::get_autosave`'s rules) and overlays it **in
memory** on the stored item before hydration (D14):

- `title`, `excerpt` (`None` → derived from the body, as `excerpt_of` does on save),
  `body`, `fields` (which carries the featured image, `hydrate.rs` `FEATURED_FIELD`), and
  the SEO patch (`Autosave.seo`, applied over `Seo::load` for the head: `HeadPage` gains an
  optional `seo` override that `page_head` uses instead of the meta read);
- `slug`, `parent_id`, `menu_order` and `terms` show **as saved**: they change the URL,
  breadcrumbs and term links, which a working copy cannot yet claim (the slug may be taken,
  the parent may be refused).

No slot, or `working` absent: the stored item, as today. The overlay never validates or
writes, so a half-typed working copy (an invalid field, an empty title) still previews.
`Previewed` gains a `source: PreviewSource { Stored, Working, Staged }` so templates and
plugins see the same `request.preview = true` as before.

`/_preview/<id>?release=<rid>` does the same with the staged change of the item's entry in
release `rid` (the actor must be able to read the release, §4.7). The editor's change mode
(§11.2) uses it.

### 3.2 Refresh

The autosaver (`admin/src/editor/autosave.ts`, `createAutosaver`) reports each completed
`PUT …/autosave`; the pane reloads the iframe after each one. While the pane is open the
autosaver runs with a 2 s idle debounce instead of the 10 s interval (D15): at most one
`PUT` per 2 s, only while the draft changed. The pane keeps the scroll position across
reloads: the iframe is same-origin (D16), so the pane reads `contentWindow.scrollY` before
the reload and restores it on `load` (clamped to the new document height).

A new item has no id and no slot until its first save: the pane shows "Save once to
preview" with the Save button.

### 3.3 Layout

- **≥ 1280 px** (`SPLIT_QUERY`): split view. Writing surface left, preview right, a
  draggable divider (keyboard: arrow keys move it 5 %, `Home`/`End` to the limits; width
  stored per user in `localStorage`, 35–65 %). The side panel becomes the sheet it already
  is below 1024 px (`useWide`), opened from the bar.
- **< 1280 px**: a segmented control "Write | Preview" in the editor bar swaps the writing
  surface for the pane; the side panel keeps its current behaviour.
- Device widths: Desktop (100 %), Tablet (768 px), Phone (390 px); narrower widths are
  centred in the pane with a 1 px border; the choice is stored per user.
- Toggle: a "Preview" button in the editor bar and `mod+alt+p` (the shortcut sheet lists it).
  Focus mode hides the pane.
- The iframe has `title="Preview of <item title>"`; a status line under it says "Updated
  12:04:31" or "Showing the saved version" (no slot) and is `aria-live="polite"`.

## 4. Releases

### 4.1 Model

A release has a title (1–200 characters), an optional description (≤ 2000), a status, and
up to **200 entries** (`MAX_RELEASE_ENTRIES`, D6). Statuses (D35):

| Status | Entries editable | Next |
|---|---|---|
| `open` | yes | `scheduled` (schedule), `applied` (apply now), deleted |
| `scheduled` | no (unschedule first) | `open` (unschedule), `applied`, `failed` |
| `failed` | yes | as `open`; `failure` holds the last report |
| `applied` | no | none (history, kept) |

Every write bumps `releases.version` (optimistic lock, `If-Match` on the API).

### 4.2 Entries and staging

An entry is unique per (release, item) and holds:

- `changes`: optional **staged change**, a full snapshot of the editable fields as
  `ContentDraft` has them (`title`, `slug`, `parent_id`, `excerpt`, `body`, `fields`,
  `menu_order`, `terms`, `seo`), stored as JSON (D1). It is validated when staged with the
  checks `update_in` runs that need no write (`check_body`, `check_seo`, `check_item` for
  the field schema and relations, the slug syntax) and again, fully, when applied;
- `transition`: `none`, `publish` (only for an item in `draft` or `pending`) or `unpublish`
  (only for `published` or `private`). An entry needs a change or a transition. A
  `scheduled` item cannot be added (`Invalid("unschedule it first")`);
- `base_version`: the item's version when added; `entry_version`: bumped on every change
  of the entry (reviews bind to it, §6.2).

New pages a release publishes are created as drafts first (the normal create) and added
with `publish`. Trashed items cannot be added.

### 4.3 Conflicts and locks

- **Conflict** (D4): an entry conflicts when the item's `version` is no longer its
  `base_version` — someone saved or transitioned the item since. This holds for
  publish-only entries too: a reviewer approved what they saw. `GET …/check` lists
  conflicts; **Rebase** (`POST …/items/{item}/rebase`) sets `base_version` to the current
  version and keeps the staged change (the admin shows a diff of the item's current saved
  content against what the entry was based on, using `admin/src/editor/diff.ts`). A rebase
  bumps `entry_version`, so an approval of the entry goes stale.
- An item may sit in several open releases (D3).
- **Locks** (D5): scheduling a release inserts one `release_locks` row per entry
  (`item_id` primary key, so an item is in at most one scheduled release on every backend).
  While locked, the public service methods refuse every change to the item outside the
  release with `CoreError::Locked { release }` (409 `release-locked`, naming the release):
  `ContentService::update`, the lifecycle methods (`publish`, `unpublish`, `schedule`,
  `submit`, `trash`, `restore`, `delete`) and `restore_revision`. The autosave slot is not
  locked (it is not content). Imports, `seed` and other system paths (`ImportBatch`,
  `DemoBatch`, `publish_due`) do not check locks; a scheduled release whose item they
  changed fails with a conflict (documented in the import guide). Unscheduling, applying
  or failing removes the locks.

### 4.4 Apply: one transaction

`ReleaseService::apply(actor, id, expected_version, override)` runs in **one transaction**
on both backends (D6):

1. Re-read the release (version check) and its entries in position order; status must be
   `open`, `failed` or (system path) `scheduled`.
2. For each entry: read the item; `version != base_version` → `ReleaseConflict`; if the
   type has a workflow, require approval (§6.3); with `changes`, call
   `ContentService::update_in(tx, rules, actor, Edit { id, expected_version: base_version,
   draft, note: Some("Release: <title>") })`; then `publish_in` or the new
   `unpublish_in` (a `pub(crate)` twin of `trash_in`) on the saved item. Every permission
   check is the item's own (`can_edit`, `publish_own`/`publish_others`).
3. Mark the release `applied` (`applied_at`, `applied_by`), delete its locks, delete its
   preview links (§7), append audit `release.apply` (entries and actions in `details`;
   `release.override` too when an override was used), emit the M6-6 event
   `release.applied` (§13), enqueue `editorial.notify` (`release.applied`, to the creator
   when someone else applied it).
4. Append **one** `invalidate` event with the union of every key the entries emitted (D7).

Any error rolls back everything; nothing is half-applied. SQLite holds its single writer for
the transaction (200 entries ≈ a few hundred ms, the same order as an import batch);
Postgres holds row locks on the entries' items, taken in position order.

**One invalidation event** (D7): the core paths each append their own `invalidate` events.
The apply runs them through `ApplyTx` (`ferrite-core/src/release/tx.rs`), a `Tx` wrapper
that delegates every repository to the real transaction except `events()`, which collects
`invalidate` keys in memory (any other kind is passed through). After the last entry the
apply appends one `invalidate` event with the deduplicated keys, or `ALL_KEY` when there
are more than 2000 (`MAX_RELEASE_KEYS`). Every node's invalidation loop then drops every
affected page in one step after the commit, so no node serves a mix of old and new pages
from its cache. `begin_nested` returns a wrapped nested `Tx` sharing the collector (the
slug retry in `save_with_free_slug` uses savepoints).

### 4.5 Check

`ReleaseService::check(actor, id) -> ReleaseCheck` applies the release in a **sandbox**
(§5.2) with one savepoint per entry and reports, per entry: `ok`, `conflict`
(`current_version`), `needs_approval`, `forbidden` (capability), `invalid` (the field
error, e.g. a slug taken since), then rolls back. It ignores conflicts while applying (the
entry is applied over the current version, so later entries are checked against the
state they would really meet) but reports them. Scheduling requires a clean check
(`CoreError::ReleaseNotReady(report)`, 409 `release-not-ready`); a failed scheduled apply
stores the check's report as `failure` (§4.6).

### 4.6 Scheduling and failures

- `POST …/schedule {at}` (future, ≤ 1 year): a clean check as the actor, then status
  `scheduled`, `scheduled_at`, `scheduled_by`, the locks, and in the same transaction the
  job `release.apply` (payload `{"v":1,"id","at"}`, unique key `release:<id>:<at micros>`,
  `run_at = at`), the pattern of `jobs::publish_scheduled_job`. A periodic
  `release.apply_due` (every 30 s, in `core_periodics`) applies every scheduled release
  whose time has passed and whose job is gone, like `content.publish_due` (D8).
- `POST …/unschedule`: back to `open`, locks removed; the queued job later finds the
  release no longer scheduled for its time and does nothing.
- **Who applies a scheduled release** (D8): the user who scheduled it, re-read from
  `users` at apply time (active, current role; scopes `Scopes::ALL`, since the credential
  that scheduled it is gone by then). A deleted or disabled scheduler, or one who lost a
  right, fails the release.
- **Failure** (D9): a permanent error (conflict, approval, permission, validation) rolls the
  apply back; a second transaction sets status `failed`, `failure` = `{ at, reason,
  entries: [the check report] }`, removes the locks, appends audit `release.failed`,
  emits `release.failed`, and enqueues `editorial.notify` to the creator and the scheduler.
  A store error is `JobError::Retry` (the job's attempts, then `release.apply_due` keeps
  trying while the release stays `scheduled`).
- Cluster: the job queue gives the job to one node; a second runner (job and periodic
  racing) loses the `releases.version` check and does nothing.

### 4.7 Permissions

No new capability or token scope (D29, D30). Content scopes apply: reads need
`content:read`, writes `content:write`.

- **See a release**: its creator, or anyone with `edit_others` (editors, admins).
- **Create** a release: `edit_own` (every role, contributors included).
- **Stage an entry**: see the release and edit the item (`can_edit`); a transition also
  needs the item's publish right (`publish_own` for one's own, `publish_others` otherwise),
  checked when staged (to fail early) and again when applied.
- **Apply, schedule, unschedule, rebase, delete**: see the release; apply and schedule then
  pass each entry's own checks as the actor.

So a contributor can stage changes to their own drafts but cannot apply a release that
publishes; an author can release their own posts; an editor anything.

## 5. Whole-site preview

### 5.1 Options considered

1. **A read overlay in Rust** (a `Repos` wrapper merging staged rows into query results):
   `ContentRepo::get`/`get_by_slug`/`get_many` are easy, but listings page by cursor in
   SQL (`ContentRepo::list`, `count`, `file_bounds` for sitemaps, `skip_rows` for
   `/page/N/`, the search index), so the overlay would have to re-implement every
   `ContentQuery` predicate (status, `published_at <= now`, kinds, term, author, parent,
   date range, media kind, `readable_by`) and the cursor order in Rust, plus terms, meta and
   redirects. Two implementations of "what is public" is the exact drift `site.rs` is built
   to avoid ("published-only by construction").
2. **Copy data into a shadow schema**: heavy, and impossible on SQLite without a second
   database.
3. **Sandbox transaction** (chosen, D10): begin a real transaction, apply the release in it
   through the same code as §4.4, render the requested page with a `SiteReader` reading
   through that transaction, then roll back. The rendered site is exactly what the commit
   would produce — listings, counts, cursors, sitemaps, the page tree, menus whose item
   links now resolve, the front page — with no second implementation.

### 5.2 The sandbox

`ApplyTx` has a second mode, `Mode::Sandbox`, in which the side-channel repositories are
no-ops (D13): `events()` (no outbox rows, so no Postgres sequence gaps; a rolled-back
event row would otherwise leave a permanent hole that, after `GAP_TIMEOUT`, flushes every
node's page cache, `ferrite-cache/src/invalidation.rs` R24), `jobs()` (enqueue returns
`Ok(None)`), `audit()`, `idempotency()`, `revisions()` (insert and prune), `search()`
(index and remove), and any repository M6-6 adds for webhook deliveries. Reads go to the
real transaction, so the sandbox sees its own writes.

`SandboxStore` (`ferrite-core/src/release/sandbox.rs`) implements `ferrite_store::Store`
over one sandbox: `read()` returns the sandbox `Repos`; `begin()` and the maintenance
methods return `StoreError::Unsupported("preview is read-only")`; `backend()` and
`pool_stats()` delegate. It is dropped at the end of the request; dropping rolls the
transaction back (and `finish()` rolls back explicitly). `SiteReader::with_store(store)`
returns a reader sharing the permalinks, media URLs, clock and image width but reading
through the sandbox.

Entries are applied in savepoints (`begin_nested`) so one failing entry (a slug taken since
it was staged) is skipped and listed in the preview bar instead of breaking the preview;
conflicting entries are applied over the current version and marked. The release is
applied as the **preview's principal**: the link's creator (§7), re-checked; entries that
principal may not apply are skipped and listed, so a preview never shows what the person
who shared it could not see.

### 5.3 Navigating the preview

A whole-site preview is browsed with ordinary paths (`/`, `/about/`, `/sitemap.xml`), so it
needs a mode, not a URL prefix (D11):

1. The admin's "Preview site" creates a preview link for the release (§7; 1 h for one's
   own preview, or what the user chose when sharing) and opens `/_preview/link/<token>`.
2. That handler checks the token and sets the cookie `__Host-ferrite_preview` (https) or
   `ferrite_preview` (http) = the token, `HttpOnly`, `SameSite=Lax`, `Path=/`, `Max-Age`
   = the link's remaining life; then `303` to `/` (or to `?to=<local path>`, which must
   start with `/` and not `//`).
3. Every public `GET`/`HEAD` page request carrying the cookie (`handler::page`, before the
   redirect lookup) resolves the token; valid: the request is answered through the sandbox
   — redirects resolved with `redirects::lookup(sandbox repos, path)`, the route rendered
   by the normal pipeline with a `ReadView` over the sandbox (§9.4) — **bypassing the page
   cache** (neither read nor written), `Cache-Control: private, no-store`, `X-Robots-Tag:
   noindex`, `Referrer-Policy: no-referrer`, not counted as a page view or `request.served`
   event, no redirect hit counted. Invalid or expired: the cookie is cleared and the
   request served normally.
4. `/_preview/exit` clears the cookie and `303`s to `/`.

`/_media`, `/_theme`, the API, `/admin/` and plugin routes ignore the cookie.

### 5.4 Costs and limits

- SQLite has one writer connection (`ferrite-store-sqlite/src/connect.rs`,
  `WRITER_ACQUIRE_TIMEOUT` 5 s). A preview request holds it from the sandbox's start to the
  end of the render, so other writes on the site wait for that long. Bounds (D34): one
  release-preview render at a time per node on SQLite, two on Postgres
  (`PREVIEW_SLOTS`); a request waits at most `render_wait` for a slot, then gets the 503
  busy page; a sandbox lives at most 15 s (`PREVIEW_DEADLINE`), then it is rolled back and
  the request gets the 503 page; each link renders at most 60 pages a minute (DB rate limit
  `preview:<link id>`, outside the sandbox).
- The cost per request is the apply (≈ 1–3 ms per entry without revisions and search) plus
  a normal render. A 200-entry release previews in well under a second on SQLite.
- Postgres: row locks on the entries' items for the request; a concurrent edit of one of
  them waits that long.

### 5.5 Plugins and themes in a preview

- Plugin filters run (the page should look like the live one), inside a **read-only plugin
  scope** (D12): `apply.rs`'s task-local `Scope` gains `read_only`, and
  `StoreHostServices::{meta_set, kv_set, kv_delete, notify_admins}` answer
  `Err("not available in a preview")` (`kv_delete` does nothing). Without it a plugin
  writing in a filter on SQLite would wait for the writer the sandbox holds. Plugin reads
  (`content.get-post`, `list-posts`, `meta.get`) go through the plugin host's own store and
  show the **live** site (a known limit, §5.6).
- Themes: `request.preview` is `true`; new `request.preview_release` holds the release
  title (else `none`). The three starter themes show "Preview of <title>" in their existing
  notice.
- **Preview bar** (D28): core injects a fixed bar after the opening `<body…>` tag (or at
  the start when there is none) of every cookie or link preview: "Preview of “<title>” · N
  changes · K not shown · Exit preview" (escaped; `role="region"`, `aria-label="Preview"`,
  inline styles, a link to `/_preview/exit`). The editor's own iframe (`/_preview/<id>`)
  gets no bar. A shared preview can therefore never pass for the live site, whatever the
  theme.

### 5.6 Fidelity limits (accepted)

Search results use the live search index (D13); plugin content reads see the live site;
the clock is now, not the release's scheduled time (D33); items a scheduled
`content.publish_scheduled` job would publish before the release are not shown; staged
menus do not exist (D2).

## 6. Approval workflows

### 6.1 Configuration

One workflow per content type (`workflows.type`), edited by admins (`manage_settings`,
scope `settings:write`):

```json
{ "steps": [
  { "key": "copy", "label": "Copy edit", "approvers": { "roles": ["editor"], "users": [] }, "required": 1 },
  { "key": "legal", "label": "Legal", "approvers": { "roles": [], "users": ["<uuid>"] }, "required": 1 }
] }
```

- 1–5 steps; `key` a slug (unique in the workflow), `label` 1–80 characters; `required`
  1–5; `approvers.roles` ⊆ {`editor`, `admin`}; `approvers.users` ≤ 20 ids of active
  editors or admins; at least one approver source. Approvers must hold `edit_others`, so
  they can read every item they review without a change to `can_read` (D17).
- Changing a workflow does not change open reviews: a review keeps a snapshot of the steps
  it started with (D23). Deleting a workflow (or the type) withdraws its open reviews.
- The admin offers one preset: "One editor approves" (one step, editors and admins, 1).

### 6.2 Reviews

A review has a **target** and a **fingerprint** (D18):

- item review (`target = "item:<id>"`): the item's newest revision id
  (`RevisionRepo::list(id, 1)`) when submitted;
- entry review (`target = "entry:<release>:<item>"`): the entry's `entry_version`.

States: `in_review` (at step `step`), `approved`, `changes_requested`, `withdrawn`,
`stale`. At most one open (`in_review` or `approved`) review per target (a partial unique
index). Any change to the target closes the open review as `stale`: a content save writes a
revision, and `after_save` (`ferrite-core/src/content.rs`, used by every create and update,
imports included) calls `ReviewRepo::mark_stale_item`; a staged-change edit or a rebase
bumps `entry_version` and marks the entry's review stale.

- **Submit** (`ContentService::submit`, extended): in a workflow type, `draft → pending`
  as today and, in the same transaction, a review at step 0 with optional assignees and a
  note (≤ 2000). From `pending` with no open review (after a stale or withdrawn one)
  `submit` creates a new review without a status change ("Resubmit"). An entry is
  submitted with `POST /releases/{id}/items/{item}/submit`. In a type without a workflow,
  submit is unchanged and creates no review (D25).
- **Decide** (`ReviewService::decide`): an eligible approver of the current step — in the
  step's roles or users, holding `edit_others`, **not the submitter and not the item's
  author** (D17) — records `approve` or `request_changes` with an optional comment
  (≤ 2000), once per step. When a step reaches `required` distinct approvals the review
  moves to the next step, or to `approved` after the last. `request_changes` closes the
  review as `changes_requested` and moves a `pending` item back to `draft` (D22).
- **Assign** (`PUT /reviews/{id}/assignees {step, users}`): the submitter or an editor
  assigns up to 10 eligible approvers to a step; only notifications use assignments
  (anyone eligible may decide).
- **Withdraw**: the submitter or `edit_others`; the review closes `withdrawn`, a `pending`
  item goes back to `draft`.

### 6.3 What "publish" does when a workflow exists (the gate)

In a type with a workflow (D19):

| Path | Rule |
|---|---|
| `publish`, `schedule` (REST, MCP, admin) | The item's open review is `approved` and its fingerprint is the item's newest revision; else `CoreError::ApprovalRequired` (409 `approval-required`, naming the current step). |
| `publish_due`, `publish_scheduled` (system) | Not gated: the item was approved when scheduled and cannot change while scheduled (D20). |
| Release apply, entry with `changes` | The entry's review `approved` at the current `entry_version`. |
| Release apply, entry with `publish` only | The item review rule above. |
| `unpublish`, `trash`, `restore`, `delete` | Not gated (removing is not publishing). |
| `update`, `restore_revision` of a public item | Refused, see §6.4. |

`allowed_actions` (`content/allowed.rs`) gets the workflow state as an input
(`WorkflowState { workflow: bool, review: Option<ReviewSummary> }`, read with the item by
`ContentService::extras_for`), so `allowed.publish` stays exactly what the server accepts,
plus new flags `needs_approval`, `stage` (§6.4) and `override` (admins).

### 6.4 Edits of public items

In a type with a workflow, a `published`, `scheduled` or `private` item cannot be changed
in place by anyone (D20): `ContentService::update` and `restore_revision` refuse with
`CoreError::StagingRequired` (409 `staging-required`). The change is staged in a release
instead (§4.2) and approved as an entry review. The editor knows from `allowed.edit =
false, allowed.stage = true` and offers "Save as change" instead of "Update" (§11.2).
Without a workflow, editing a published item stays live as today, and staging is optional.

### 6.5 Admin override

An admin (role `admin`, not a capability) may publish, schedule or apply without the
approvals by sending `override_reason` (1–500 characters, D21). It writes an audit row
`workflow.override` with the target, the missing approvals and the reason, and emits the
M6-6 event `review.overridden`. Overrides are not offered to MCP tools (§12). Editing a
public item still goes through a release; the admin applies it with an override.

### 6.6 Notifications

Mail goes through a durable job (D24), `editorial.notify` (payload `{"v":1,"event",
"review"|"release",…}`), enqueued in the transaction of the change. The handler
(`crates/ferrite/src/jobs/editorial.rs`) computes recipients and text when it runs and sends
through the node's `MailQueue` (`OutgoingMail`, subjects with `subject_title`, every title
through `header_safe`); without `[mail]` it completes with a debug log.

| Event | To |
|---|---|
| `review.submitted`, step advanced | The step's assignees, else every eligible approver (bcc, ≤ 50, `MAX_RECIPIENTS`) |
| `review.approved` | The submitter |
| `review.changes_requested` | The submitter, with the comment |
| `release.applied` (by someone else) | The creator |
| `release.failed` | The creator and the scheduler, with the report |

Each mail links to the admin screen (`<site>/admin/reviews?review=<id>` or
`/admin/releases/<id>`), never to content. No per-user opt-out yet (Later).

## 7. Preview links (signed, expiring)

- A link is a row of `preview_links`: `token_hash` = sha-256 of the token, `target`
  (`item` or `release`) and its id, `created_by`, `expires_at` (1–168 h, default 24 h),
  `last_used_at` (D26). Token: `ferp_` + 43 base64url characters (32 random bytes,
  `auth::key::random_bytes`). The token is shown once.
- Create: `POST /api/v1/content/{type}/{id}/preview-links` or
  `POST /api/v1/releases/{id}/preview-links` (`{expires_in_hours}`); the creator must be
  able to preview the target (`can_read` for an item, §4.7 for a release). 30 links per
  user per hour (`preview_link:user:<id>`). Audit `preview_link.create`. List per target;
  revoke with `DELETE /api/v1/preview-links/{id}` (the creator or `edit_others`). A
  release's links go when it is applied or deleted; an item's when it is deleted
  (`PreviewLinkRepo::delete_for_target`, in that transaction).
- Use: `/_preview/link/<token>` (route grammar in `ferrite-http/src/route.rs`: exactly
  `ferp_` + 43 characters, anything else under `/_preview/link/` is the 404 page). An item
  link renders the item's **stored** state as `/_preview/<id>` does; a release link starts
  the cookie mode (§5.3). Each use re-checks (D27): unexpired, the creator active, not
  `must_reset`, still allowed to preview the target; the render runs as the creator's actor
  (role from `users`, scopes `content:read` only). `last_used_at` is touched at most once a
  minute.
- Pruned daily by the periodic `preview_links.prune` (expired rows).
- The MCP tool `content_preview_link` returns such a link (D26), so a model can hand a user
  a preview that opens without signing in; the M5 follow-up is closed.

## 8. Storage: migration `0017_releases_workflows`

Both backends, `-- ferrite:phase expand`, `-- ferrite:lossy-down` (the down drops every
release, workflow, review and link), same table and constraint names. SQLite types follow
02 §5 (`BLOB` ids, `INTEGER` microseconds, `TEXT` JSON with `json_valid`); Postgres `uuid`,
`timestamptz`, `jsonb`, text keys `COLLATE "C"`. Status columns have no `CHECK` (P2: rows
decode only with a known value). Every foreign key that deletes has an index.

```sql
CREATE TABLE releases (
  id, title NOT NULL, description NOT NULL DEFAULT '', status NOT NULL,
  scheduled_at NULL, scheduled_by NULL -> users ON DELETE SET NULL,
  override_reason NULL, failure JSON NULL,
  created_by NULL -> users ON DELETE SET NULL, created_at NOT NULL, updated_at NOT NULL,
  applied_at NULL, applied_by NULL -> users ON DELETE SET NULL,
  version NOT NULL DEFAULT 1 );
-- releases_status_scheduled_idx (status, scheduled_at); releases_created_by_idx,
-- releases_scheduled_by_idx, releases_applied_by_idx

CREATE TABLE release_items (
  release_id -> releases ON DELETE CASCADE, item_id -> content_items ON DELETE CASCADE,
  position NOT NULL, transition NOT NULL, changes JSON NULL,
  base_version NOT NULL, entry_version NOT NULL DEFAULT 1,
  added_by NULL -> users ON DELETE SET NULL, updated_at NOT NULL,
  PRIMARY KEY (release_id, item_id) );
-- release_items_item_idx (item_id), release_items_added_by_idx

CREATE TABLE release_locks (
  item_id PRIMARY KEY -> content_items ON DELETE CASCADE,
  release_id NOT NULL -> releases ON DELETE CASCADE );
-- release_locks_release_idx

CREATE TABLE workflows (
  type PRIMARY KEY -> content_types (name) ON DELETE CASCADE, steps JSON NOT NULL,
  updated_by NULL -> users ON DELETE SET NULL, updated_at NOT NULL, version NOT NULL DEFAULT 1 );

CREATE TABLE reviews (
  id PRIMARY KEY, target NOT NULL, item_id NOT NULL -> content_items ON DELETE CASCADE,
  release_id NULL, revision_id NULL, entry_version NULL,
  status NOT NULL, step NOT NULL DEFAULT 0, steps JSON NOT NULL, note NULL,
  submitted_by NULL -> users ON DELETE SET NULL, submitted_at NOT NULL, decided_at NULL,
  version NOT NULL DEFAULT 1,
  FOREIGN KEY (release_id, item_id) -> release_items ON DELETE CASCADE );
-- reviews_open_target_key UNIQUE (target) WHERE status IN ('in_review', 'approved')
-- reviews_item_idx (item_id), reviews_status_idx (status, submitted_at),
-- reviews_release_item_idx (release_id, item_id), reviews_submitted_by_idx

CREATE TABLE review_decisions (
  id PRIMARY KEY, review_id NOT NULL -> reviews ON DELETE CASCADE, step NOT NULL,
  user_id NULL -> users ON DELETE SET NULL, decision NOT NULL, comment NULL, at NOT NULL,
  UNIQUE (review_id, step, user_id) );
-- review_decisions_user_idx

CREATE TABLE review_assignees (
  review_id -> reviews ON DELETE CASCADE, step NOT NULL, user_id -> users ON DELETE CASCADE,
  PRIMARY KEY (review_id, step, user_id) );
-- review_assignees_user_idx

CREATE TABLE preview_links (
  id PRIMARY KEY, token_hash NOT NULL UNIQUE, target_kind NOT NULL, target_id NOT NULL,
  created_by NOT NULL -> users ON DELETE CASCADE, created_at NOT NULL, expires_at NOT NULL,
  last_used_at NULL );
-- preview_links_target_idx (target_kind, target_id), preview_links_expires_idx,
-- preview_links_created_by_idx
```

The composite foreign key of `reviews` is `MATCH SIMPLE` on both backends (the default), so
an item review (`release_id` NULL) is not checked against `release_items`.

**Catalogue** (`ferrite-store/src/catalogue.rs`, D37): `releases`, `release_items`,
`release_locks`, `workflows`, `reviews`, `review_decisions`, `review_assignees` are
exported (`Export::Rows`, in FK order after `content_items` and `users`); `preview_links` is
skipped (like `sessions`: tokens are bound to a site and short-lived). After `import-site`
a scheduled release has no job; `release.apply_due` picks it up.

**Repositories** (`ferrite-store/src/repos.rs`, new accessors on `Repos`, implemented on
both backends and on every wrapper: `CountingStore`, `FailingOptionsStore`, `ApplyTx`):

- `ReleaseRepo`: `insert`, `get`, `update` (version-checked, `Conflict`), `delete`,
  `list(ReleaseQuery { status, created_by, cursor, limit })` (cursor tag
  `RELEASE_CURSOR_TAG`), `entries(release)`, `entry(release, item)`, `put_entry` (insert or
  replace, bumping `entry_version` on replace), `delete_entry`, `releases_of(item)`,
  `lock(item, release)` (`UniqueViolation { constraint: "release_locks_pkey" }` when held),
  `unlock_all(release)`, `lock_of(item) -> Option<ReleaseId>`, `due(now, limit)`.
- `WorkflowRepo`: `get(type)`, `put` (version-checked), `delete`, `list`.
- `ReviewRepo`: `insert` (`UniqueViolation { constraint: "reviews_open_target_key" }` when
  one is open), `get`, `update` (version-checked), `open_for(target)`,
  `list(ReviewQuery { status, assignee, submitted_by, kind, cursor, limit })`,
  `mark_stale_item(item)`, `mark_stale_entry(release, item)`, `withdraw_for_type(type)`,
  `decisions(review)`, `add_decision`, `assignees(review)`, `set_assignees(review, step,
  users)`, `counts_for(user)` (assigned open reviews, for the dashboard).
- `PreviewLinkRepo`: `insert`, `get_by_hash`, `touch(id, at)`, `list_for(kind, id)`,
  `delete(id)`, `delete_for_target(kind, id)`, `prune(now, limit)`.

Domain types (`ferrite-domain/src/editorial.rs`, ids in `ids.rs`): `ReleaseId`, `ReviewId`,
`PreviewLinkId`, `ReleaseStatus`, `Release`, `ReleaseEntry`, `EntryTransition`,
`StagedChange`, `Workflow`, `WorkflowStep`, `Approvers`, `Review`, `ReviewStatus`,
`ReviewTarget`, `ReviewDecision`, `Decision`, `PreviewLink`, `PreviewTarget`.

## 9. Core

### 9.1 Services

- `ferrite-core/src/release/`: `mod.rs` (`ReleaseService::{create, get_for, list, update,
  delete, stage, unstage, rebase, check, schedule, unschedule, apply, apply_scheduled,
  apply_due, sandbox}`), `tx.rs` (`ApplyTx`, `Mode::{Collect, Sandbox}`), `sandbox.rs`
  (`SandboxStore`, `SandboxReport { applied, skipped: Vec<(ContentId, reason)> }`),
  `staged.rs` (`StagedChange` ⇄ `ContentDraft`, staging checks).
- `ferrite-core/src/workflow/`: `mod.rs` (`WorkflowService::{get, list, put, delete}`,
  step validation), `review.rs` (`ReviewService::{submit_entry, decide, assign, withdraw,
  list, get}`; `ContentService::submit` calls `review::start_in`), `gate.rs`
  (`require_approval(repos, actor, item, target, override) -> Result<Option<NewAudit>>`,
  `require_live_edit(repos, item)`, `WorkflowState`).
- `ferrite-core/src/preview_link.rs`: `PreviewLinkService::{create, list, revoke,
  resolve}` (`resolve` → `PreviewPrincipal { link, actor, target }`).
- `ContentService`: the lock check (`release::check_unlocked(repos, id)`) and the gate in
  the public methods (`update`, `transition` for publish/schedule/submit,
  `restore_revision`, `delete`); new `pub(crate) unpublish_in`; `after_save` marks item
  reviews stale.
- `SiteReader`: `with_store(Arc<dyn Store>) -> SiteReader`, `preview_working(actor, id)`,
  `preview_staged(actor, id, release)`, `preview_as(actor, id)` for item links.

### 9.2 Errors

New `CoreError` variants, mapped in `ferrite-http/src/api/problem.rs` and by the MCP
results: `Locked { release: ReleaseId }` → 409 `release-locked`;
`ApprovalRequired { step: Option<String> }` → 409 `approval-required`;
`StagingRequired` → 409 `staging-required`; `ReleaseConflict(Vec<EntryProblem>)` and
`ReleaseNotReady(ReleaseCheck)` → 409 `release-conflict` / `release-not-ready` with an
`entries` extension member. Job handlers map them to `JobError::Permanent`.

### 9.3 Jobs and periodics

| Kind | Payload | Handler |
|---|---|---|
| `release.apply` | `{"v":1,"id","at"}`, unique `release:<id>:<at µs>` | `ReleaseService::apply_scheduled(id, at)` |
| `release.apply_due` | periodic, 30 s | `ReleaseService::apply_due(db_now)` |
| `editorial.notify` | `{"v":1,"event",…}` | builds and sends the mail |
| `preview_links.prune` | periodic, daily | `PreviewLinkRepo::prune` in batches of `MAX_PRUNE_BATCH` |

Kinds and payloads live in `ferrite-core/src/jobs.rs`; handlers in
`crates/ferrite/src/jobs/editorial.rs`, registered in `crates/ferrite/src/jobs.rs`
`handlers` and `core_periodics`.

### 9.4 The read view (ferrite-http)

The render path reads `state.reader` and `state.store` directly (`app/pipeline.rs` `load`,
`finish`, `page_head`, the `BlockingBridge`; `app/feeds.rs`; `AppState::site_info`). A
`ReadView { reader: Arc<SiteReader>, store: Arc<dyn Store> }` (`app/view.rs`) replaces
those reads; `AppState::view()` is the live one, a preview builds one over a
`SandboxStore`. This refactor changes no behaviour and lands first (plan Task 9).

## 10. REST API (`/api/v1`)

All with the usual session + CSRF or bearer rules, `utoipa` DTOs, `If-Match` on writes of
versioned rows, `Idempotency-Key` on `POST` creates and apply.

| Method and path | What |
|---|---|
| `GET /releases` | `?status=&mine=&cursor=&limit=` → list (title, status, counts, schedule, creator) |
| `POST /releases` | `{title, description?, items?: [EntryInput]}` → 201 |
| `GET /releases/{id}` | the release with entries (item ref, type, locale, status, transition, has_changes, review summary, conflict flag), `ETag` |
| `PATCH /releases/{id}` | title, description |
| `DELETE /releases/{id}` | open or failed only |
| `POST /releases/{id}/items` | `EntryInput {item_id, transition, changes?: StagedChangeDto}` |
| `PUT /releases/{id}/items/{item}` | replace the entry |
| `DELETE /releases/{id}/items/{item}` | remove the entry |
| `POST /releases/{id}/items/{item}/rebase` | base on the current version |
| `POST /releases/{id}/items/{item}/submit` | entry review `{reviewers?, note?}` |
| `GET /releases/{id}/check` | `ReleaseCheck` |
| `POST /releases/{id}/schedule` | `{at, override_reason?}` |
| `POST /releases/{id}/unschedule` | |
| `POST /releases/{id}/apply` | `{override_reason?}` → 200 applied, 409 with entries |
| `GET/POST /releases/{id}/preview-links` | list / create |
| `GET/POST /content/{type}/{id}/preview-links` | list / create |
| `DELETE /preview-links/{id}` | revoke |
| `GET /workflows`, `GET/PUT/DELETE /workflows/{type}` | workflow config (admin) |
| `GET /reviews` | `?status=&assigned=me&submitted=me&type=` queue |
| `GET /reviews/{id}` | review with steps, decisions, assignees, target summary |
| `POST /reviews/{id}/decisions` | `{decision, comment?}` |
| `PUT /reviews/{id}/assignees` | `{step, users}` |
| `POST /reviews/{id}/withdraw` | |

Changed: `POST /content/{type}/{id}/submit` takes an optional JSON body `{reviewers?,
note?}`; `…/publish` and `…/schedule` take `override_reason?`; the content DTO's
`allowed` gains `needs_approval`, `stage`, `override`; the content DTO gains `review:
ReviewSummary | null` and `releases: [{id, title, status}]` (releases holding the item,
≤ 10); `GET /content/{type}/{id}` is unchanged otherwise. 07 §1's status table and the
checked-in `openapi.json` are updated.

## 11. Admin

All screens follow 11 (tokens, 44 px touch targets, axe clean, i18n in
`admin/src/i18n/en/*.json`, `stale` rows at full contrast).

### 11.1 Editor: live preview

§3.3. Files: `admin/src/editor/preview/{preview-pane.tsx, use-preview-frame.ts}`,
`editor.tsx` (layout), `autosave.ts` (the debounce option), `panel-state.ts`
(`SPLIT_QUERY`).

### 11.2 Editor: releases and reviews

- Publish bar secondary actions: **Add to release…** (dialog: pick an open release or
  "New release", the transition; the working draft becomes the staged change when "Include
  my changes" is on) and **Share preview…** (expiry 1 h, 24 h, 7 days; the link is shown
  once with Copy; the list of the item's links with Revoke).
- When `allowed.stage` (a public item of a workflow type): **Save as change** replaces
  Update and opens the same dialog.
- Banner when the item has staged changes: "Staged in “Spring launch” — Open the change".
  **Change mode** (`/content/$type/$id?release=<rid>`): the editor edits the entry's staged
  change, saves with `PUT …/items/{item}` (`mod+s`), keeps only the local backup (no
  autosave slot, D36), previews with `?release=`.
- Side panel **Review** section (workflow types): the steps with their state, decisions with
  comments, assignees (an assign popover), Submit/Resubmit with reviewers and a note,
  Approve / Request changes with a comment for eligible approvers, Withdraw.
- Publish dialog: when `needs_approval` and `override`, an "Override approval" disclosure
  with a required reason.

### 11.3 Releases (`/admin/releases`, `g r`)

- List: `DataTable` (title, status badge, entries, scheduled time, creator, updated),
  filters Open / Scheduled / Applied / Failed, "New release".
- Detail (`/admin/releases/$id`): title and description edited in place; an entries table
  (item, type, locale, action chips "Changes" / "Publish" / "Unpublish", review state,
  conflict badge with "Rebase" and a diff drawer); **Check** (the report inline);
  **Preview site** (opens the preview in a new tab); **Share preview…**; **Schedule…**
  (`schedule-picker.tsx`); **Apply now** (a confirm dialog listing what goes live, with the
  override disclosure for admins); a failed release shows its report on top.
- Palette: "New release", "Open releases".

### 11.4 Reviews (`/admin/reviews`, `g v`) and workflows

- Queue: tabs "Assigned to me", "All open" (editors), "Submitted by me"; rows open the
  editor (item reviews) or the release (entry reviews) with the Review section focused.
- Dashboard: a "Waiting for your review" card with the count and the oldest three.
- Content type screen (`structure.types.$name`): a **Workflow** section (admins): steps as a
  sortable list (`@dnd-kit`, already a dependency) with label, approvers (role checkboxes,
  a user picker limited to active editors and admins), required count; the preset; remove
  workflow (confirm: open reviews are withdrawn).

## 12. MCP

New tools in `ferrite-http/src/mcp/tools/releases.rs` and `reviews.rs` (D31), with the
conventions of 12 §5.1 (Markdown bodies, `version` for writes, audit `mcp.<tool>`):

| Tool | What |
|---|---|
| `releases_list` | releases the caller may see |
| `release_get` | a release with its entries (staged bodies as Markdown) |
| `release_create` | `{title, description?}` |
| `release_stage` | add or replace an entry: `item`, `transition`, optional changes (`body_markdown` or `body_blocks`, title, fields, …) |
| `release_unstage` | remove an entry |
| `release_check` | the check report |
| `release_schedule` | `{at}` or `null` to unschedule (`destructiveHint` false) |
| `release_apply` | apply now (`destructiveHint`: it publishes several items) |
| `release_preview_link` | a preview link for the release |
| `reviews_list` | the caller's review queue (read only) |

Changed: `content_submit_for_review` takes `reviewers` and `note`; `content_preview_link`
returns a preview link (`expires_in_hours` 1–168, default 24) that opens without a session;
`content_publish` / `content_schedule` / `content_update` errors for `approval-required`
and `staging-required` tell the model what to do ("submit it for review with
`content_submit_for_review`; a person approves it in the admin" / "stage the change with
`release_stage`"). Review decisions, overrides and workflow configuration are not tools
(12 §5.4 grows: "approves or rejects reviews, overrides approvals, configures workflows").
The tool count goes from 56 to 66; `site/content/05-reference/02-mcp-tools.md` lists them.

## 13. Plugin, theme and webhook surface

- **Plugins**: no WIT change, no new hook (D32). A release's items fire the existing
  `post.saved` and `post.published` actions from the core paths, after the commit (they are
  jobs enqueued in the apply transaction). Previews run filters in the read-only scope
  (§5.5).
- **Themes**: `request.preview_release` (string or `none`); `request.preview` as before.
  Starter themes updated.
- **Webhooks (M6-6)**: new events, emitted through M6-6's emitter in the transaction of the
  change: `release.scheduled`, `release.unscheduled`, `release.applied` (`{release: {id,
  title}, items: [{id, type, locale, action}], applied_by}`), `release.failed` (`{release,
  reason, entries}`), `review.submitted`, `review.approved`, `review.changes_requested`,
  `review.overridden` (`{review: {id, target, step}, item: {id, type, locale}, by}`). The
  per-item content events M6-6 emits from the core paths fire for each applied entry. In
  the sandbox, M6-6's delivery repository is a no-op.

## 14. Security

- **Approval integrity**: approvals bind to content (revision id or entry version), never
  to status; any change makes them stale; approvers cannot approve their own or their own
  item's content; overrides are admin-only, reasoned and audited.
- **No live edits behind the gate**: §6.4 closes the "edit after publish" bypass.
- **Preview links**: 256-bit tokens, hashed at rest, expiring (≤ 7 days), revocable,
  audited, rate-limited (create and use), re-checked against the creator on every use,
  rendering as the creator with `content:read` only. Pages under preview are `no-store`,
  `noindex`, `no-referrer`, never cached, never counted. The cookie is `HttpOnly`,
  `SameSite=Lax`, `__Host-` on https, holds only the token. A shared release preview always
  shows the core preview bar (D28), so it cannot be passed off as the live site.
- **Open redirect**: `?to=` takes a local path only.
- **Sandbox**: it can only write inside a transaction that is always rolled back; side
  channels (outbox, jobs, audit, webhooks, mail) are no-ops; plugin writes are refused;
  `SandboxStore::begin` is refused, so nothing in the render path can open a write.
- **DoS**: per-node preview slots, the 15 s deadline, per-link render rate; release size
  200; workflow sizes bounded.
- **Untrusted text**: release titles, review comments and notes are user text: escaped in
  the admin, in the preview bar and in mail (`header_safe` for subjects; control and bidi
  characters removed from subjects as 12 §2 does for titles).
- **Scopes**: an API token with `content:write` can stage, apply and decide; one with only
  `content:read` can read releases and reviews it may see. OAuth grants (`Scopes::OAUTH`)
  are unchanged.

## 15. Cluster and profiles

- Solo (SQLite): one node; jobs and periodics in-process; the preview holds the single
  writer (§5.4).
- Cluster (Postgres, N nodes): releases, locks, reviews and links are rows, so every node
  sees the same state; `release.apply` runs on one node (job claim), the periodic on one
  node per period; the single `invalidate` event reaches every node's invalidation loop
  (LISTEN wake-ups as today); previews are per request on whichever node answers (the
  cookie holds a token any node resolves).
- No new config keys; constants: `MAX_RELEASE_ENTRIES = 200`, `MAX_RELEASE_KEYS = 2000`,
  `PREVIEW_SLOTS` (1 SQLite / 2 Postgres), `PREVIEW_DEADLINE = 15 s`,
  `PREVIEW_LINK_MAX = 168 h`.

## 16. Coordination with other M6 parts

- **M6-1** (locales): entries and reviews show the item's `locale`; workflows are per type,
  for all locales; "Translate" creates a draft that goes through review like any item; the
  preview cookie covers every locale prefix.
- **M6-4** (accessibility gate): its "block publishing on errors" check must sit in the
  core publish path that `publish_in` reaches (not in a handler), so release apply runs it
  per entry; the release check reports it as `invalid`.
- **M6-5** (change sets): MCP writes made by `release_stage` and `release_apply` go through
  the core paths it groups; release apply via MCP produces one change set entry per item.
  Revert of a whole release is not part of M6-3.
- **M6-6** (webhooks): the events of §13 and the sandbox no-op of its delivery repository.

## 17. Out of scope / Later

Staging menus, terms, settings and redirects in releases (each needs before-images like
M6-5's); auto-publish on final approval; per-locale workflows; reviewers outside
editors/admins (needs a `can_read` extension); notification preferences; review comments
on text ranges; visual diffs; plugin hooks `release.applied` / `review.*` (a manifest
change); scheduling a release relative to another; a release calendar view.

## 18. Decisions

- **D1** Staged changes are JSON snapshots in `release_items.changes`, not revision rows: revisions stay "what was saved", and the 50-revision prune can never drop staged work.
- **D2** An entry is one item with optional changes and one transition (`none`, `publish`, `unpublish`); content only — menus, terms, settings and redirects are not staged in M6-3.
- **D3** An item may be in several open releases: alternative plans are legitimate; conflicts surface when one is applied.
- **D4** Conflict = the item's version moved since `base_version`, for publish-only entries too; resolved by an explicit rebase, because what was reviewed must be what goes live.
- **D5** A scheduled release locks its items (`release_locks` PK) against changes through the public service methods; system paths are not locked and make the release fail with a conflict instead.
- **D6** Apply is one transaction on both backends through the existing core paths, capped at 200 entries, so it is all-or-nothing and bounded.
- **D7** One merged `invalidate` event per apply (`ApplyTx` collects keys; `all` above 2000), so every node drops the release's pages in one step.
- **D8** Scheduled apply = a uniquely keyed `release.apply` job plus a 30 s `release.apply_due` periodic, run as the scheduler re-read at apply time: the same reliability pattern as scheduled publishing, with current rights.
- **D9** Permanent failures mark the release `failed` with a check report, unlock it and notify; store errors retry: a 3 a.m. conflict must not leave items locked.
- **D10** The whole-site preview applies the release in a rolled-back sandbox transaction instead of a Rust read overlay: one definition of "public", exact listings, counts, cursors, sitemaps, tree and menus.
- **D11** Preview navigation uses a cookie holding a preview-link token, so ordinary site paths render under the preview without rewriting links.
- **D12** Plugins run in previews with writes refused: no side effects from previews and no SQLite writer self-wait.
- **D13** Outbox, jobs, audit, idempotency, revisions and search writes are no-ops in the sandbox: no sequence gaps (which flush caches cluster-wide), no stray side effects, a cheaper apply; preview search uses the live index.
- **D14** The live preview overlays the autosave slot in memory (title, excerpt, body, fields, SEO; slug, parent, terms and menu order as saved), so it never fails on a half-typed working copy and needs no transaction.
- **D15** With the preview open, autosave runs on a 2 s idle debounce: "live" needs seconds, not the 10 s interval.
- **D16** The preview iframe is same-origin and unsandboxed: public pages already share `/admin/`'s origin (`X-Frame-Options: SAMEORIGIN`, admin CSP `frame-src 'self'`), so framing adds no trust; it keeps the session cookie and scroll restore simple.
- **D17** Workflow approvers are editors or admins (roles and/or named users), `required` distinct approvers per step, never the submitter or the item's author: four eyes without widening `can_read`.
- **D18** Reviews bind to a fingerprint (newest revision id, or the entry's version); any change makes them stale.
- **D19** In a workflow type, publish and schedule need an approved, current review; release apply checks per entry; unpublish and trash are not gated.
- **D20** In a workflow type, public items (published, scheduled, private) cannot be edited in place; changes are staged and approved — otherwise "edit after approval" bypasses the workflow.
- **D21** Override is for the admin role only, with a required reason and an audit row; it is not offered to MCP.
- **D22** "Request changes" closes the review and sends a pending item back to draft; resubmission starts a new review.
- **D23** Reviews snapshot their steps; editing a workflow does not move open reviews; deleting one withdraws them.
- **D24** Notifications are a durable `editorial.notify` job enqueued in the change's transaction; recipients are computed when it runs; no `[mail]` means no mail.
- **D25** Reviews exist only in types with a workflow; plain submit-for-review is unchanged (the one-step preset covers "notify an editor").
- **D26** Preview links are hashed DB rows (`ferp_` tokens, 1–168 h, default 24 h), revocable and audited; `content_preview_link` returns one, closing the M5 follow-up.
- **D27** A link renders as its creator, re-checked on every use, with `content:read` only.
- **D28** Core injects a preview bar into every link or cookie preview, independent of the theme.
- **D29** Releases need no new capability: entries use the item's own edit and publish rights; a release is visible to its creator and to `edit_others`.
- **D30** No new token scope: releases and reviews use `content:*`, workflow configuration `settings:write`; the 22-bit scope set and stored token scopes stay as they are.
- **D31** MCP gets release tools (publishing is already a tool) but no review decisions, overrides or workflow config.
- **D32** No WIT change and no new plugin hook; per-item `post.saved`/`post.published` fire from the apply.
- **D33** The preview clock is now, not the release's scheduled time.
- **D34** Release previews: 1 slot per node on SQLite, 2 on Postgres, a 15 s deadline, 60 renders a minute per link.
- **D35** Statuses `open`, `scheduled`, `applied`, `failed`; applied releases are kept as history; open and failed ones can be deleted; no `cancelled`.
- **D36** The editor's change mode keeps no autosave slot (the slot is per item and belongs to live editing); the local backup protects it.
- **D37** Releases, entries, locks, workflows and reviews are exported; preview links are not.
- **D38** Final approval does not publish; publishing (or scheduling, or a release) stays a separate action.
- **D39** Entry transitions are validated against the item's status when staged; the base-version check guarantees the status is unchanged at apply.
- **D40** A release's preview links are deleted when it is applied or deleted, an item's when it is deleted: a link never outlives what it shows.

## 19. As built (M6-3)

Rulings P4 and P7 (`docs/superpowers/rulings/m6.md`) and the execution rulings R-M6-3-*
(`docs/superpowers/m6-3-exit.md`) change this design as follows:

- **D16 (P7):** the editor's preview frame is `sandbox="allow-scripts allow-popups
  allow-forms"` without `allow-same-origin` and loads through a 1 h preview link of the
  editor (`/_preview/link/<token>?working=1`, change mode `?release=<id>`); item links
  take those two queries. The frame is an opaque origin, so §3.2's scroll restore is not
  built. Links always show the preview bar (also in the editor's frame).
- **D20 (P7):** in a workflow type an approver (holding `edit_others` and named by a step
  by role or id) edits a published, scheduled or private item in place, with the audit row
  `review.bypassed`; everyone else gets `staging-required`. `allowed.edit` follows it.
- **D10 (P7):** the SQLite writer is held per preview request (one page).
- **P4:** a release calls M6-4's `quality::gate::check` for every entry it publishes; a
  blocked entry refuses the whole release (`release-conflict`, outcome `invalid`).
- The partial unique index is `reviews_open_target_uq` (not `_key`): SQLite's constraint
  map wants `_pkey`/`_uq` names.
- §9.4: no `ReadView` type; a preview renders through `AppState::preview_view(store,
  title)`, the same state over the sandbox store (reader, redirects), without counters,
  without cache writes and with read-only plugins.
- Entries of a whole-site preview are applied with the link creator's full rights (the
  sandbox rolls back); the page renders as the link's `content:read` principal.
- §10: no `Idempotency-Key` on apply (it is version-checked; a retry is `412`). Optional
  bodies on submit, publish, apply and link creation. The content DTO's `releases` is
  left out when empty.
- §12: `content_preview_link` creates a link, so it is a write tool (`content:write`).
  Releases created over MCP record their per-item changes in M6-5 change sets on apply;
  the tools that change only release or link rows are listed as excluded in the capture
  guard test.
