# 18 — Outgoing webhooks

Status: **built in M6-6** (`docs/superpowers/m6-6-exit.md`; the R-M6-6-n execution rulings there refine §4 and §8 as noted below) · Date: 2026-11-02

An administrator registers HTTPS endpoints that FerriteCMS calls when something happens on
the site: a post is published, media is uploaded, a comment arrives, a user is created, an
import finishes. Each call is a signed JSON `POST` in the
[Standard Webhooks](https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md)
format, delivered at least once through the job queue, retried on a fixed schedule,
recorded in a delivery log and sent only to public addresses. Typical receivers: a
static-site or deploy hook (Netlify, Vercel, Cloudflare Pages), a CDN purge, a chat
notification, Zapier / n8n / Make, a search indexer.

Roadmap: M6-6 ("Webhooks on content, media, comment, user and release events: HMAC-signed
bodies, retries through the job queue, the plugin `http` import's SSRF checks, a delivery
log"; "Admin screen and API (`/api/v1/webhooks`), test delivery, secrets write-only").
Migration: `0016_webhooks`.

## 1. Goals and non-goals

Goals:

1. **No event is lost.** An event is recorded in the same transaction as the write that
   caused it (the job queue is the outbox, as it already is for plugin `on-action` hooks,
   `ferrite_core::actions`). A write that rolls back sends nothing.
2. **Standard format.** Headers, signature and secret format follow Standard Webhooks, so
   receivers verify with the official libraries (`standardwebhooks` for JS, Python, Go,
   Rust, …) or Svix's.
3. **Safe egress.** Deliveries use the plugin `http` import's checked resolver
   (`ferrite-http/src/plugins/egress.rs`: every resolved address must be public, the
   connected peer is checked again, no proxy, no redirects followed), so an administrator
   cannot point a webhook at the cloud metadata service or the database.
4. **Visible.** A delivery log per endpoint (attempts, status codes, timings, a response
   excerpt), a test delivery, redelivery, and an auto-disable with a notice when an
   endpoint keeps failing.
5. **Both profiles.** Solo and cluster; on N nodes each attempt is made by exactly one
   node (the job lease), and endpoint changes apply at once on every node (no per-node
   cache).

Non-goals (this milestone): inbound webhooks; webhooks defined by plugins or themes (plugins
keep `on-action` hooks, 05 §4); per-endpoint ordering; asymmetric (`v1a`, ed25519)
signatures; custom request headers per endpoint; a payload template language; events for
every write in the system (§3.4 lists what is left out); MCP tools for webhooks (§9).

## 2. Concepts

- **Endpoint** — a URL, a name, the event types it subscribes to (patterns, §3.1), an
  optional content-type filter, a signing secret, an enabled flag and health fields. At
  most 20 per site.
- **Event** — one thing that happened: a type (`content.published`), a time, an actor and
  a thin `data` object (§3.2). Stored once in `webhook_events`, whatever the number of
  endpoints.
- **Delivery** — one event for one endpoint: `pending` → `delivered` | `failed` |
  `skipped`. Each try is an **attempt** (`webhook_attempts`). A redelivery is a new
  delivery of the same event (`redelivery_of` names the original).
- **Message id** — the Standard Webhooks `webhook-id` is the **event id**: it is the same
  on every attempt and every redelivery, so a receiver that dedupes by it never applies an
  event twice. (One event sent to two endpoints carries the same id to both.)

## 3. Events

### 3.1 Catalogue (payload version 1)

| Type | Fired by (core, in the write's `Tx`) | `data` |
|---|---|---|
| `content.created` | `ContentService::create` (also `content_create` over MCP) | content (§3.2) |
| `content.updated` | `ContentService::update`, revision restore | content |
| `content.published` | every transition to `published`: publish (`transition_item`), scheduled publish (`publish_one`); not an upload (media, below) | content |
| `content.unpublished` | unpublish (published → draft) | content |
| `content.scheduled` | schedule | content + `scheduled_at` |
| `content.submitted` | submit for review (→ `pending`) | content |
| `content.trashed` | trash | content |
| `content.restored` | restore from trash | content |
| `content.deleted` | permanent delete (`delete_in`) | content as it was |
| `media.uploaded` | `MediaService::record_upload` | media |
| `media.updated` | `MediaService::update` (title, alt) | media |
| `media.trashed`, `media.restored`, `media.deleted` | `MediaService::trash` / `restore` / `delete` | media |
| `comment.created` | `CommentService::submit` (any resulting status) | comment |
| `comment.moderated` | `CommentService::moderate` (status changed) | comment + `previous_status` |
| `comment.deleted` | `CommentService::delete` | comment as it was |
| `user.created` | `UserService::create`, the operator `user create` (the shared helper that enqueues `user.created` actions, `users.rs`) | user |
| `user.updated` | `UserService::update` (display name, role, status, e-mail) | user + `changed` (field names) |
| `user.deleted` | `UserService::delete` | user as it was + `reassigned_to` |
| `import.finished` | `ImportRun::finish` (`import/mod.rs`, WordPress and Markdown, CLI and job) | `{run, source, state, stats}` |
| `webhook.ping` | `POST /webhooks/{id}/test` only; never matched by patterns | `{endpoint, message}` |
| `release.*` | **reserved for M6-3** (releases: `release.scheduled`, `release.applied`, `release.failed`); M6-3 adds them to the catalogue and emits them through §4.1 | M6-3 decides |

Media items are content items of kind `media` (06 §6); the content emitter skips kind
`media`, and `MediaService` emits the `media.*` types itself, so an upload (create +
publish in one `Tx`) is one `media.uploaded`, not `content.created` + `content.published`.

Subscription patterns: an exact type, `<group>.*` (`content.*`, `media.*`, `comment.*`,
`user.*`, `import.*`, later `release.*`) or `*`. At most 50 per endpoint; unknown exact
types and unknown groups are `422`. `*` and `<group>.*` include types added later (§3.3).
An optional `content_types` list (content type names, at most 50) narrows `content.*`
events to those types; it has no effect on other groups.

Wave-B parts add their own types to the catalogue through the same entry point (§4.1);
M6-3 (releases, approvals), M6-4 (content health) and M6-5 (change sets) name theirs in
their designs.

### 3.2 Payload

The body (UTF-8 JSON, `Content-Type: application/json`):

```json
{
  "id": "0193f1d2-7c3a-7b40-9e1a-2c0d5f6e8a11",
  "type": "content.published",
  "timestamp": "2026-11-02T10:15:30.123456Z",
  "version": 1,
  "site": { "url": "https://example.com" },
  "data": {
    "id": "0193f1d2-…",
    "type": "post",
    "status": "published",
    "title": "Hello",
    "slug": "hello",
    "path": "/2026/11/hello/",
    "url": "https://example.com/2026/11/hello/",
    "author": "0193…",
    "parent": null,
    "version": 7,
    "published_at": "2026-11-02T10:15:30.000000Z",
    "modified_at": "2026-11-02T10:15:30.000000Z",
    "actor": "0193…"
  }
}
```

- `id` is the event id (= `webhook-id`), `timestamp` the time of the write (RFC 3339 UTC,
  6 fractional digits, as everywhere in the API), `version` the payload version of the
  endpoint (§3.3), `site.url` the configured `site.url` without a trailing slash.
- **Thin payloads** (Standard Webhooks "thin vs full"): ids plus the fields receivers
  route on. No content body, fields, excerpts or SEO; a receiver that needs them calls
  `GET /api/v1/content/{id}` with a token, which is audited and scoped. Titles are cut at
  300 characters.
- `data` per group:
  - content: `id, type, status, title, slug, path, url, author, parent, version,
    published_at, modified_at, actor` (+ `scheduled_at` for `content.scheduled`). `path`
    is the permalink path computed in the write's `Tx` with the pinned permalink rules;
    `url` is `site.url` + `path`, added at delivery. `url` is `null` and `path` is the
    would-be path for items that are not public (drafts, pending, trashed). `actor` is the
    acting user id, `null` for system writes (scheduled publishing). After M6-1 lands,
    content and terms carry `locale` and `translation_group` (whichever of M6-1 and M6-6
    merges second adds them; additive, so still version 1).
  - media: `id, title, alt, mime, bytes, width, height, original_name, actor`. No file
    URL (the variant keys belong to the media module; receivers call `GET /media/{id}`).
  - comment: `id, item: {id, type}, parent, status, author_name, user, body, created_at,
    actor` (+ `previous_status`). `body` is the plain text cut at 2 000 characters. Never
    `author_email`, `author_url` or `ip_hash`.
  - user: `id, login, display_name, role, status, actor` (+ `changed`, `reassigned_to`).
    Never the e-mail address, hashes, TOTP or sessions.
- The whole body is at most 64 KiB (`MAX_WEBHOOK_BODY`); with the cuts above it never
  comes near that. `data` is stored in `webhook_events.data` at emit time; the body is
  rendered from it at each attempt deterministically (struct field order, `serde_json`),
  so every attempt sends the same bytes unless `site.url` changed in between.

### 3.3 Versioning

`version` is the payload schema version, chosen per endpoint (`payload_version`, only `1`
exists). **Additive** changes (a new field, a new event type, a new `data` member) keep
the version: receivers must ignore unknown fields and types (stated in the guide). A
**breaking** change (a field removed, renamed or re-typed) gets version 2; endpoints stay
on their version until an administrator moves them, and both are rendered while any
endpoint uses the old one.

### 3.4 Left out on purpose

Terms, menus, redirects, settings, themes, plugins, content types, sessions, tokens and
logins: they are configuration, not content events, and most are already in the audit
log. `request.served` (one per page view) and autosaves are far too chatty. Bulk side
effects are not expanded into per-item events: an import emits one `import.finished`
(its batches write through `ImportBatch`, which fires no action hooks either), a user
delete that reassigns content emits one `user.deleted` with `reassigned_to`, a permalink
structure change emits nothing per item. `seed demo` emits nothing.

## 4. Delivery

### 4.1 The outbox (enqueue in the write's transaction)

`ferrite_core::webhooks::emit(repos: &dyn Repos, event: NewWebhookEvent) -> Result<u32,
CoreError>`, called inside the write's `Tx` next to the existing `events().append` /
`actions::enqueue` calls:

1. `repos.webhooks().active_for_emit()` reads the enabled endpoints whose `install_id`
   equals the database's current `schema_meta.install_id` (one statement, a sub-select;
   the table holds at most 20 rows). Patterns and the content-type filter are matched in
   Rust (`webhooks::matches`). No match: return `0`, nothing written. This read is the
   only cost a write pays on a site without webhooks.
2. One `webhook_events` row (uuid v7 id, type, data, created_at).
3. Per matching endpoint: one `webhook_deliveries` row (`pending`, attempts 0) and one
   job `webhook.deliver`, payload `{"v":1,"delivery":"<uuid>","attempt":1}`, unique key
   `webhook.deliver:<delivery>:1`, priority `-1` (below core jobs, like plugin actions),
   `max_attempts` 3 (these are runner retries for infrastructure failures, not delivery
   attempts).

The rows commit or roll back with the write (D2). The job's `NOTIFY` (Postgres) wakes a
runner on any node; SQLite's runner polls.

### 4.2 One attempt

The `webhook.deliver` handler (`crates/ferrite/src/jobs/webhooks.rs`, registered by
`serve`) calls `WebhookService::deliver(delivery, attempt, now)`:

1. Read the delivery, its event and endpoint. Missing delivery (endpoint deleted, log
   pruned): done. `delivery.status != pending` or `delivery.attempts + 1 != attempt` (an
   attempt already recorded, a job re-run after a crash): done, nothing sent.
2. Endpoint disabled, or its `install_id` differs from the database's (a copy, §6.4):
   mark the delivery `skipped` (`last_error` "endpoint disabled" / "paused: copied
   database"), done.
3. Take one of the node's webhook slots (`max(1, cluster.workers - 1)`, a `Semaphore`
   like `PluginActionHandler`'s); none free: `JobError::Busy` (attempt refunded).
4. Open the secret(s) (§5.2); a secret that does not open (a database moved to another
   site key): mark `skipped` ("secret cannot be read: rotate it"), done.
5. Render the body, sign it (§5.1), send through `WebhookTransport::send` (§6.1).
6. In one `Tx`: insert the `webhook_attempts` row; update the delivery (attempts,
   `last_status_code`, `last_error`, `next_attempt_at`, status) and the endpoint's health
   (§4.4); if another attempt is due, enqueue `webhook.deliver` for `attempt + 1` with
   `run_at` = now + delay (§4.3) and unique key `webhook.deliver:<delivery>:<n>`.
7. Return `Ok` (the job is `done`); a store error anywhere is `JobError::Retry`, so the
   runner tries the same attempt again (its unique key and the step-1 check keep it single).

As built: step 6 re-reads the endpoint with `get_endpoint_for_update` (`FOR UPDATE` on
Postgres) so a concurrent admin edit and the health update never overwrite each other
(R-M6-6-4); a test delivery (`webhook.ping`) gets one attempt and no retries (R-M6-6-5);
the binary takes the slot before calling `deliver` (skips take one too, briefly).

The HTTP request happens between reading and recording, so a node that dies after the
receiver answered and before step 6 commits sends that attempt again after the job's lease
expires: **at least once** (D3). Receivers dedupe by `webhook-id`.

### 4.3 Success, failure and the retry schedule

- **Success**: any `2xx`. The delivery is `delivered`.
- **Failure**: any other status, a timeout, a refused address, a TLS or connection error.
  `3xx` is a failure ("redirect not followed"; the spec: update the URL instead).
- **`410 Gone`**: the receiver asked to stop. The delivery is `failed` and the endpoint is
  disabled at once with reason `gone` (§4.4).
- **Schedule** (Standard Webhooks' recommended one, first 8 attempts): attempt 1 at once,
  then after 5 s, 5 min, 30 min, 2 h, 5 h, 10 h and 14 h; each delay plus a random 0–10 %
  jitter. Attempt 8 is the last, about 31 h 35 min after the event; its failure makes the
  delivery `failed`.
- **`Retry-After`** on `429` or `503` (seconds or an HTTP date): the next delay is
  `max(scheduled delay, Retry-After)`, capped at 24 h.
- Ordering is **not guaranteed**: attempts of different deliveries interleave, retries
  overtake, and nodes run in parallel. Receivers order by `timestamp` and the content
  `version` (the guide says so).

### 4.4 Endpoint health and auto-disable

Columns on the endpoint: `last_success_at`, `last_failure_at`, `failing_since` (set by the
first failed attempt after a success or since creation; cleared by any `2xx`) and
`consecutive_failures` (failed attempts since the last success; reset by a `2xx`).

**Auto-disable** (in the step-6 `Tx`): when `failing_since` is more than **5 days** ago and
`consecutive_failures >= 10`, the endpoint is disabled with `disabled_reason = failing`
(D8). `410` disables at once with `gone`. A manual disable is `manual`. Re-enabling (API
or admin) clears `disabled_reason`, `failing_since` and `consecutive_failures` and stamps
the current `install_id`.

**Notice** to administrators when an endpoint is disabled automatically (`failing` or
`gone`):

- an audit row `webhook.auto_disabled` (no actor; details: endpoint id, name, URL host,
  reason, `consecutive_failures`) in the same `Tx`;
- a mail to the active administrators (`users().list_active_admins`, as the plugin
  `notify-admins` does) when `[mail]` is configured, sent after the commit through the
  node's `MailQueue`, best effort (a failed send is logged, never retried);
- the Webhooks screen (§8) shows the endpoint as disabled with the reason and a banner;
  `ferrite doctor`'s `webhooks.endpoints` check warns (§10.3).

Deliveries still pending when an endpoint is disabled are `skipped` when their next job
runs. After re-enabling, an administrator redelivers what was missed (`redeliver` since a
time, §7).

### 4.5 Bounds

| What | Value |
|---|---|
| Request timeout (connect, TLS, send, response headers and the read below) | 15 s (the spec recommends 15–30 s; D6) |
| Request body | at most 64 KiB (§3.2) |
| Response read | at most 4 KiB, then the connection is dropped; the first 1 KiB, lossy UTF-8 with control characters replaced, is stored as `response_excerpt` |
| Response headers kept | none, except `Retry-After` read for scheduling |
| Concurrent deliveries per node | `max(1, cluster.workers - 1)` slots; a full slot is `Busy` (attempt refunded) |
| Handler timeout | 30 s (`JobHandler::timeout`) |
| Endpoints per site | 20 |
| Test deliveries | 10 per user per minute (`AuthService` limits) |
| Redeliver since | at most 1 000 deliveries per call |
| Log retention | 30 days (events and, by cascade, their deliveries and attempts) |

### 4.6 Retention and the prune job

`webhooks.prune`, a periodic job every hour (like the built-in prunes, `ferrite-cluster/src/
builtin.rs`, but registered from `crates/ferrite/src/jobs.rs`'s `core_periodics` next to
its handler): deletes `webhook_events` older than 30 days that have no `pending` delivery,
in batches of `MAX_PRUNE_BATCH` within a 240 s budget; deliveries and attempts go by
cascade. `webhook.deliver` jobs themselves are pruned by `jobs.prune` (7 days done, 30
dead).

## 5. Signing

### 5.1 Headers and signature (Standard Webhooks)

| Header | Value |
|---|---|
| `webhook-id` | the event id (uuid) |
| `webhook-timestamp` | the attempt's time, Unix seconds |
| `webhook-signature` | `v1,<base64(HMAC-SHA256(key, "<id>.<timestamp>.<body>"))>`; during a rotation two signatures separated by a space (current first) |
| `content-type` | `application/json` |
| `user-agent` | `ferrite/<version> webhooks` |

`key` is the base64-decoded part of the secret after `whsec_`. The signed `<body>` is the
exact bytes sent. Base64 is standard with padding (as the spec's libraries write it).
Known-answer test (from the spec repository's library tests): secret
`whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw`, id `msg_p5jXN8AQM9LWM0D4loKWxJek`, timestamp
`1614265330`, body `{"test": 2432232314}` → `v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=`.
Implemented in `ferrite_core::webhooks::sign` with the workspace's `hmac`, `sha2` and
`base64` (already dependencies of `ferrite-core`); no new crate.

The receiver checks the signature and that `webhook-timestamp` is within its tolerance
(the libraries default to 5 minutes); the guide shows it in JavaScript and Python with the
official `standardwebhooks` packages and by hand.

### 5.2 Secrets

- Format `whsec_` + standard base64 of 32 random bytes (`getrandom`). An administrator may
  bring their own on create (`whsec_` + base64 of 24 to 64 bytes), e.g. when moving a
  receiver from another system.
- **Write-only**: shown once in the create and rotate answers (`201` / `200`), never
  again; no read endpoint, no audit details, no log line, no export in clear.
- Stored **sealed** (`ferrite_core::auth::seal`, XChaCha20-Poly1305 under the site key's
  seal subkey) with purpose `webhook-secret:<endpoint id>`, so a sealed value copied to
  another row does not open (the same scheme as plugin secret settings, `ext_settings`).
- **Rotation**: `POST /webhooks/{id}/secret` makes a new secret, keeps the old one sealed
  as `previous_secret_sealed` until `previous_secret_expires_at` (now + 24 h) and signs
  with both during that window (the spec's multi-signature header). A second rotation
  within the window replaces the previous one (only two signatures ever).

## 6. Security

### 6.1 Egress (SSRF)

`WebhookTransport` (trait in `ferrite-core`, as `ClientMetadataFetcher` is for OAuth) is
implemented by `ferrite_http::webhook_send::WebhookClient` over a new
`egress::webhook_client()` builder that reuses `egress::build`'s `CheckedResolver`:

- **Addresses**: the name is resolved once per attempt; the whole answer is refused when any
  address is not public (`egress::is_public_ip`: loopback, private, link-local and cloud
  metadata, CGNAT, multicast, reserved, documentation, the IPv4-mapped forms); reqwest
  connects only to the checked addresses (no second lookup, so DNS rebinding cannot swap
  them) and the connected peer is checked again (`remote_addr`). An IP-literal host must
  pass the same rule.
- **No redirects** (`redirect::Policy::none()`; a `3xx` is a failure), no proxy
  (environment ignored), no cookie store, no `Referer`, no decompression.
- **TLS**: rustls with the platform verifier and the `webpki-roots` fallback, as egress.
- Errors stored and shown name the class only ("address not allowed", "timed out", "TLS
  error", "connection failed", "status 503"), never a resolved address; refusals are
  logged at `warn` with the endpoint id and URL host, once a minute per endpoint.

URL rules (checked in core on create and update, `422` field `url`): absolute, `https`,
a host, no user information, no fragment, at most 2 048 bytes, any port. A query string is
allowed (deploy hooks carry their token there).

**Local targets** for development: `[webhooks] local_targets = true` in `ferrite.toml`
(env `FERRITE_WEBHOOKS__LOCAL_TARGETS`) allows `http` URLs and loopback and private
addresses (link-local / metadata ranges stay refused). It is an operator switch, not an
admin setting, so a site administrator cannot open internal targets by themselves (D9).
`config check` prints `webhooks=local` when on and `ferrite doctor` warns.

### 6.2 Who may do what

New capability `manage_webhooks` (administrators only) and a new scope area `webhooks`
(`webhooks:read`, `webhooks:write`) appended after `system` in `Area::ALL`, so existing
scope bits keep their positions (D4). A webhook can send site data to any public URL, so it
is not covered by `settings:write` or `system:write`.

| Call | Needs |
|---|---|
| list, get, deliveries, a delivery, the event catalogue | `manage_webhooks` + `webhooks:read` (`require_reader`) |
| create, update, enable/disable, delete, rotate, test, redeliver | `manage_webhooks` + `webhooks:write` (`require`) |

`webhooks:*` are not in `Scopes::OAUTH` (an AI client cannot be granted them) and not in
`role_scopes`' every-role reads. Capabilities are checked in `WebhookService`, never in
handlers (CLAUDE.md conventions).

### 6.3 Data leaving the site

Only what §3.2 lists, to URLs an administrator entered. The endpoint URL itself can carry a
credential (a deploy hook token): it is returned in full to `webhooks:read` callers (the
admin must be able to see and edit it) but **audit details and logs hold only its host**.
Response excerpts come from the receiver and are shown escaped (plain text) in the admin.

### 6.4 Copies of a site (staging must not call production hooks)

Every endpoint stores the `install_id` it was created or last enabled under. `restore`,
`import-site` and `db copy` give the copy a new install id (M4-1), so on a copy every
endpoint is **paused**: emits skip it, pending deliveries are `skipped`, and the admin shows
"Paused: this database was copied or restored — enable it to send from this site". Enabling
stamps the current install id. This also covers a SQLite backup restored onto another host
(D10).

### 6.5 Audit

`webhook.create`, `webhook.update` (changed field names; `url` as host only),
`webhook.enable`, `webhook.disable`, `webhook.delete`, `webhook.rotate_secret`,
`webhook.test`, `webhook.redeliver` (count), `webhook.auto_disabled` (system). Never a
secret or a full URL.

## 7. Data model and migration `0016_webhooks`

Both backends, same version and name, `-- ferrite:phase expand`, with `.down.sql` (drops the
four tables). SQLite types per 02 §5 (16-byte `BLOB` ids, `INTEGER` micros, JSON `TEXT`
with `json_valid`); Postgres `uuid`, `timestamptz`, `jsonb`, `boolean`, text keys
`COLLATE "C"`.

```sql
CREATE TABLE webhook_endpoints (
    id                          uuid PRIMARY KEY,
    name                        text NOT NULL,            -- 1..=100 chars, no control chars
    url                         text NOT NULL,            -- §6.1 rules, <= 2048 bytes
    events                      jsonb NOT NULL,           -- array of patterns, 1..=50
    content_types               jsonb,                    -- array of type names or NULL
    enabled                     boolean NOT NULL,
    disabled_reason             text CHECK (disabled_reason IN ('manual','failing','gone')),
    disabled_at                 timestamptz,
    install_id                  text NOT NULL,
    secret_sealed               text NOT NULL,
    previous_secret_sealed      text,
    previous_secret_expires_at  timestamptz,
    payload_version             integer NOT NULL DEFAULT 1,
    failing_since               timestamptz,
    consecutive_failures        integer NOT NULL DEFAULT 0,
    last_success_at             timestamptz,
    last_failure_at             timestamptz,
    created_by                  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at                  timestamptz NOT NULL,
    updated_at                  timestamptz NOT NULL
);

CREATE TABLE webhook_events (
    id          uuid PRIMARY KEY,
    type        text NOT NULL,
    data        jsonb NOT NULL,
    created_at  timestamptz NOT NULL
);
CREATE INDEX webhook_events_created_idx ON webhook_events (created_at);

CREATE TABLE webhook_deliveries (
    id               uuid PRIMARY KEY,
    endpoint_id      uuid NOT NULL REFERENCES webhook_endpoints (id) ON DELETE CASCADE,
    event_id         uuid NOT NULL REFERENCES webhook_events (id) ON DELETE CASCADE,
    status           text NOT NULL CHECK (status IN ('pending','delivered','failed','skipped')),
    attempts         integer NOT NULL DEFAULT 0,
    next_attempt_at  timestamptz,
    last_status_code integer,
    last_error       text,
    redelivery_of    uuid,                                 -- no FK: the original may be pruned
    created_at       timestamptz NOT NULL,
    finished_at      timestamptz
);
CREATE INDEX webhook_deliveries_endpoint_idx ON webhook_deliveries (endpoint_id, created_at DESC, id DESC);
CREATE INDEX webhook_deliveries_event_idx ON webhook_deliveries (event_id);

CREATE TABLE webhook_attempts (
    delivery_id      uuid NOT NULL REFERENCES webhook_deliveries (id) ON DELETE CASCADE,
    attempt          integer NOT NULL,
    started_at       timestamptz NOT NULL,
    duration_ms      integer NOT NULL,
    status_code      integer,
    error            text,                                 -- class only (§6.1)
    response_excerpt text,                                 -- <= 1 KiB
    PRIMARY KEY (delivery_id, attempt)
);
```

Constraint names follow the existing `<table>_pkey` / `<table>_<col>_fk` convention (the
contract tests read them).

**Repository** `WebhookRepo` (`Repos::webhooks()`), on both backends, every text bound
NUL-checked: `insert_endpoint`, `get_endpoint`, `list_endpoints`, `count_endpoints`,
`update_endpoint` (whole row, `NotFound` when missing), `delete_endpoint`,
`active_for_emit()` (enabled and `install_id` = `schema_meta.install_id`),
`insert_event`, `get_event`, `insert_delivery`, `get_delivery`, `list_deliveries(endpoint,
query: status filter, keyset cursor, limit)`, `record_attempt(delivery, attempt row,
delivery update)` (conditional on `status = 'pending' AND attempts = attempt - 1`,
returning `false` otherwise), `set_delivery_status`, `deliveries_since(endpoint, since,
statuses, limit)`, `delivery_counts(endpoint, since)` (by status, for the list's health
column), `prune_events(before, limit) -> u64`.

**Export catalogue** (`ferrite-store/src/catalogue.rs`): `webhook_endpoints` is
`Export::Rows` (after `users`; the sealed secrets travel as stored, as plugin secrets do;
on the target they are paused by §6.4 and, under another site key, need a rotation);
`webhook_events`, `webhook_deliveries` and `webhook_attempts` are `Export::Skip` (a log;
added to `skipped_tables_are_the_p2_list`). `backup` (a SQLite file snapshot) holds
everything; a restore pauses the endpoints (§6.4), and old `webhook.deliver` jobs in the
restored queue find their endpoint paused and skip.

## 8. REST API (`/api/v1/webhooks`, tag `webhooks`)

DTOs in `crates/ferrite-http/src/api/dto/webhooks.rs` with `utoipa` annotations; handlers
in `api/webhooks.rs` only translate; `WebhookService` on `ApiState`. Writes need the
session's CSRF token or a bearer token as everywhere. `openapi.json` regenerated.

| Method and path | Body / query | Answer |
|---|---|---|
| `GET /webhooks` | — | `200 {items: [WebhookDto]}` (≤ 20, by name) |
| `POST /webhooks` | `WebhookInput {name, url, events[], content_types?, enabled? = true, secret?}` | `201 CreatedWebhookDto {webhook, secret}` + `Location`; `422` fields `name`, `url`, `events`, `content_types`, `secret`; `422` "at most 20 webhooks" |
| `GET /webhooks/{id}` | — | `200 WebhookDto` |
| `PATCH /webhooks/{id}` | `WebhookPatch {name?, url?, events?, content_types? (null clears), enabled?}` | `200 WebhookDto` (last write wins, as redirects; `enabled: true` re-enables, §4.4) |
| `DELETE /webhooks/{id}` | — | `204` (deliveries and attempts go with it; pending jobs then find nothing) |
| `POST /webhooks/{id}/secret` | — | `200 {secret, previous_valid_until}` |
| `POST /webhooks/{id}/test` | — | `202 {delivery}` (`webhook.ping`, job priority 10; works on a disabled endpoint so it can be checked before enabling, never on a paused copy: `422`); `429` past 10/min |
| `GET /webhooks/{id}/deliveries` | `status?`, `limit?` (1..=200, 50), `cursor?` | `200 {items: [DeliveryDto], next_cursor}` newest first |
| `GET /webhooks/{id}/deliveries/{delivery}` | — | `200 DeliveryDetailDto {delivery, event: {id, type, timestamp, data}, attempts: [AttemptDto]}` |
| `POST /webhooks/{id}/deliveries/{delivery}/redeliver` | — | `202 {delivery}` (a new delivery of the same event); as built both redelivers are `422` on a disabled or paused endpoint (enable it first, R-M6-6-6) |
| `POST /webhooks/{id}/redeliver` | `{since, statuses? = ["failed","skipped"]}` | `202 {count}` (≤ 1 000, oldest first; `pending` in `statuses` is `422` field `statuses`) |
| `GET /webhooks/events` | — | `200 {version: 1, types: [{type, group, description}]}` |

`WebhookDto`: `id, name, url, events, content_types, enabled, state` (`active` |
`failing` | `disabled` | `paused`), `disabled_reason, disabled_at, failing_since,
consecutive_failures, last_success_at, last_failure_at, payload_version,
secret_rotating_until, created_at, updated_at, counts_24h {delivered, failed, pending,
skipped}`. Never a secret. `DeliveryDto`: `id, event_id, event_type, status, attempts,
last_status_code, last_error, next_attempt_at, created_at, finished_at, redelivery_of`.
`AttemptDto`: `attempt, started_at, duration_ms, status_code, error, response_excerpt`.

## 9. MCP, plugins and themes

- **MCP: no tools** (D11). A tool that creates a webhook would let an assistant (or a
  prompt injection in content it reads) send site data to any URL; reading the log adds
  little. `webhooks:*` stay out of `Scopes::OAUTH`. The `audit_list` and `jobs_list`
  tools already show webhook audit rows (host only) and `webhook.deliver` jobs (delivery
  ids only). The MCP tool count (56) and `mcp_reference.rs` are unchanged.
- **Plugins**: no WIT change, no new world. Plugins keep `on-action` hooks; they cannot
  read or write webhook rows (no import reaches them).
- **Themes**: nothing.

## 10. Admin, configuration and operations

### 10.1 Admin screen (`/admin/webhooks`)

Administration section, after Plugins, `manage_webhooks`, chord `g w` (free in 11 §6),
icon `WebhookIcon` (lucide), palette entries "Go to Webhooks" and "New webhook". Built with
the 11-admin-ui kit (`PageHeader`, `DataTable`, `StatusBadge`, `EmptyState`,
`CopyOnce`, `FormField`, `RelativeTime`), i18n namespace `webhooks.json`, tokens only, 44 px
touch targets, axe clean.

- **List** (`/webhooks`): name, URL host (full URL in a tooltip), events summary ("All
  events", "Content (3)", …), state badge (Active / Failing since … / Disabled: failing |
  gone | manual / Paused: copied), last delivery, 24 h counts; row opens the detail.
  Empty state explains webhooks with a "New webhook" action. A banner at the top when any
  endpoint is auto-disabled or paused.
- **New webhook** (dialog): name, URL (https hint; the local-targets note when the server
  says so is out of scope, the API's `422` text is shown), event picker (groups with
  "all in group", "All events", fed by `GET /webhooks/events`), content-type filter
  (shown when a `content.*` type is chosen), optional own secret behind "Advanced".
  Create shows the secret once with `CopyOnce` and a "Send test" button.
- **Detail** (`/webhooks/$id`): header with state, actions Send test, Rotate secret
  (confirm; new secret shown once; "old secret valid until …"), Enable / Disable, Delete
  (confirm, names the endpoint); a settings form (name, URL, events, content types);
  the deliveries table (status filter, event type, created, attempts, last code, next
  attempt), paging with "Load more"; a row opens a sheet with the event JSON (read-only
  code view), the attempts (code, duration, error, response excerpt as text) and
  Redeliver. "Redeliver failed since…" (date-time picker) when the endpoint has failed or
  skipped deliveries.
- Test delivery: the button posts, then polls the delivery (1 s, up to 20 s) and shows the
  outcome inline (delivered with code and time, or the error class).
- Account → API tokens: the scope picker gains the `webhooks` area (`SCOPE_AREAS` in
  `admin/src/api/tokens.ts`); System → Audit gets texts for the `webhook.*` actions
  (`system/audit-text.ts`).

### 10.2 Configuration

`[webhooks]` in `ferrite.toml` (`ferrite-config`): `local_targets` (bool, default `false`,
§6.1). Nothing else is configurable: schedule, timeouts, retention and limits are constants
in `ferrite_core::webhooks` (D12).

### 10.3 `ferrite doctor`

`webhooks.endpoints` (database group, after `db.identity`): `ok` "N endpoints, N active"
(or "none"); `warn` when any endpoint is auto-disabled (`failing`, `gone`), has been failing
for more than 24 h, or is paused on this install (names and hosts only); `warn` when
`local_targets` is on; `skip` without a database. Read-only.

### 10.4 Cluster and solo

- Endpoint rows are read in every emitting `Tx` and at every attempt; there is no cache, so
  a change on one node applies on all at once. Nothing is published on the events bus and
  no page-cache key changes (webhooks do not affect rendered pages).
- Each attempt is one job; the job lease (04 §6) gives it to exactly one node. Slots are
  per node.
- The site key must be the same on every node (cluster mode already requires
  `FERRITE_AUTH__SECRET_KEY`), so any node opens the secrets.
- Mixed versions during a rolling upgrade: a node older than M6-6 has no handler for
  `webhook.deliver` and does not claim the kind (the runner claims only the kinds it
  handles); its writes emit no events (its code has no `emit` calls), which is the one gap
  of a rolling upgrade, documented in the upgrade runbook.
- Solo SQLite: the same code; the runner polls. `ferrite user create` and the CLI imports
  enqueue events with `serve` stopped; the next `serve` delivers them.

## 11. Testing

- Store contract tests on both backends (`ferrite-store-tests/src/webhooks.rs`):
  `migration_0016_roundtrip`, endpoint CRUD and cascade, `active_for_emit` honours
  `enabled` and `install_id`, `record_attempt` single-use, keyset paging, counts, prune
  keeps events with pending deliveries, `catalogue_matches_live_schema`.
- Core (`ferrite-core/tests/webhooks.rs`, in-memory SQLite): emit inside a rolled-back
  `Tx` leaves nothing; each catalogue type is emitted by its write path (one test per
  group); media upload emits one `media.uploaded` only; pattern and content-type matching;
  the signature known-answer test and the two-signature rotation header; secrets never in
  DTOs, audit or errors; the schedule, jitter bounds, `Retry-After`, `410`, auto-disable at
  5 days and 10 failures; paused copies; capability and scope refusals (editor, `content:
  write` token, OAuth scopes).
- HTTP (`ferrite-http/tests/webhooks/` and `tests/api/webhooks.rs`): hostile targets —
  `127.0.0.1`, `[::1]`, `169.254.169.254`, `10.0.0.1`, a name resolving to loopback (test
  resolver), a name with one public and one private address, a redirect to an internal
  host, a response that never ends, a 10 MiB response, a slow-loris response past 15 s —
  all refused or bounded; a local receiver (with `local_targets`) verifies the signature
  with an independent implementation in the test; the API end to end and
  `openapi_snapshot_is_current`.
- Binary (`crates/ferrite/tests/webhooks.rs`): `serve` with a local receiver: publish →
  delivered; receiver down → retries scheduled (test seam shortens the schedule); two
  cluster nodes on Postgres deliver each attempt once (`FERRITE_TEST_PG_URL`).
- Admin: Vitest for the model, list, dialog, detail and delivery sheet; Playwright
  `e2e/webhooks.spec.ts` with a local receiver (`e2e/support/webhook-sink.ts`, the server
  started with `FERRITE_WEBHOOKS__LOCAL_TARGETS=true`): create, copy the secret once, test
  delivery, publish a post and see the delivery, rotate, disable, delete; axe on both
  screens.

## 12. Out of scope (later)

Inbound webhooks; per-endpoint custom headers; `v1a` ed25519 signatures; payload version 2;
filtering by more than content type (term, author); per-endpoint rate limits; FIFO ordering
per endpoint; a dashboard card; events for terms, menus, settings, themes and plugins;
plugin-defined events (a WIT change, new world version); a webhook-signing helper in the
guest SDK; delivering from a node-local queue without the database.

## 13. Decisions

- **D1** Thin payloads (ids + routing fields, no bodies) — keeps drafts and fields out of
  third-party logs and the body small; receivers fetch through the audited, scoped API.
- **D2** The job queue is the outbox: event, delivery rows and the first job are written in
  the write's `Tx` — the pattern plugin actions already use, so no event is lost and
  nothing is sent for a rolled-back write.
- **D3** At least once, no ordering — exactly-once over HTTP is impossible; the stable
  `webhook-id` (the event id) lets receivers dedupe.
- **D4** New `manage_webhooks` capability and `webhooks:*` scopes, appended after `system`
  — a webhook exports data, so neither settings nor system rights should imply it, and
  appending keeps existing scope bits.
- **D5** One job per attempt, with the schedule kept by the handler (`run_at`), not the
  runner's `job_backoff` — `job_backoff` is a 10·2ⁿ s curve capped at 1 h, too short for
  a receiver that is down for hours, and the runner's attempts stay for infrastructure
  retries.
- **D6** 15 s timeout, 4 KiB response read — the low end of the spec's 15–30 s, because each
  delivery holds a job worker.
- **D7** Standard Webhooks' first 8 retry steps (≈ 31.5 h) — long enough to ride out a
  weekend-morning outage, short enough for the log's 30-day retention.
- **D8** Auto-disable after 5 days of failures **and** at least 10 failed attempts — "five
  days" alone would disable a rarely-called endpoint on its first failure after a long quiet
  spell.
- **D9** Local and private targets only by the operator's `[webhooks] local_targets`, not an
  admin setting — the site administrator is not trusted with the network the server sits
  in.
- **D10** Endpoints pause on a copied or restored database (install id) — staging copies
  must not fire production deploy hooks; re-enabling is one click.
- **D11** No MCP tools — a webhook is an exfiltration channel and a target for prompt
  injection; admins use the screen or the API.
- **D12** Fixed constants instead of settings for schedule, timeout and retention — fewer
  knobs to test on two backends; revisit when a user asks.
- **D13** `webhook-id` is the event id, shared by all endpoints and redeliveries — what the
  spec means by "unchanged across retries"; per-endpoint ids would defeat dedupe of a
  redelivery.
- **D14** Endpoint URLs are shown to admins but only their host goes to the audit and logs —
  deploy-hook URLs carry tokens.
- **D15** Comment payloads carry the text (cut at 2 000 characters) but never the e-mail,
  URL or address hash — moderation notifications are the main comment use case; the
  personal data stays.
- **D16** Imports emit one `import.finished`, not per-item events — a 10 000-post import
  would otherwise enqueue 10 000 deliveries per endpoint.
