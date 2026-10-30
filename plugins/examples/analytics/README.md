# analytics

An example FerriteCMS plugin that counts page views without tracking anyone, and shows them
on a stats page. It runs in a WebAssembly sandbox and has two parts: a `request.served`
action (priority 10) that receives the site's page responses in batches, and one route,
`GET /_p/analytics/`, that renders the numbers as HTML with an inline SVG chart (no script).

## What it sees, and what it cannot count

Core tells the plugin, for every public page response: the path (percent-encoded, no query
string, at most 512 bytes), the status, the time (UTC), the host of the `Referer` header, a
coarse user-agent class (`browser`, `bot` or `other`) and whether the page cache answered.
There is no cookie, no IP address (not even a hash), no user-agent string, no user id and no
visitor id.

So the plugin counts **page views**, and it **cannot count unique visitors or sessions**:
nothing in an event identifies a visitor. That is deliberate.

## What it counts

For each event whose path is not excluded (see `exclude_paths`):

| Event | Counted as |
|---|---|
| `ua_class` `browser`, status 200 or 304 | a **view**: added to `views`, to the path's count and, if there is a foreign referrer host, to that host's count |
| `ua_class` `bot` (any status) | `bots` |
| `ua_class` `other` (no user agent, `curl`, …; any status) | `other` |
| any status from 200 to 599 | also one count in `status` by class: `2xx`, `3xx`, `4xx`, `5xx` |

A referrer equal to the site's own host (from the `site.url` option) is not a referral, and
neither is a missing referrer (a direct visit).

Core classes a user agent as a bot when it contains `bot`, `crawl`, `spider` or `slurp`, and as
a browser when it starts with `Mozilla/`. This is coarse: a bot that does not announce itself
counts as a browser.

The event cannot say what kind of page was answered or who asked, so these count as views
too, when a browser asks for them and the answer is 200: the RSS feed, `/sitemap.xml`,
`/robots.txt`, search results, and the pages that logged-in users (you, previewing) open.
Add the ones you do not want, for example `/feed,/sitemap.xml,/robots.txt`, to `exclude_paths`.

## Install

