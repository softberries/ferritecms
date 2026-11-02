# 16 — Accessibility and content health

Status: **design for M6-4** · Date: 2026-11-02

Two features on one rule engine:

1. **Checks while writing.** The editor shows accessibility and link problems of the item
   being written — inline in the text and as a list in the side panel — and a per-type
   setting can refuse publishing while the item has accessibility errors. The same checks
   are an MCP tool, so an assistant can check (and fix) what it wrote before it publishes.
2. **Content health.** A periodic job checks the whole published site — broken internal and
   (opt-in) external links, orphan pages, missing alt text, redirect chains and loops,
   items not edited for a year — and an admin screen, a dashboard card and an MCP tool show
   the result.

Wave B of M6: this design assumes M6-1's locale model (every content item has `locale`, a
BCP 47 tag, and `translation_group`; terms and menus have a `locale`; `site.locales` lists
the site's locales, default first) and M6-6's outgoing webhooks (it names one new event,
§9). Roadmap: "M6-4 — Accessibility and content health".

## 1. Goals and non-goals

Goals:

1. **One implementation.** Every rule is Rust in `ferrite-core::quality`. The editor asks
   the server (`POST /api/v1/content/{type}/check`) instead of running a JavaScript copy;
   the MCP tool, the publish gate and the health job call the same functions (D1).
2. **Stable rule ids** with a severity, a category and the WCAG 2.2 success criteria they
   relate to, so suppressions, the API, the docs and assistants can name a rule (§3).
3. **Never break publishing paths that have no human to fix things.** Imports, the seed,
   scheduled publishing at its due time and media uploads are never refused by the checks
   (§5).
4. **Bounded cost.** Checks of a body are linear and capped; the health job pages through
   the site in short transactions; external links are opt-in, rate-limited per host,
   cached, and never checked from a request (§6).
5. **Both profiles.** Solo and cluster: the job runs on one node per period (job queue plus
   a named lock); results live in the database, so every node serves the same report (§8).

Non-goals (this milestone): checking rendered theme output (colour contrast, landmarks:
that is what the axe checks of the starter themes do in CI, `admin/e2e/public-themes.spec.ts`),
checking raw `html` blocks beyond saying they are not checked, readability scores,
spelling, plugins adding rules (later: a WIT hook, §12), a CLI command for the health run,
checking images' pixels (text in images).

## 2. Where the checks run

| Path | Runs | Effect |
|---|---|---|
| Editor (admin) | `POST /api/v1/content/{type}/check` with the working draft, debounced (1.5 s after the last change) and before the publish dialog opens | Inline marks and the panel list; the publish dialog shows the gate |
| Publish / schedule (REST, admin, MCP, bulk) | `ContentService::publish` / `schedule` in core | Refused with `CoreError::QualityBlocked` when the type's gate is on and accessibility errors remain (§5) |
| Update of a live item (REST, admin, MCP, revision restore) | `ContentService::update_in` | Refused when the gate is on and the edit adds accessibility errors (§5) |
| MCP | `content_check` (an item, or a draft body) and `health_report` | Tool results (§10) |
| Health job | `content.health`, daily 04:00 UTC and on "Run now" | Rows in `health_findings` (§6) |
| Imports, seed, due-time scheduled publish, media | — | Never checked on write; the next health run reports them |

No rule runs in a public request (page rendering is unchanged except for decorative
images, §4.1).

## 3. Rules

Each rule is a `Rule { id, severity, category, wcag, scope }` in a static table
(`ferrite-core/src/quality/rules.rs`), enumerated in the OpenAPI schema of `FindingDto.rule`
and listed on the docs site (a test keeps the three equal). Severities:
**error** (a clear failure a reader meets), **warning** (very likely a problem; a person
decides), **notice** (worth a look). Categories: `accessibility`, `links`, `media`,
`site`, `freshness`. Scope: `item` (runs on one item's title, body and rich-text fields; in
the editor, MCP and the health job) or `site` (health job only).

