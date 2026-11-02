# 17 — AI change sets

Status: **design for M6-5** · Date: 2026-11-02 · Roadmap: "M6-5 — AI change sets" ·
Builds on 12 (MCP), assumes M6-1's locale model (wave B).

An assistant connected over MCP (design 12) changes the live site as soon as a tool
returns. The audit log says *that* it did (`mcp.<tool>` rows, 12 §5.1) but not *what the
object looked like before*, and only content keeps revisions. This part records every
write an assistant makes, with the object's state before and after, groups the writes
into **change sets** (one working session of one connection), shows them in the admin
with diffs, and lets a person revert a whole set or one change, refusing where someone
edited the object since.

## 1. Goals and non-goals

Goals:

1. **Every assistant write is reviewable.** Each successful write through `/mcp`, by an
   OAuth grant or a personal token, records a change with before- and after-images of
   what it touched, in the write's own transaction (no write without its record).
2. **Grouped as people think of them.** Writes of one connection close in time form one
   change set ("what Claude did this afternoon").
3. **Undo that is safe.** Revert a set (newest change first, one transaction) or one
   change; a change whose object was edited since is refused with a clear reason unless
   the person forces it.
4. **No new trust.** Reverting is a person's act in the admin (session only), and every
   reverse write runs as that person through the normal core write paths: their role and
   the usual validation, events, revisions and audit.
5. **Both profiles, N nodes.** All state is in the database; no node-local memory.

Non-goals: recording REST API or admin writes (people already have the audit log and
revisions; scripts are not assistants), undoing side effects outside the database
(mail sent, plugin `post.saved` jobs that called out, `http` requests), redoing a revert,
an MCP tool that reverts (§10, D14), recording writes that jobs make later on an
assistant's behalf (a `permalink.redirects` job, `media.variants`, a scheduled publish).

## 2. Concepts

- **Agent**: the connection that acts. An OAuth grant (`oauth_grants.id`; stable across
  the hourly access-token rotation, 12 §3.2) or a personal API token (`api_tokens.id`)
  used on `/mcp`. Core sees it as `Actor.agent: Option<Agent>`, set only by the MCP
  guard (§3.1).
- **Change set**: an ordered group of changes of one agent. It is **open** while the
  agent keeps writing with gaps under 30 minutes; the next write after a longer gap, or
  after 500 changes, or after a person reverted anything in it, opens a new one (§6).
- **Change**: one object touched by one write: `op` (`content.update`, `term.delete`, …),
  the object (`kind` + `id`), a label (title or name at write time), and two **images**
  (§4): the object before and after, each *absent* (did not exist / no longer exists),
  *present* (compressed canonical JSON + hash) or *skipped* (too large: hash only, the
  change cannot be reverted).
- **Revert**: restore objects to their before-images through the core write paths.

## 3. Capture

### 3.1 Marking the actor