The plugin is a folder holding `ferrite-plugin.toml` and one `.wasm`. From the plugins release
asset (`ferrite-plugins-<version>.tar.gz`, which holds `analytics/`), or built from source:

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml -p analytics
mkdir analytics-plugin
cp plugins/examples/analytics/ferrite-plugin.toml analytics-plugin/
cp plugins/target/wasm32-wasip2/release/analytics.wasm analytics-plugin/
```

With the release asset, unpack it (`tar xzf ferrite-plugins-<version>.tar.gz`) and use its
`analytics/` folder where the commands below say `analytics-plugin`. Then, with `serve` stopped
if the site uses SQLite:

```sh
ferrite plugin check   --data /path/to/site --dir analytics-plugin   # prints the permission report
ferrite plugin install --data /path/to/site --dir analytics-plugin --enable
```

`install` shows the permission report and asks you to approve it (`--yes` approves without
asking; it is required when stdin is not a terminal). Or use the admin's Plugins screen, or the
API: inspect with `POST /api/v1/plugins/inspect`, then `POST /api/v1/plugins` with the report's
`fingerprint` as `approve`. A new plugin is installed disabled unless you pass `--enable`.

Counting starts when the plugin is loaded: events from before it was enabled are gone.

## Permissions

The report lists these, and the plugin asks for nothing else:

- `options.read = ["site.url"]` (low): read the site URL, to know the site's own host.
- `kv` (low): store its numbers in the plugin's own key/value store.
- `routes` (high, as every route is): answer `GET /_p/analytics/`, at most 30 requests a
  minute per visitor.
- hook `request.served` (medium): receive every page view of the site, as described above.

It has no network access, no mail and no access to content.

## The stats page and who may open it

Open `/_p/analytics/` on your site. Route requests carry the signed-in user's id and nothing
else (no role), so the plugin keeps its own list: the **Viewers** setting.

1. Sign in to the admin, then open `/_p/analytics/`. Signed out you get a 403 that says to sign
   in first.
2. A signed-in user who is not listed gets a 403 that shows **their own user id**.
3. Paste that id into **Viewers** on the plugin's page in the Plugins screen. Ids are separated
   by commas or new lines and compare exactly. An empty list means nobody, so a fresh install
   shows the stats to no one.
4. Open the page again.

The page shows, for the last 1, 7, 30 or 90 days (`?days=N`: the first `days` value is read as
a whole number, 0 counts as 1, anything above 90 as 90, and a missing value or one that is not
a whole number (`abc`, `-3`) as 30): page views, bot and other requests, the
response classes, a bar chart of views per day (empty days included), the top 20 paths and the
top 20 referrer hosts. Days are UTC days and the period ends **today**, by the wall clock of
the node that serves the page. Paths and hosts are HTML-escaped and shown cut to 200
characters.

The page decodes stored data inside the sandbox, which has a fixed budget of work per request.
The totals and the chart always cover the whole period. The path and referrer lists are built
from the newest day with data (always, however big its blob) and then up to three more days,
as long as their blobs add up to 150 000 bytes. A site whose days are full, with 500 paths and
200 referrers, gets four days of lists; a quiet site gets four too, because of the day limit.
The page says when the lists cover fewer days than the period. Choose a shorter period to
include every day in the lists.

## Settings

Set them on the plugin's page in the admin's Plugins screen, or with
`PATCH /api/v1/plugins/analytics/settings`.

| Setting | Type | Default | Effect |
|---|---|---|---|
| `viewers` | textarea | empty (nobody) | ids of the users who may open the stats page |
| `retention_days` | number | 90 | days of data kept, 1 to 366 (a value outside that is clamped, one that is not a number is 90) |
| `exclude_paths` | text | `/admin,/_p` | comma-separated path prefixes that are not counted |

`exclude_paths` entries must start with `/`; others are ignored. A prefix covers the path itself
and everything below it: `/admin` covers `/admin` and `/admin/users`, not `/administrator`. A
trailing `/` is ignored, `/` alone excludes everything, and an empty setting excludes nothing.
Paths are compared as received, percent-encoded. Core does not send events for `/admin` or
`/_p/`, so the default is a safety net.

## What is stored

One JSON blob per UTC day under the plugin's KV key `day:YYYY-MM-DD`; there is no other key:

```json
{"v":1,"views":120,"bots":14,"other":3,
 "status":{"2xx":131,"4xx":6},
 "paths":{"/":40,"/about/":12},
 "referrers":{"news.example.org":9}}