| Rule id | Sev. | Cat. | WCAG 2.2 | Scope | Finds |
|---|---|---|---|---|---|
| `image-alt-missing` | error | accessibility | 1.1.1 (A) | item | An `image` whose `alt` is absent or blank (after trimming and removing format characters) and that is not marked `decorative` (§4.1) |
| `image-alt-filename` | warning | accessibility | 1.1.1 (A) | item | `alt` that is a file name (ends in `.jpg`, `.jpeg`, `.png`, `.gif`, `.webp`, `.avif`, `.svg`, ASCII case-insensitive), a camera name (`IMG_1234`, `DSC01234`, `PXL_…`), or one of `image`, `photo`, `picture`, `img`, `graphic` |
| `heading-empty` | error | accessibility | 1.3.1 (A), 2.4.6 (AA) | item | A `heading` with no text |
| `heading-level-skipped` | warning | accessibility | 1.3.1 (A) | item | A heading more than one level below the previous one (the title counts as level 1, so a body that starts with `h3` is flagged) |
| `link-text-empty` | error | accessibility | 2.4.4 (A), 4.1.2 (A) | item | A link whose text is blank |
| `link-text-vague` | warning | accessibility | 2.4.4 (A) | item | A link whose whole text is a vague phrase of the item's language (§4.2) |
| `link-text-url` | notice | accessibility | 2.4.4 (A) | item | A link whose text is a bare `http(s)` URL |
| `table-header-missing` | error | accessibility | 1.3.1 (A) | item | A `table` with no `tableHeader` cell in its first row and none starting each row |
| `table-header-empty` | warning | accessibility | 1.3.1 (A) | item | A `tableHeader` cell with no text |
| `html-unchecked` | notice | accessibility | — | item | An `html` block (raw HTML is not checked) |
| `media-missing` | error | media | — | item | An image `ferrite:media/<id>` (body, rich-text field) or the `featured_image` naming no media item, or one that is not public (trashed): the page renderer drops such images (`site/hydrate.rs` resolves only public media, and `blocks::render`'s `render_image` writes nothing for an unresolved id) |
| `featured-alt-missing` | warning | media | 1.1.1 (A) | item | A featured image whose media item has a blank `alt` (themes use the media item's alt) |
| `link-internal-broken` | error | links | — | item | A link to a path on this site that answers 404 or 410 (§6.2) |
| `link-internal-redirected` | notice | links | — | item | A link to a path on this site that redirects (a stored redirect or a canonical 301); the detail names the final path |
| `link-anchor-missing` | notice | links | — | item | A `#fragment` link with no heading id of that name in the same document |
| `link-external-broken` | error | links | — | site | An external `http(s)` link that failed (§6.3; only when `health.external_links` is on) |
| `orphan` | warning | site | — | site | A published item of a type without an archive that nothing leads to (§6.4) |
| `redirect-chain` | warning | site | — | site | A redirect whose target is itself redirected (the detail lists the hops) |
| `redirect-loop` | error | site | — | site | Redirects that lead back to where they started |
| `redirect-target-broken` | error | site | — | site | A redirect to a path on this site that answers 404 or 410 |
| `stale` | notice | freshness | — | site | A published item whose content was last edited more than `health.stale_days` ago (§6.5) |

Two more ids are reserved for the engine itself: `check-truncated` (notice; more than 500
findings on one item, or a body nested deeper than 64 levels: the rest is not checked) and
`findings-truncated` (notice on the run; more than 100 000 findings).

Rule ids are stable: a rule is never renamed; a retired rule's id is never reused.

### 3.1 What a finding is

```rust
pub struct Finding {
    pub rule: RuleId,            // &'static Rule
    pub location: Location,      // Title | Body { path } | Field { name, path } | FeaturedImage | Item | Redirect { id }
    pub key: String,             // the suppression key (§7), ≤ 300 characters
    pub context: Option<String>, // what was found: link text, alt, href, heading text (≤ 120 characters)
    pub detail: serde_json::Value, // rule-specific, ≤ 2 KiB (e.g. {"href": "/x/", "status": 404})
}
```

`path` is the list of child indices from the document root (`content[i]` at each level),
which is how the editor maps a finding to a position in its own document (§11.1). The
message a reader sees is built from the rule id and `context` by the admin's i18n and by
the MCP layer (English); core stores no prose.

## 4. Rule details

### 4.1 Decorative images

Today the editor's alt popover treats an empty `alt` as decorative
(`admin/src/editor/image-node.tsx`), and the WordPress importer copies `alt=""` from the
export as it is (`ferrite-import-wp/src/convert/doc.rs`): an empty alt cannot tell
"decorative" from "forgotten". The image node gets an optional attribute
`decorative: true` (D2):

- The editor's alt popover gets a "Decorative image" switch; on, it stores
  `decorative: true` and `alt: ""`.
- `blocks::render`'s `render_image` writes `alt=""` for a decorative image whatever its
  `alt` holds (unchanged otherwise).
- `blocks::markdown`'s `image` returns `None` for a decorative image, so it becomes a
  `ferrite-block` fence and round-trips (§5.3 of 12); Markdown `![](…)` in is not
  decorative.
- `image-alt-missing` skips decorative images.
- `is_editor_safe` checks node types, marks and heading levels only, so it is unchanged;
  the admin's `editor-schema.json` fixture lists no attributes.

Existing images with `alt: ""` become `image-alt-missing` findings. That is intended: the
health report is how a site finds them, and the fix is one click (§11.1).

### 4.2 Vague link text

A link's text is the concatenation of consecutive text nodes in one parent that carry the
same `link` mark (`href` equal), trimmed, with format characters (`Cf`) removed. It is
compared, lowercased and with trailing `.`, `…`, `:`, `»`, `→` stripped, against the list
for the item's language (the primary subtag of its `locale`, M6-1), plus the English list
for every language. Lists are data in `quality/vague.rs`:

- `en`: click here, here, read more, more, learn more, this, link, this link, continue,
  details, go
- `de`: hier, hier klicken, klicken sie hier, mehr, weiterlesen, mehr erfahren
- `fr`: ici, cliquez ici, en savoir plus, lire la suite, plus
- `es`: aquí, haz clic aquí, leer más, más, más información
- `pl`: tutaj, kliknij tutaj, więcej, czytaj więcej

Other languages use the English list only (D3). Adding a language is a data change.

### 4.3 Walking a body

One iterative walk (an explicit stack, as `is_editor_safe` does) over the body and every
`rich_text` field (`fields.rs` `FieldKind::RichText`, including inside repeaters),
in document order. Input is untrusted JSON: wrong types are skipped, not errors. Bounds:
depth 64 (as `blocks::MAX_DEPTH`), 500 findings per item, 1 000 links per item; past a
bound the walk stops and adds `check-truncated`. Heading ids for `link-anchor-missing` come
from the same function the renderer uses (`blocks/anchor.rs`, exposed as
`blocks::heading_ids`), so they match the rendered `id`s.

## 5. The publish gate

A per-type setting `quality_gate`: `off` (default; checks only report) or `errors`
(accessibility errors block publishing). It is a column on `content_types` (§7), edited
with the type (Content types screen, `PATCH /api/v1/types/{name}`, MCP
`content_type_update`); the `media` type refuses `errors` (`InvalidField`), having no body.

With `errors`:

- **Publish and schedule** (`ContentService::publish`, `schedule`; every caller: REST
  `POST …/publish|schedule`, the admin's bulk publish, MCP `content_publish` and
  `content_schedule`) run the item-scope rules on the stored item inside the transition's
  transaction; unsuppressed findings of category `accessibility` with severity `error` →
  `CoreError::QualityBlocked { findings }` (at most 50 listed). Links, media and site
  rules never gate (D4): a broken link may be fixed by publishing its target in the same
  release, and the gate must not depend on the router or the network.
- **Editing a live item** (`update_in` for an item that is `published` or `scheduled`;
  `restore_revision` goes through it too): refused only when the edit *adds*
  accessibility errors (the unsuppressed error count of the new title, body and fields is
  higher than that of the stored ones). An editor can always fix a typo in a page that
  was published before the gate was turned on (D5).
- **Not gated**: `submit` (draft → pending: review is where it gets fixed), the
  scheduler's due-time publish (`publish_due`, `publish_scheduled`: system paths with no
  one to tell; the item was checked when it was scheduled, and its edits since were
  gated) (D6), `ContentService::publish_in` (seed and media uploads), imports (they write
  through `ImportBatch`, never through these methods) (D7).
- Suppressed findings do not count (§7.2).

`publish_in` gets no gate: callers that need one (M6-3's releases) call
`quality::gate::check(repos, &item, &ty)` themselves inside their transaction (D8).

REST answers `422` with problem type `content-quality` (title "This item has
accessibility errors", `findings`: the finding DTOs). MCP returns an error result whose
text lists each finding and whose structured content holds them. The admin's publish
dialog already knows from its last check and disables the confirm button (§11.1); the
`422` is the backstop.

## 6. Content health

### 6.1 The job

`content.health` (payload `{"v":1,"trigger":"schedule"|"manual","user":<id>|null}`):

- **Periodic**: daily at 04:00 UTC (`Periodic { every: 86 400 s, offset: 4 h }`, after
  `artifacts.gc` at 03:00).
- **Manual**: `POST /api/v1/health/runs` enqueues one with key
  `content.health:manual:<uuid v7>` unless a `content.health` job is queued or running
  (`409`, the same check as `import_job_active` in `ferrite-core/src/import/jobs.rs`).
- **Exclusive**: the handler takes the named lock `content.health` with `Store::try_lock`;
  held → `JobError::Busy` (back in the queue without an attempt).
- **Timeout** 1 h (`JobHandler::timeout`); the runner extends the lease while it runs.

A run (`ferrite-core/src/quality/health.rs`, `HealthRunner::run`):

1. Mark older `running` runs `abandoned`; insert the run row (`running`); delete runs
   older than 30 days except the newest `done` one.
2. **Items**: page through items with status `published` or `scheduled` of every type
   but `media`, 200 per page by `(id)` keyset (a new store read,
   `QualityRepo::published_page`). Per page: one media read for every referenced media id,
   the suppressions of the page's items, the item-scope rules except links; links are
   collected into an internal map (path → referrers) and an external map (URL →
   referrers), each capped (50 000 paths, 20 000 URLs; beyond: `findings-truncated`).
   Findings are written per page in one short transaction.
3. **Internal links**: each distinct path once through the `PathProbe` (§6.2); referrers
   get `link-internal-broken` / `link-internal-redirected`; the item ids the paths resolve
   to are marked *linked*.
4. **Orphans** (§6.4), from the menus (`MenuRepo::list` + `get`), `reading.front_page`
   and the linked set.
5. **Redirects**: page through `RedirectRepo::list` (by source); build the source → target
   map (matching sources as `RedirectService::resolve` does: exact, then the trailing slash
   toggled); `redirect-loop`, `redirect-chain` (≥ 2 hops), and internal final targets
   through the probe → `redirect-target-broken`.
6. **Freshness**: the newest revision time per item (`QualityRepo::last_revision_at`, one
   grouped query per 200 ids; an item with no revision uses `published_at`, else
   `created_at`) → `stale`.
7. **External links** when `health.external_links` is on (§6.3).
8. Finish: totals per rule and severity into the run row, status `done`; delete the
   findings of every other run; append the event `health.completed` (§9).

A failed run (store error) is marked `failed` with a sanitized error and the job retries
under the usual backoff; a run cut short by the timeout is `abandoned` by the next.

### 6.2 Internal paths (`PathProbe`)

A core trait (`quality::PathProbe { async fn probe(&self, path_and_query) -> PathStatus }`)
implemented in `ferrite-http/src/quality_probe.rs` (`SiteProbe`) over the node's
`AppState`. A path is internal when it is a local path (`/…`, not `//`) or an absolute
URL on `site.url`'s origin. The probe never renders a page, never touches the page cache,
the metrics, `request.served` or plugins (D9):

1. The redirect table (`RedirectService::resolve`), followed up to 10 hops; a `410`
   target is `Gone`; an external target is `External(url)`.
2. `route::resolve` against the node's route table; `Route::Canonical(p)` is followed once
   (`Redirected`).
3. Content, front, archive, term, author, date and search routes: the page pipeline's
   lookup step (`app/pipeline.rs` `load`, made `pub(crate)`) → `Ok { item }` when it finds
   something, else `NotFound`. `Route::Media` → the artifact row exists. Robots, sitemaps,
   feeds, theme assets, health → `Ok`. `/_preview/…` → `NotFound` (never public).
4. Paths outside the public router (`/_p/…` plugin routes, `/admin/…`, `/api/…`, `/mcp`,
   `/oauth/…`, `/.well-known/…`) → `Unchecked`.

`PathStatus = Ok { item: Option<ContentId> } | Redirected { to, item } | NotFound | Gone |
External(String) | Unchecked`. The editor's check endpoint uses the same probe (at most
200 distinct paths per call), so the editor shows broken internal links too.

### 6.3 External links (`LinkProbe`)

Off by default (`health.external_links`, D10). When on:

- **Client**: `ferrite-http/src/link_check.rs` (`LinkChecker`), built like
  `import_fetch::HttpSource` on the egress module's client: the resolver refuses a name
  when any address is not public (`egress::is_public_ip`), the peer is checked again, no
  proxy, cookies, `Referer` or decompression; IP-literal hosts must pass the address rule.
  `http` and `https` only, default ports only (80/443; other ports are `Unchecked`, D11),
  no credentials in the URL. User-Agent `ferrite/<version> linkcheck (+<site.url>)`.
- **Request**: `HEAD`; `405`/`501` → `GET` whose body is dropped after the headers. At
  most 5 redirects, each hop checked as the first URL. 10 s per URL, everything included.
- **robots.txt** (RFC 9309): fetched once per host per run (cap 64 KiB, 5 s; parsed by a
  small in-house parser for the groups `ferrite-linkcheck` and `*`, `Allow`/`Disallow`
  with `*` and `$`). A disallowed URL is `Unchecked (robots)`, not broken. robots.txt
  `4xx` → allow all; `5xx`, timeout or a refused address → the host is skipped this run.
- **Pace**: 4 checks at once per run, 1 at a time per host, 2 s between requests to one
  host, at most 1 000 URL checks per run and 30 minutes for the phase; what is left
  waits for the next run (the oldest results first).
- **Classification**: 2xx/3xx-to-2xx → ok; `401`/`403` → ok (exists, restricted); `404`,
  `410`, NXDOMAIN → broken at once; `429` → retry next run; `5xx`, timeout, connect or TLS
  errors → failing, reported as broken after 2 failing checks at least 20 h apart.
- **Cache**: `link_checks` (§7): an `ok` result is reused for 7 days, a broken one is
  re-checked every run; rows not referenced for 30 days are deleted. The cache is shared
  by all nodes.
- Logs name the host only, never the URL.

Never called from a request: only the health job holds a `LinkChecker` (D12).

### 6.4 Orphans

An *orphan* is a published item of a type with `has_archive = false` (pages and custom
types without an archive) that is not the front page, not the target of any menu item of
its locale, not linked from any published item's body or rich-text fields, and whose
parent is not reachable either (a child of a reachable page counts as reachable: the
starter themes show the page tree and breadcrumbs, `site/tree.rs`) (D13). Types with an
archive are reachable from it. Detail: `{"has_parent": bool}`.

### 6.5 Freshness

`health.stale_days` (30..=3650, default 365). "Last edited" is the newest revision's
`created_at`, because `content_items.updated_at` moves on every status transition
(`ferrite-store-sqlite/src/repos/content.rs` binds `Timestamp::now()` on every update)
(D14).

## 7. Storage (migration `0019_content_health`)

Expand-only, both backends, `.down.sql` (`-- ferrite:lossy-down`: suppressions and the
gate setting are dropped), contract tests on both.

- `content_types.quality_gate TEXT NOT NULL DEFAULT 'off' CHECK (quality_gate IN ('off',
  'errors'))` → `ContentType.quality_gate: QualityGate` (`ferrite-domain`).
- `quality_suppressions(item_id FK→content_items ON DELETE CASCADE, rule TEXT, key TEXT
  ('' = every finding of the rule on the item), reason TEXT (1..=500 characters),
  user_id NULL FK→users ON DELETE SET NULL, created_at, PRIMARY KEY (item_id, rule, key))`.
- `health_runs(id PK (uuid v7), status TEXT 'running'|'done'|'failed'|'abandoned',
  trigger TEXT 'schedule'|'manual', requested_by NULL FK→users SET NULL, node TEXT,
  started_at, finished_at NULL, items_checked INT, paths_checked INT, urls_checked INT,
  totals JSON ({rule: count}), truncated BOOL, error TEXT NULL)`, index `(status,
  started_at)`.
- `health_findings(run_id FK→health_runs CASCADE, seq INT, rule TEXT, severity TEXT,
  category TEXT, item_id NULL FK→content_items CASCADE, redirect_id NULL FK→redirects
  CASCADE, item_type TEXT NULL, locale TEXT NULL, key TEXT, context TEXT NULL, detail
  JSON, PRIMARY KEY (run_id, seq))`, indexes `(run_id, category, severity, seq)` and
  `(run_id, item_id)`. `item_type` and `locale` are copied for the filters.
- `link_checks(url_hash BLOB/bytea PK (sha-256 of the URL), url TEXT, host TEXT, status
  TEXT 'ok'|'failing'|'broken'|'skipped', http_status INT NULL, detail TEXT NULL, failures
  INT, first_failed_at NULL, checked_at, last_seen_at)`, index `(last_seen_at)`.

Export catalogue (`ferrite-store/src/catalogue.rs`): the new column and
`quality_suppressions` are exported; `health_runs`, `health_findings` and `link_checks`
are `Export::Skip` (derived; the next run rebuilds them).

Store API: `QualityRepo` on `Repos` (`suppressions_for(items)`, `suppress`,
`unsuppress`, `published_page`, `last_revision_at`, run and finding writes and paged
reads, link cache get/put/prune); `ContentTypeRepo` reads and writes `quality_gate`.

### 7.2 Suppressions

A suppression is `(item, rule, key, reason)`. `key` names one finding (image: its `src`;
link: `href` + `|` + text; heading: level + text; table: `table:<n>`, the n-th table; a
site finding: its own key), or is empty for the whole rule on the item. Suppressed
findings are still returned (marked, with the reason, who and when) but count nowhere:
not in the gate, not in totals. Suppressing an `error` on an item of a gated type needs
`edit_others`; otherwise `can_edit` on the item suffices (D15). Every suppression and its
removal is audited (`quality.suppress`, `quality.unsuppress`). Site findings without an
item (redirect findings) cannot be suppressed: they are fixed in Redirects.

## 8. Cluster and profiles

- The job queue gives each period's job to one node; the `content.health` lock keeps a
  manual run and a periodic run from overlapping (Postgres: advisory lock; SQLite: in
  process, one node anyway).
- Findings, runs, suppressions and the link cache are rows: any node answers
  `/api/v1/health/*` and `check`. Each node's `SiteProbe` uses its own route table, which
  nodes rebuild on `types.changed` already.
- No page-cache keys: nothing public changes. `health.completed` is an outbox event with
  key `run:<id>`, which no page records, so older nodes that treat an unknown kind as
  "invalidate its keys" (M1.5 P28) drop nothing.
- Solo: the run's reads use the reader pool, its writes are short transactions (one per
  page of 200 items), so it does not hold the SQLite writer.

## 9. Events and webhooks

One new outbox event kind: `health.completed`, keys `["run:<id>"]`. M6-6 can offer it as a
webhook event (payload from the run row: totals per severity, started/finished). No
per-item events: a finding is not a change of content.

## 10. API and MCP

REST (`ferrite-http/src/api/quality.rs`, DTOs in `api/dto/quality.rs`, OpenAPI):

| Method and path | Needs | Does |
|---|---|---|
| `POST /content/{type}/check` | `content:read`; for `id`, read access to the item | Body `{id?, title, body, fields?, featured_image?, locale?}` (the editor's draft; 4 MiB JSON cap) → `{findings: [FindingDto], counts: {error, warning, notice, suppressed}, gate: {mode, blocked}}` |
| `GET /content/{type}/{id}/suppressions` | read access | The item's suppressions |
| `POST /content/{type}/{id}/suppressions` | §7.2 | `{rule, key, reason}` → `201` |
| `DELETE /content/{type}/{id}/suppressions/{rule}?key=` | §7.2 | `204` |
| `GET /health/summary` | `edit_others`, `content:read` | The newest `done` run, the running one, the next periodic time, totals by rule and severity, whether external links are on |
| `GET /health/findings` | same | `?category=&rule=&severity=&type=&locale=&limit=&cursor=` → a page of findings with an item ref (id, type, title, status, locale, `admin_url`) or a redirect ref (id, source, target) |
| `POST /health/runs` | `edit_others`, `content:write` | `202 {job_id}`; `409` while one is queued or running |

`check` is a static segment beside `/content/{type}/{id}` (ids are UUIDs, so no item is
shadowed). `TypeDto` and the type patch gain `quality_gate`. Settings allowlist
(`ferrite-core/src/settings.rs`): `health.external_links` (bool, default `false`),
`health.stale_days` (integer 30..=3650, default 365). The 422 of §5 is
`Problem::content_quality`.

MCP (`ferrite-http/src/mcp/tools/quality.rs`, area `content`):

- `content_check` (read-only): `{id}` checks the stored item; `{type, title?,
  body_markdown? | body_blocks?, fields?, locale?}` checks a draft (Markdown converted as
  `content_create` does). Returns the findings with their rule id, severity, WCAG
  criteria, a sentence per finding and `context` (the text, alt or href found), the
  gate, and a hint per rule on how to fix it. The description tells the model to run it
  before `content_publish`.
- `health_report` (read-only): the summary plus one page of findings, filtered like
  `GET /health/findings`.
- `content_publish` / `content_schedule` / `content_update` error results carry the
  blocking findings.
- `ferrite://guide/writing` gains a section on alt text, headings and link text, and
  says to call `content_check` before publishing.

No tool starts a health run or suppresses findings (D16): a suppression is a person's
judgement with a reason, and runs are cheap to ask a person for.

## 11. Admin

### 11.1 Editor

- `editor/quality/use-quality-check.ts`: runs `check` 1.5 s after the last change and
  when the publish dialog opens; one request at a time; a result older than the current
  document revision is dropped.
- `editor/quality/decorations.ts`: a ProseMirror plugin that maps each finding's `path`
  to a node and adds a decoration (a wavy underline for text, an outline for images and
  tables, in `--warning`/`--destructive` tokens) plus a widget button with an `aria-label`
  ("Accessibility: image has no alt text"), so the cue is not colour alone (WCAG 1.4.1).
  Findings in fields show on the field.
- Side panel section "Checks" (`editor/panel/quality-section.tsx`): counts by severity,
  then each finding with its message, "Show" (selects and scrolls to the node), a fix
  action where one exists (image: open the alt popover; heading: focus it), and
  "Suppress…" (a dialog with a required reason), suppressed ones collapsed under
  "Suppressed (n)". Live region for the counts (`aria-live="polite"`).
- Image node: the "Decorative image" switch (§4.1).
- Publish dialog: "2 accessibility errors" with links to them; when the type's gate is
  `errors` and errors remain, the confirm button is disabled with that reason; a `422`
  from the server shows the same list.

### 11.2 Screens

- **Content health** (`/health`, `routes/_app/health.index.tsx`, `admin/src/health/`;
  nav section "content", `edit_others`, shortcut `g h`, free in 11 §6): summary tiles
  (errors, warnings, notices; last run, next run; "Run now"), tabs Accessibility, Links,
  Media, Site, Freshness; a table of findings (rule, item with type and locale, context,
  severity) with filters for type, locale and severity; each row links to the item's
  editor (`?check=<rule>:<key>` opens the panel at the finding). Empty and never-run
  states. External links off: a note linking to Settings.
- **Dashboard**: a "Content health" card (`dashboard/health-card.tsx`) with the three
  counts and the last run, for `edit_others`.
- **Content types**: the type form gets "Accessibility checks: Report only / Block
  publishing on errors".
- **Settings → Content health** (a new group in `settings/groups.ts`): external link
  checks (with the cost and privacy note: the server contacts the linked sites) and the
  stale threshold.

## 12. Security notes

- Bodies, fields, Markdown, robots.txt and HTTP responses are untrusted: walks are
  iterative and capped (§4.3), robots.txt is size-capped, response bodies are not read
  (except robots.txt).
- SSRF: external checks use the egress resolver rules (no private, loopback, link-local
  or unique-local addresses, checked at resolve and at connect, every redirect hop
  re-checked); internal paths never leave the process.
- Privacy: external checks reveal to linked sites that the site links to them, from the
  server's address. Off by default; the setting says so.
- The check endpoint reads only what the caller may read: media facts come from ids in
  the posted body but return only "exists / trashed / alt blank", never titles or URLs of
  media the caller cannot read; probe results return only status and the resolved path.
- Suppressing gate errors needs `edit_others` (§7.2), so an author cannot switch the gate
  off for their own item.

## 13. Out of scope and later

Plugin-provided rules (a `quality` WIT hook in a new world version), checks of rendered
theme HTML, colour contrast of content (inline colours do not exist in the editor
schema), caption checks for video embeds (WCAG 1.2.2; a provider API would be needed),
a CLI `ferrite health`, per-locale vague-phrase lists beyond five languages, notifications
when a run finds new errors (M6-6 webhooks or a later mail), a "fix all" for alt text with
AI suggestions.

## 14. Decisions

- **D1** One Rust rule engine called through an endpoint; no JS copy — the checks need
  store reads (media, suppressions) and the router anyway, and two copies drift.
- **D2** An explicit `decorative` image attribute — an empty alt cannot tell decorative
  from forgotten, and imports produce the second kind.
- **D3** Vague-phrase lists for en, de, fr, es, pl; others fall back to English — a
  short list of exact phrases has few false positives; more languages are data.
- **D4** Only `accessibility` errors gate — links and media depend on other items, the
  router and the network; a gate must be deterministic from the item.
- **D5** Live edits are refused only when they add errors — turning the gate on must not
  lock editors out of fixing old pages.
- **D6** The due-time scheduled publish is not gated — nobody would see the refusal; the
  item was gated when it was scheduled and on every edit since.
- **D7** Imports and the seed are never gated — the roadmap says imports must not be
  blocked; the health job reports them.
- **D8** `publish_in` stays ungated and `quality::gate::check` is public in the crate —
  its callers (seed, media) need no gate; M6-3 releases call the check explicitly.
- **D9** The internal probe uses the pipeline's lookup, not an in-process HTTP request —
  no rendering, no analytics or metrics pollution, no plugin calls.
- **D10** External link checks are opt-in — they contact third parties from the server.
- **D11** Only default ports for external checks — a link checker is not a port scanner of
  public hosts.
- **D12** External checks only in the job, never in `check` — request latency and abuse.
- **D13** A child of a reachable page is reachable — the page tree helpers and
  breadcrumbs make it so in the starter themes; a theme without them may hide real
  orphans (stated in the docs).
- **D14** "Last edited" is the newest revision — `updated_at` moves on status changes.
- **D15** Suppressing a gate error needs `edit_others` — otherwise the gate is advisory.
- **D16** No MCP tool suppresses or starts runs — a suppression is a reasoned human call.
- **D17** The gate setting is a `content_types` column, not a site option — it belongs to
  the type, is edited with it and travels with it in exports.
- **D18** Health findings are rows of the newest run only — the screen filters and pages
  them; history is the run totals (30 days).
- **D19** No rate limit on `check` beyond the body cap and the 200-path probe cap — it is
  read-only, and DB-backed limits would write a row per keystroke pause.