`ferrite_core::Actor` (Copy, built only through `Actor::new` and `with_scopes`) gains
`pub agent: Option<Agent>`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent { OAuth(OAuthGrantId), Token(ApiTokenId) }
impl Actor {
    pub fn with_agent(self, agent: Agent) -> Self;   // `new` sets None, `with_scopes` keeps it
}
```

`ferrite-http/src/mcp/auth.rs::guard`, after `resolve_token`, sets
`who.actor = who.actor.with_agent(..)`: `Agent::OAuth(grant)` when `who.oauth` is set,
else `Agent::Token(id)` from `Via::Token(id)`. Nothing else sets it, so REST, admin,
CLI and job writes are never captured (D2). Personal tokens on `/mcp` are agents like
grants (D3): the roadmap names them, and the token is the only stable identity.

Every derived actor keeps the marker: `media::content_writer` and the comment
`Commenter::User(*actor)` copy the actor, `with_scopes` uses `..self`.

### 3.2 Where: core write paths, keyed by the actor

The 30 write tools of the 56 all call public core service methods that the REST API
shares (`ContentService::update`, `TaxonomyService::update_term`,
`OptionService::set_many`, `ExtensionAdmin::set_plugin_enabled`, …; checked in
`crates/ferrite-http/src/mcp/tools/*.rs`). Capture therefore lives in **core, inside
those methods' transactions** (D1), not in the MCP handlers: before-images are read in the
same `Tx` as the write, a rolled-back write leaves no record, and the REST paths cost
nothing (no agent, no reads).

The capture helper (`ferrite-core/src/changes/capture.rs`):

```rust
pub(crate) struct Capture { /* agent + user, op, before-images, created refs */ }
impl Capture {
    /// No agent: returns an inert Capture and reads nothing.
    pub(crate) async fn begin(repos: &dyn Repos, actor: &Actor, op: Op,
                              objects: &[ObjectRef]) -> Result<Self, CoreError>;
    /// An object that did not exist at `begin` (a create, the id known only now).
    pub(crate) fn created(&mut self, object: ObjectRef);
    /// Reads the after-images, opens or extends the set (§6), inserts one change per
    /// object. Called last in the write path, after every other statement (D9).
    pub(crate) async fn finish(self, repos: &dyn Repos) -> Result<(), CoreError>;
}
```

Each captured method gets two or three lines: `let cap = Capture::begin(tx, actor, op,
&[obj]).await?;` before its `_in` call and `cap.finish(tx).await?;` after it. Composite
paths (media update = content update + media row) capture once at the public method,
never inside the shared `_in` helpers, so one tool write is one change per object. A
failed `finish` fails the write (D4): unlike the post-commit `mcp.*` audit row, the
record is the promise of this feature.

### 3.3 Write tools, ops, objects and the reverse write

| Tool | Core method (captured) | `op` | Object(s) | Revert restores via |
|---|---|---|---|---|
| `content_create` | `ContentService::create` | `content.create` | content (created) | `trash_in` (D7) |
| `content_update` | `ContentService::update` | `content.update` | content | `update_in` with the before fields, note "Reverted an AI change" |
| `content_publish` | `ContentService::transition` (Publish) | `content.publish` | content | status back: `unpublish` (or `schedule`/`submit` per before-status) |
| `content_unpublish` | `transition` (Unpublish) | `content.unpublish` | content | `publish_in` |
| `content_schedule` | `transition` (Schedule) | `content.schedule` | content | status back (draft/pending) |
| `content_submit_for_review` | `transition` (Submit) | `content.submit` | content | status back (draft) |
| `content_trash` | `transition` (Trash) | `content.trash` | content | `restore_in`, then the before status |
| `content_restore` | `transition` (Restore) | `content.restore` | content | `trash_in` |
| `content_restore_revision` | `ContentService::restore_revision` | `content.restore_revision` | content | `update_in` with the before fields |
| `media_upload` | `MediaService::record_upload` | `media.upload` | media (created) | media trash (D7) |
| `media_update` | `MediaService::update` | `media.update` | media | `MediaService` update path (title, alt) |
| `media_trash` / `media_restore` | `MediaService::trash` / `restore` | `media.trash` / `media.restore` | media | media restore / trash `_in` |
| `term_create` | `TaxonomyService::create_term` | `term.create` | term (created) | delete the term |
| `term_update` | `update_term` | `term.update` | term | `update_term` path with name, slug, parent, description |
| `term_delete` | `delete_term` | `term.delete` | term | re-insert with the same id and its item links (§7.3) |
| `menu_set` | `MenuService::put` | `menu.put` | menu (by name; created when new) | `put_in(before)`, or delete when it was new |
| `content_type_create` | `ContentTypeService::create` | `type.create` | content type (created) | `delete` rules (refused while it has items) |
| `content_type_update` | `ContentTypeService::update` | `type.update` | content type | `update_in` with the before label, `has_archive`, schema |
| `redirect_create` | `RedirectService::upsert` | `redirect.create` | redirect (by source; may exist) | delete, or the before row |
| `redirect_update` / `redirect_delete` | `update` / `delete` | `redirect.update` / `redirect.delete` | redirect (by id) | the before row (re-inserted with its id and origin) |
| `comment_moderate` | `CommentService::moderate` | `comment.moderate` | comment | `moderate(before.status)` |
| `comment_reply` | `CommentService::submit` (`Commenter::User`) | `comment.create` | comment (created) | `moderate(trash)` |
| `settings_update` | `OptionService::set_many` → `write_changes` | `settings.update` | one option per key | `set_many_in` with the before values (`null` = default) |
| `theme_activate` | `ExtensionAdmin::activate_theme` | `theme.activate` | active theme | `write_activate_theme` of the before theme (§7.3) |
| `theme_settings_update` | `ExtensionAdmin::set_theme_settings` | `theme.settings` | option `theme.<name>.settings` | `write_theme_settings` with a patch that yields the before object |
| `plugin_enable` / `plugin_disable` | `ExtensionAdmin::set_plugin_enabled` | `plugin.enable` / `plugin.disable` | plugin | `write_plugin_enabled(before.enabled)` |
| `plugin_settings_update` | `ExtensionAdmin::set_plugin_settings` | `plugin.settings` | one option per key | the before option rows written as stored (sealed secrets stay sealed, D12) |

(The embedded-theme path of `theme_activate` installs a row first through
`ExtensionAdmin::install_theme`; the captured object is still the active theme.)

A new MCP write tool added later (M6-1's translate, M6-3, M6-4) must capture; the guard
test `every_write_tool_records_a_change` (plan Task 8) calls each tool whose annotations
say `readOnlyHint: false` and fails if it records nothing.

Side effects that core derives from a write (redirects deleted on publish, the 301 a
slug change records, the `permalink.redirects` job of a permalink change, `post.saved`
action jobs) are not captured as changes of their own; a reverse write derives its own
side effects again (D8).

## 4. Images

`ObjectRef` (`changes/object.rs`) names an object and reads its **snapshot**, a JSON
object of what a person can change, without volatile columns:

| Kind (`object_kind`) | `object_id` | Snapshot members |
|---|---|---|
| `content` | item id | `kind`, `locale`, `translation_group` (M6-1), `status`, `slug`, `parent_id`, `author_id`, `title`, `excerpt`, `body`, `fields`, `menu_order`, `published_at`, `scheduled_at`, `terms` (ids, sorted), `seo` ([`Seo::load`]: title, description, canonical, robots) |
| `media` | item id | the `content` members plus `alt` (the `media` row) and `trashed_from` (meta `TRASHED_FROM_META`) |
| `term` | term id | `taxonomy`, `locale` (M6-1), `slug`, `name`, `description`, `parent_id`, `items` (linked item ids, sorted, at most 1,000) and `items_total` |
| `menu` | menu name (M6-1: `<locale>/<name>` if menus are keyed per locale) | the stored `Menu` (`label`, `items` with ids, parents, positions, kinds, targets, urls, labels), `locale` |
| `type` | type name | the stored `ContentType` (label, `has_archive`, `fields`, `built_in`, …) |
| `redirect` | redirect id | `source_path`, `target`, `code`, `origin` (not `hits`, `last_hit_at`) |
| `comment` | comment id | `item_id`, `parent_id`, `status`, `user_id` (no body, name or e-mail: personal data stays in `comments`) |
| `option` | option key | `{ "value": <stored JSON> }`; a plugin secret as stored (`{"sealed": "v1.…"}`) |
| `theme` | `active` | `{ "name", "artifact" }` of the active theme row (absent: the built-in default) |
| `plugin` | plugin name | `enabled`, `disabled_reason`, `version`, `artifact` |

Left out everywhere: `version`, `created_at`, `updated_at`, `body_html`, counters.

- **Canonical form**: object keys sorted recursively, no insignificant whitespace
  (`changes::canonical(&Value) -> Vec<u8>`), so Postgres `jsonb` (which reorders keys)
  and SQLite `TEXT` give the same bytes for the same state.
- **Hash**: SHA-256 hex of the canonical bytes (`after_hash`, `before_hash`). The revert's
  conflict check compares hashes only (§7.2).
- **Storage**: the canonical bytes compressed with `zstd::bulk::compress(.., 3)` (the
  workspace's pinned `zstd =0.14.3`, already in the binary through `ferrite`'s archives;
  `ferrite-core` gains the dependency, D10). Read back with
  `zstd::bulk::decompress(.., MAX_IMAGE)`.
- **Caps**: a canonical image above `MAX_IMAGE` = 16 MiB (a 4 MiB body is far below it)
  is stored *skipped* (hash only); its change shows "too large to keep" and cannot be
  reverted. The term `items` list is capped at 1,000 ids (`items_total` keeps the count;
  revert re-links only the stored ids and says how many were left out).

## 5. Storage (migration `0018_change_sets`)

Expand-only, both backends, `.down.sql` drops both tables; contract tests on both.

SQLite (`crates/ferrite-store-sqlite/migrations/0018_change_sets.up.sql`):

```sql
-- ferrite:phase expand
-- AI change sets (design 17 §5). `change_sets`: one working session of one MCP agent
-- (an OAuth grant or a personal token; `agent_id` has no foreign key: grants and tokens
-- are deleted while their history stays). `open_key` ('oauth:<id>' / 'token:<id>') is set
-- while the set takes new changes; UNIQUE so concurrent writers of one agent meet in one
-- row. `change_set_changes`: one object touched by one write, with zstd-compressed
-- canonical JSON before/after images and their SHA-256 hashes (hash NULL = absent; hash
-- without bytes = too large to keep).
CREATE TABLE change_sets (
    id          BLOB CONSTRAINT change_sets_pkey PRIMARY KEY NOT NULL,
    user_id     BLOB CONSTRAINT change_sets_user_id_fk REFERENCES users (id) ON DELETE SET NULL,
    agent_kind  TEXT NOT NULL CHECK (agent_kind IN ('oauth', 'token')),
    agent_id    BLOB NOT NULL,
    client      TEXT NOT NULL,
    open_key    TEXT CONSTRAINT change_sets_open_key_uq UNIQUE,
    started_at  INTEGER NOT NULL,
    last_at     INTEGER NOT NULL,
    changes     INTEGER NOT NULL DEFAULT 0,
    reverted_at INTEGER,
    reverted_by BLOB CONSTRAINT change_sets_reverted_by_fk REFERENCES users (id) ON DELETE SET NULL
);
CREATE INDEX change_sets_user_idx ON change_sets (user_id, last_at);
CREATE INDEX change_sets_agent_idx ON change_sets (agent_id, last_at);
CREATE INDEX change_sets_last_idx ON change_sets (last_at);
CREATE INDEX change_sets_reverted_by_idx ON change_sets (reverted_by);