```

- The day is the event's own UTC time, so a batch that spans midnight updates two blobs. An
  event whose time cannot be read is skipped, and so is one dated more than a day ahead of
  today (a node whose clock runs fast): it must not push the history out. The plugin writes the
  counts before the maps, and the page relies on that to read older days cheaply.
- `paths` holds at most 500 keys and `referrers` at most 200. Past that, a key that is not
  already there is counted under `"(other)"`, which is not a path or a host.
- A path is stored cut to its first 200 bytes and a referrer host to 100 (core allows 512 and
  253). The page shows 200 characters anyway, paths that differ only after the cut share one
  count, and the length of a key a visitor can choose is bounded. Core sends the path
  percent-encoded, so nothing in it needs a JSON escape, and a blob is at most about 130 KB
  (500 paths of 200 bytes, 200 hosts of 100, with counts of many digits).
- **Retention** keeps the newest `retention_days` days, counted back from today (the wall
  clock): with 3, on 1 January the data for 30 and 31 December and 1 January is kept and 29
  December goes. An event for a day older than that is not stored. After each batch the
  plugin lists the day keys, oldest first, and deletes the expired ones. A call may make 100 KV
  operations; counting uses a handful, the rest go to deleting, so a large backlog (a lowered
  retention) clears over a few batches.
- A blob written by a newer version of the plugin (`"v"` above 1) is left alone, and that
  day's events are not counted. A blob that is not JSON is replaced.
- **Limits per batch.** A batch that touches more than three days counts only the newest
  three (a batch holds seconds of one node's events, so two at midnight is the normal most).
  It also decodes and re-encodes at most 150 000 bytes of stored blobs: the newest
  touched day always, an older one only if it fits, so a batch across midnight on two full
  days counts the newest and skips the older (logged, its events are lost). The 1 MiB a call
  may write is never the limit that binds; fuel is.
- A day for which the host answers "no blob" is checked with a listing before a first blob is
  written over it: the host answers the same for a missing key and for a read that failed,
  and writing a fresh blob over a day that exists would lose its counts. If the key is there,
  the day is left alone and logged.
- A batch whose events are not all of the shape core sends is dropped whole and logged; an
  event whose time cannot be read is only skipped.
- Retention follows the wall clock of the node that runs the job, so a node whose clock is
  days fast prunes that many of the oldest days early.

## The counts are approximate

Fine for a trend, not for billing. This is the same caveat as the `counter` example:

- **At-least-once delivery.** A batch is a `plugin.action` job. If the call fails after it
  wrote something, the job is retried and the events that were already counted are counted
  again.
- **Read-modify-write race.** A day's blob is read, changed and written back, which is not
  atomic. With more than one runner worker or several nodes, two batches handled at once can
  read the same blob, and one of them is lost.
- **Failed reads.** The host answers `None` for a missing key and for a store read that
  failed, and an empty list for a listing that failed. The plugin checks a missing blob with
  a listing before it writes a first one over the day, which catches a failed read. It cannot
  tell if the listing fails too, so a day whose read and listing both fail while the write
  succeeds is overwritten with the current batch alone. That needs two calls in a row to fail
  and a third to work; it is a known gap of the 0.1 `kv` interface.
- **Lost events.** Events buffered on a node are lost when it crashes (up to 10 seconds'
  worth), when its buffer is full (10 000 events), when a batch cannot be queued, or when the
  plugin is not loaded on that node at flush time.

An atomic increment (`kv.incr`) would remove the race; it needs a new plugin world version.

## Limits of this example

The plugin runs in world `ferrite:plugin/plugin@0.1.0`, which constrains it:

- **No unique visitors**, no sessions, no bounce rate, no countries, no devices: the event has
  none of these.
- **No query strings**: `?utm_source=…` is gone before the plugin sees the request, so there
  are no campaigns.
- **No custom events or goals, no outbound-link tracking** (those need script in the page).
- **No admin page.** Plugins cannot add screens, and a route sees only the user id, not the
  role. The stats page is a route with its own viewer list.
- **No real-time view.** Events are batched, so the numbers lag by about 10 seconds.
- **No export or reset.** The data is reachable only through the stats page. Delete it by
  removing the plugin with `--purge`.
- **A coarse bot filter**, as above, and 404 scanner noise is counted in `status` (not in
  `views`, `paths` or `referrers`).
- **Work per call is bounded, not free.** The counts are held in ordinary hash maps with a
  random seed, so crafted paths or referrer hosts cannot pick colliding keys. A day's maps are
  capped at 500 and 200 keys and cut to 200 and 100 bytes, and the stats page sums at most
  four days of them. Decoding a day costs fuel in proportion to its entries and bytes, which
  is why the lists cover only the newest days (above) and why a batch decodes at most 150 000
  bytes. At the default fuel limit of 50 million, the most expensive batches measured need 26
  to 30 million (1000 events with 508-byte paths and 253-byte hosts, spread over a midnight or
  over three days and counted onto a day that already holds a full blob, or only adding to
  its keys): about 1.7 to 1.9 times headroom. Core percent-encodes the path before it cuts it,
  so a `\` or `"` in a request path (which core accepts and answers 404) costs no more than
  any other byte; left raw, 1000 such 404s needed more than 80 million.

## Uninstall and data

`ferrite plugin remove analytics --data /path/to/site` (or the admin) removes the plugin; its
settings and the day blobs are kept for 30 days, so a reinstall in that time finds them, and
`--purge` deletes them at once. Nothing else is stored.

The plugin data belongs to the name `analytics`: install an update or a reinstall only from a
source you trust.