CREATE TABLE change_set_changes (
    set_id        BLOB NOT NULL CONSTRAINT change_set_changes_set_id_fk REFERENCES change_sets (id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    at            INTEGER NOT NULL,
    op            TEXT NOT NULL,
    object_kind   TEXT NOT NULL,
    object_id     TEXT NOT NULL,
    label         TEXT NOT NULL DEFAULT '',
    before_image  BLOB,
    before_hash   TEXT,
    after_image   BLOB,
    after_hash    TEXT,
    reverted_at   INTEGER,
    reverted_by   BLOB CONSTRAINT change_set_changes_reverted_by_fk REFERENCES users (id) ON DELETE SET NULL,
    revert_forced INTEGER NOT NULL DEFAULT 0 CHECK (revert_forced IN (0, 1)),
    CONSTRAINT change_set_changes_pkey PRIMARY KEY (set_id, seq)
);
CREATE INDEX change_set_changes_object_idx ON change_set_changes (object_kind, object_id);
CREATE INDEX change_set_changes_reverted_by_idx ON change_set_changes (reverted_by);
```

Postgres: the same with `uuid`, `timestamptz`, `bytea`, `boolean NOT NULL DEFAULT false`,
`integer`, and `text COLLATE "C"` for `open_key`, `op`, `object_kind`, `object_id` (R8).

- `ferrite-domain/src/change_set.rs`: `ChangeSetId` (in `ids.rs`' `uuid_id!` list),
  `AgentKind { OAuth => "oauth", Token => "token" }` (`string_enum!`), `ChangeSet`,
  `NewChangeSet`, `Image { Absent, Present { hash, bytes }, Skipped { hash } }`,
  `NewChange`, `Change`, `ChangeSummary` (no bytes), `ChangeSetQuery { user, agent_id,
  open, limit, cursor }`, `CHANGE_SETS_DAYS_OPTION = "mcp.change_sets_days"`.
- `ferrite-store::ChangeSetRepo` on `Repos` (`fn change_sets(&self)`), implemented by both
  stores and by the `CountingStore`, `FailingOptionsStore` and test wrappers that
  implement `Repos` (`ferrite-core/tests/media.rs`, `ferrite-cluster/tests/jobs.rs`,
  `ferrite-http/tests/app/reload_race.rs`):
  - `extend_open(open_key, now, idle_since, max_changes) -> Option<(ChangeSetId, u32)>`:
    `UPDATE … SET changes = changes + 1, last_at = now WHERE open_key = ? AND last_at >=
    idle_since AND changes < max RETURNING id, changes`.
  - `open(&NewChangeSet) -> (ChangeSetId, u32)`: clears `open_key` on the row holding it,
    then `INSERT … ON CONFLICT (open_key) DO UPDATE SET changes = change_sets.changes + 1,
    last_at = excluded.last_at RETURNING id, changes` (a concurrent opener of the same
    agent lands in the same set).
  - `insert_change(&NewChange)`, `get(id)`, `list(&ChangeSetQuery) -> Page<ChangeSet>`
    (newest `last_at` first, cursor; each row with its count of reverted changes),
    `summaries(id) -> Vec<ChangeSummary>`, `change(id, seq) -> Option<Change>`,
    `changes(id) -> Vec<Change>` (with images, for the revert; at most 500),
    `mark_reverted(id, seqs, by, at, forced_seqs)`, `close(id)`, `prune(before, limit)`.
- Export catalogue (`crates/ferrite-store/src/catalogue.rs`): both tables `Export::Rows`
  (site history, like `audit_log`; images as `Bytes`), in FK order after `users`.

## 6. Grouping into sets

The MCP endpoint is stateless (`NeverSessionManager`, `legacy_session_mode = false`,
`crates/ferrite-http/src/mcp/mod.rs`): there is no MCP session id to group by, so a set
is **per agent and a time window** (D5):

1. `open_key` = `oauth:<grant>` or `token:<token id>`.
2. `extend_open(key, now, now − 30 min, 500)`. A row: that set and its next `seq`.
3. None (no open set, idle 30 minutes or more, or full): resolve the label (the grant's
   client name, else `API token <name>` as `mcp::auth::client_name` does; a missing row:
   "AI assistant") and `open(..)`, which closes the stale row and inserts a new one.
4. Every change of one write is inserted with consecutive `seq`s (`finish` asks for one
   `seq` per object: `extend_open` adds `n`).

A person's revert of anything in a set closes it (`open_key = NULL`), so later writes of
the assistant start a new set and never mix with a half-reverted one.

**Concurrency**: the set row is touched last in the write's `Tx` (D9), so two writes of
one agent serialize on it only after each holds its own object rows; with SQLite's one
writer nothing interleaves. On Postgres the `UNIQUE (open_key)` makes concurrent openers
meet in one row. Before-images of objects without a version column (terms, menus,
options, redirects) are read in the `Tx` without a row lock; a concurrent commit between
that read and the write can make the image slightly stale (READ COMMITTED; the same
window plan ruling P2 accepts for term cycles).

## 7. Revert

### 7.1 Requests

`ChangeSetService::revert(actor, set, RevertRequest { changes: Option<Vec<u32>>, force:
bool, dry_run: bool }) -> Result<RevertReport, RevertError>`:

- `changes: None` = every change of the set not yet reverted; `Some(seqs)` = those (one
  change in the UI). Already reverted ones are listed as `skipped`.
- Selected changes are applied **newest first** (`seq` descending) in **one
  transaction**; any failure rolls back all of them (D6).
- A selected change with a later, not reverted change *of the same set* on the same
  object is refused (`later_change`, naming that `seq`) unless that later change is
  selected too; force does not override this.
- `dry_run` runs every check and every reverse write, then rolls back and answers what
  would happen (the admin's confirm dialog shows it).

### 7.2 Conflicts

For each selected change, in order, inside the `Tx`: read the object's current snapshot
and compare its hash with the change's `after_hash` (absent = absent):

- equal: revert;
- different: conflict `changed_since` (someone or something edited it after the
  assistant: a person, another set, a job such as a scheduled publish); for content the
  conflict names the newest revision's author and time;
- `after` *skipped*: `too_large`, never revertable;
- object gone where the after-image is present: `gone` (a person deleted it).

Without `force`, any conflict refuses the whole request: `RevertError::Conflicts(vec)`,
HTTP `409 change-conflict` with the list in `report`. With `force`, `changed_since`
changes are reverted anyway (the later edit is overwritten; for content it stays in the
revisions) and recorded with `revert_forced`; `gone` and `too_large` stay refusals. Since
changes run newest first, reverting a whole set whose objects were each touched several
times needs no force: each reverse write leaves the object at the previous change's
after-image.

### 7.3 Reverse writes

All as the reverting person (`Actor` of the session), through crate-internal `_in`
functions in the one `Tx` (`ContentService::update_in`, `publish_in`, `trash_in`,
`restore_in`, `transition_in`; media trash/restore factored to `_in`; `taxonomy`
term `_in` helpers; `menu::put_in`; `types::update_in`; options `set_many_in`;
extension `write_*`). So capabilities, field and schema validation, invalidation events,
revisions (note "Reverted an AI change" on content) and the writes' own audit rows apply
as for any edit. Per kind:

- **content / media**: before absent → trash (media through the media trash path, so
  `trashed_from` is kept). Else: current trash and before not → restore; editable members
  differ (title, slug, parent, excerpt, body, fields, menu order, terms, SEO; locale and
  translation group only if M6-1's update path allows changing them, else shown only) →
  `update_in` with the before values, at the current version read in the `Tx`; then the
  status: `published` → publish (an existing `published_at` is kept, as publish does),
  `draft` → unpublish or restore, `pending` → submit, `scheduled` → schedule at the
  before time if still in the future, else left draft with a note in the report, `trash`
  → trash; media `alt` through the media row. Redirects that a publish deleted are not
  recreated (D8).
- **term**: absent → `delete_term` rules (refused while it has child terms). Present and
  the term exists → the update path with the before name, slug, parent, description (a
  slug change back records its 301 as any rename). Present and gone → insert the term
  with its id (`TermRepo::insert`), then for each stored item id that still exists add the
  link (`for_item` + `set_for_item`), emitting the term and item keys.
- **menu**: absent → `MenuService::delete` path; present → `put_in(before)` (validates
  targets: a trashed target item fails the change; reverse order restores items first).
- **type**: absent → the `delete` rules ("type has content" is a per-change failure);
  present → `update_in` with the before label, `has_archive` and schema. Takes
  `LOCK_TYPES`.
- **redirect**: absent → delete; present → the stored row re-written with its id, source,
  target, code and origin after `check_chain`, emitting `redirect:` keys.
- **comment**: absent (a reply) → status `trash`; present → its before status.
- **option** (settings): one `set_many_in` call for all option changes of the revert
  (`null` for absent), so cross-key checks see the final state; under `LOCK_TYPES` when
  a `permalink.*` key is among them (which enqueues `permalink.redirects` again).
- **theme**: the before theme row must exist with the before artifact hash (it loaded
  then), else the change fails with "activate it on Appearance"; a before-image *absent*
  (the built-in default, no row) fails the same way (D13). Under `LOCK_EXTENSIONS`.
- **plugin**: `write_plugin_enabled(before.enabled)`; refused when the plugin is no
  longer installed. **plugin settings**: the before option values written back as stored
  (a sealed secret copied sealed, never opened; absent → the option deleted),
  `options.changed` per key, under `LOCK_EXTENSIONS`; theme settings through
  `write_theme_settings` with a patch that turns the current object into the before one.

Locks are taken before `begin`, in one order: `LOCK_TYPES`, then `LOCK_EXTENSIONS`, only
those the selected changes need.

### 7.4 Bookkeeping

In the same `Tx`: `mark_reverted` (`reverted_at`, `reverted_by`, `revert_forced`),
`reverted_at`/`reverted_by` on the set when every change is reverted, `close(set)`, an
audit row `change_set.revert` (target `change_set` / id, details `{changes: [seq], forced:
[seq], client}`) and an event `change_set.reverted` (keys `change_set:<id>`) for M6-6's
webhooks; nodes older than M6-5 treat the unknown kind as an `invalidate` of a key no page
records (M1.5 P28), so it is harmless in a mixed cluster. A reverse write by a person is
not captured (no agent), so a revert never opens a change set.

## 8. Permissions

- **See** a set: its user (`read_admin` and scope `users:read`, as Connected apps), or
  `manage_users` for anyone's (administrators), as `OAuthService::grants`. A set whose
  user was deleted (`user_id` NULL) is visible to administrators only.
- **See images** (the diff): additionally the viewer's current right to read the
  object's area (`require_read(actor, area)`; options: `manage_settings`; themes, plugins:
  `manage_themes` / `manage_plugins`). Without it the change shows its op, kind and
  label, `images: "hidden"`. A demoted user cannot read through their old changes.
- **Revert**: a signed-in **session** only (an API token is `403`, as the consent API:
  undoing is a person's decision), CSRF as usual; the set's user or `manage_users`; then
  every reverse write's own checks as that person (a contributor cannot revert a publish
  they could not make; the error names the change and the missing capability).

## 9. REST API (`/api/v1/change-sets`)

DTOs in `crates/ferrite-http/src/api/dto/change_sets.rs`, routes in
`crates/ferrite-http/src/api/change_sets.rs`, `utoipa`-annotated, in `openapi.json`.

| Route | Answer |
|---|---|
| `GET /change-sets?user=&agent=&open=&limit=&cursor=` | `ChangeSetList { items: [ChangeSetDto], next_cursor }`; `user` another user's (`manage_users`), `user=all` every user's (`manage_users`); default own |
| `GET /change-sets/{id}` | `ChangeSetDetailDto { set: ChangeSetDto, changes: [ChangeDto] }` |
| `GET /change-sets/{id}/changes/{seq}` | `ChangeDetailDto { change: ChangeDto, before, after }` (JSON images, redacted, or `images: "hidden"`) |
| `POST /change-sets/{id}/revert` `{changes?: [seq], force?: bool, dry_run?: bool}` | `200 RevertReportDto { reverted: [seq], skipped: [seq], forced: [seq], notes: [{seq, text}] }`; `409 change-conflict` with `report.conflicts: [{seq, reason, object, detail}]`; a reverse write's own error with `report.change = seq` |

- `ChangeSetDto`: `id`, `user` (`{id, login, display_name}` or null), `agent`
  (`{kind: "oauth"|"token", id, client, connected: bool}` — whether the grant or token
  still exists), `started_at`, `last_at`, `open`, `changes`, `reverted`, `status`
  (`open|closed|reverted|partly_reverted`).
- `ChangeDto`: `seq`, `at`, `op`, `object` (`{kind, id, label, admin_path}`; `admin_path`
  is the admin screen of the object when it still exists), `created`, `deleted`,
  `revertable` (`false` for skipped images), `reverted_at`, `reverted_by`, `forced`.
- Redaction in `before`/`after`: plugin secret options become `{"secret": "set"}`
  (`"unset"` when absent); comment snapshots hold no personal data already (§4).
- Rate: the API's usual limits; revert takes an `Idempotency-Key` like the other writes.

## 10. MCP

- New read-only tool **`changes_list`** (`crates/ferrite-http/src/mcp/tools/changes.rs`,
  router `change_tools`): the calling agent's own recent change sets (`limit` 1..=20,
  default 5; or `set` for one) with their changes (`seq`, `at`, `op`, kind, id, label,
  reverted), the open set first, and `review_url` (`<site>/admin/ai-activity/<id>`), so an
  assistant can tell its user what it did and where to review or undo it. No scope beyond
  the token (it lists only this agent's own writes); never images. Annotations
  `readOnlyHint`, `idempotentHint`. 57 tools; the reference page lists it.
- **No revert tool** (D14): undo stays human (12 §5.4); an assistant asked to undo
  something makes a forward edit, which is itself recorded.
- The server instructions gain one sentence: "Your changes are recorded as change sets
  that the user can review and undo in the admin; changes_list shows them."
- `settings_update` refuses every `mcp.*` key (today only `mcp.enabled`), so an assistant
  cannot shorten the retention of its own record (§12).

## 11. Admin

Design rules of 11 (tokens, 44 px targets, axe clean); strings in `admin/src/i18n/en/ai.json`.

- **AI activity** `/admin/ai-activity` (`routes/_app/ai-activity.index.tsx`, screen entry
  `activity` in `shell/nav.ts`, section "admin", shown with `read_admin`; palette "AI
  activity"): the change sets as a table — client (the `ClientLogo`/`ClientKind` marks of
  Connected apps; a key icon for personal tokens), user (administrators; filter), started,
  last change (relative), changes, status badge; filters status and connection
  (`?agent=`). Empty state explains that assistants' changes appear here.
- **Change set** `/admin/ai-activity/$id` (`ai-activity.$id.tsx`): header (client, user,
  time span, counts, "Revert all"), then the changes newest first, each a row with the op
  verb, the object's label linked to its screen, time and state (reverted by whom, forced,
  too large), expanding to a diff loaded from `…/changes/{seq}`:
  - content and media: the revisions drawer's `diffRevision` + `RevisionDiff`
    (`admin/src/editor/diff.ts`, `editor/revisions/revision-diff.tsx`) for title, excerpt
    and body, plus a field table (status, slug, parent, terms, SEO, custom fields);
  - everything else: a generic `JsonDiff` (dotted paths, removed/added values with the
    same tokens and `DiffLegend`); created/deleted objects show one side.
  Per change: "Revert this change".
- **Revert dialog**: runs the request with `dry_run` first and lists what will happen and
  every conflict with its reason ("Edited by Ana 10 min ago", "Changed again later in this
  set", "Deleted since"); with conflicts the confirm is disabled until "Overwrite later
  edits" is ticked (sends `force`); `gone`/`too_large` stay listed as not revertable.
  After success a toast and the list refresh; a 409 from a race reopens the dialog.
- **Connected apps** (`account/connected-apps-card.tsx`, `users/user-apps.tsx`): each grant
  gets an "Activity" link (`/ai-activity?agent=<grant>`); **API tokens**
  (`account/tokens-card.tsx`, `users/user-tokens.tsx`) the same for tokens with sets.
- **Dashboard** AI card (`dashboard/ai-card.tsx`): once a grant exists, "N changes by
  assistants in the last 24 hours — Review" when N > 0.
- **Settings → AI assistants** (`settings/ai-assistants.tsx`): "Keep AI change history
  for N days" bound to `mcp.change_sets_days`.

## 12. Retention

Site option `mcp.change_sets_days` (settings allowlist in `ferrite-core/src/settings.rs`:
integer 1..=365, default 90, `manage_settings`). The built-in periodic job
`change_sets.prune` (`ferrite-cluster/src/builtin.rs`, hourly like the other prunes,
`PRUNE_BUDGET`) deletes sets whose `last_at` is older than that (changes cascade), in
batches of `MAX_PRUNE_BATCH`, reading the option through the store (an unreadable value:
the default). Open sets are never older than 30 minutes, so the prune never races a
writer that matters.

## 13. Cluster, profiles, events

- All state is in `change_sets` / `change_set_changes`; any node records and reverts.
  The set row and the unique `open_key` coordinate concurrent writers (§6); revert takes
  the database locks the writes need (§7.3).
- Cache invalidation needs nothing new: every reverse write emits its own events.
- Solo (SQLite) and cluster (Postgres) behave the same; contract tests on both.
- New event kind for M6-6: `change_set.reverted` (keys `change_set:<id>`). A webhook
  consumer reads the set through the API. No event per captured change (the content,
  term, … events of the write itself already exist).

## 14. Security

- No privilege gain: revert runs as the person, through the same checks; images are shown
  only to who may read the area now; API tokens cannot revert.
- Secrets: plugin secret values are copied sealed and redacted in every DTO; comments'
  personal data is not copied into images; theme settings have no secrets (06 §1).
- Hostile inputs: the `seq` list (≤ 500, deduplicated, unknown → `422`), `force` /
  `dry_run` booleans; images are produced by core, never taken from a client, and are
  size-capped before compression and on decompression (`MAX_IMAGE`).
- A revert overwriting a person's later edit needs an explicit `force`, and the revisions
  keep that edit for content.
- The record cannot be erased by the assistant: no tool deletes sets, `mcp.*` settings are
  refused over MCP, the prune follows the option a person sets.

## 15. Out of scope

Redo; reverting side effects outside the database; capture of REST/admin/CLI writes;
naming or annotating sets; per-tool allow lists; MCP elicitation to confirm a revert;
diffs of plugin KV data (plugins write it themselves); notifications when an assistant
opens a set (M6-6 webhooks can follow `mcp.*` audit rows later).

## 16. Decisions

- **D1** Capture in core service methods inside the write's `Tx`, keyed by `Actor.agent`:
  MCP and REST share those methods, and only the `Tx` gives a consistent before-image and
  no record for a rolled-back write.
- **D2** Only `/mcp` marks the actor: REST writes by personal tokens are scripts, not
  assistants, and the roadmap scopes the feature to MCP.
- **D3** Personal tokens over MCP are agents exactly like grants (key = token id): Claude
  Code and Cursor connect that way (12 §3.1).
- **D4** A failed capture fails the write: "every assistant write is reviewable" must hold;
  the post-commit audit row stays best-effort as in M5.
- **D5** Sets = agent + 30-minute idle window (max 500 changes): the endpoint is stateless,
  so there is no MCP session id; 30 minutes covers a conversation's pauses.
- **D6** Revert newest first in one transaction, all or nothing: partial reverts of a set
  would leave states nobody saw.
- **D7** Reverting a create trashes (content, media, comments), never deletes: trash is
  reversible and the GC/retention rules already apply.
- **D8** Derived side effects are not replayed; the reverse write derives its own: replaying
  deleted redirects or 301s could fight rules core enforces now.
- **D9** The set row is touched last in each write's `Tx`: it is the one row every write of
  an agent shares, so taking it last avoids lock-order cycles on Postgres.
- **D10** Images are full canonical snapshots, zstd-compressed, not diffs: a revert needs
  the whole before state, diffs are computed for display; `zstd` is already in the binary.
- **D11** Conflict = current hash ≠ after hash, whoever changed it: simple, exact, and the
  newest-first order makes multi-touch sets revert without force.
- **D12** Plugin secret settings are copied sealed and restored as stored, never opened:
  the API never returns a secret and the host's seal rules stay intact.
- **D13** Theme activation is reverted only to an installed row with the same artifact;
  anything else asks the person to use Appearance (the load check lives in the HTTP layer).
- **D14** No MCP revert tool; `changes_list` is read-only: undo is a person's decision
  (12 §5.4), and `force` could overwrite a person's work.
- **D15** Revert is session-only and limited to the set's user and administrators, with
  every reverse write checked as that person.
- **D16** Images are visible only with the viewer's current read rights on the area.
- **D17** Retention is a site option (`mcp.change_sets_days`, default 90), changeable only
  in the admin; change sets are exported and backed up like the audit log.
- **D18** A revert closes the set, so later assistant writes start a new one.
