+++
title = "Webhooks"
description = "Call your own URLs when content, media, comments, users or imports change: signed requests for deploy hooks, CDN purges and chat notices, retried until they arrive."
+++

# Webhooks

A webhook is a URL of yours that FerriteCMS calls when something happens on the site: a
post is published, an image is uploaded, a comment arrives, a user is created, an import
finishes. Typical receivers are a static-site deploy hook (Netlify, Vercel, Cloudflare
Pages), a CDN purge, a chat notification, Zapier, n8n or Make, or a search indexer.

Every call is a signed JSON `POST` in the
[Standard Webhooks](https://www.standardwebhooks.com/) format, so receivers can check it
with the official `standardwebhooks` libraries. A call that fails is retried for about a
day and a half, and every attempt is in the webhook's delivery log.

Webhooks are for administrators: the screen and the API need the `manage_webhooks`
capability, which only the Administrator role has.

## Add a webhook

In the admin, open **Webhooks** (Administration, or `g` then `w`) and choose **New
webhook**:

1. **Name**: for you, such as "Deploy to Netlify".
2. **URL**: an `https://` address. A token in the query string is fine (deploy hooks carry
   one); the audit log keeps only the host.
3. **Events**: all events, whole groups (all content events, all media events, …) or
   single events. A whole group also gets the events added to it in later versions.
4. **Content types** (shown when you chose content events): limit content events to some
   types, such as posts only. None checked means every type.

Create shows the **signing secret** once. Copy it into your receiver now: it is not shown
again. **Send test** sends a `webhook.ping` and shows whether it arrived.

You can have up to 20 webhooks.

## What is sent

| Event | When |
|---|---|
| `content.created`, `content.updated` | an item is created or saved (also a restored revision) |
| `content.published`, `content.unpublished` | an item is published (also when its scheduled time comes), or taken back to draft |
| `content.scheduled`, `content.submitted` | an item is scheduled, or submitted for review |
| `content.trashed`, `content.restored`, `content.deleted` | trashed, restored, deleted for good |
| `media.uploaded`, `media.updated` | a file is uploaded; its title or alt text changes |
| `media.trashed`, `media.restored`, `media.deleted` | as for content |
| `comment.created`, `comment.moderated`, `comment.deleted` | a comment is submitted, its status changes, it is deleted |
| `user.created`, `user.updated`, `user.deleted` | an account is created, changed or deleted |
| `import.finished` | a WordPress or Markdown import run finishes |
| `webhook.ping` | **Send test** only |

Not sent: changes to terms, menus, redirects, settings, themes and plugins (they are in the
audit log), page views, autosaves, the single items of an import (it sends one
`import.finished`) and `seed demo`.

A request looks like this (ids shortened, signature made up):

```http
POST /build-hook HTTP/1.1
content-type: application/json
user-agent: ferrite/0.2.0 webhooks
webhook-id: 0193f1d2-7c3a-7b40-9e1a-2c0d5f6e8a11
webhook-timestamp: 1793614530
webhook-signature: v1,K5tH3mq8QZ0p2u6pRrN6d1o3J8l2yZbqk4wqk6wB1nE=

{"data":{"actor":"0193…","author":"0193…","id":"0193…","modified_at":"2026-11-02T10:15:30.000000Z","parent":null,"path":"/2026/11/hello/","published_at":"2026-11-02T10:15:30.000000Z","slug":"hello","status":"published","title":"Hello","type":"post","url":"https://example.com/2026/11/hello/","version":7},"id":"0193f1d2-7c3a-7b40-9e1a-2c0d5f6e8a11","site":{"url":"https://example.com"},"timestamp":"2026-11-02T10:15:30.123456Z","type":"content.published","version":1}
```

The payload is **thin**: ids and the fields a receiver routes on (type, status, title,
slug, path and URL), never the body, custom fields, SEO fields, e-mail addresses or
passwords. A receiver that needs more calls the [REST API](../03-developers/03-rest-api.md)
with a token, for example `GET /api/v1/content/{id}`. Comment events carry the comment
text (at most 2 000 characters) but never the commenter's e-mail address or URL. `url` is
`null` for items that are not public (drafts, trashed items).

Receivers must ignore fields and event types they do not know: new ones are added without
changing `version`.

## Check the signature

`webhook-signature` is `v1,` and the base64 HMAC-SHA256 of `<webhook-id>.<webhook-timestamp>.<body>`
with the secret's key (the part after `whsec_`, base64-decoded). Check it against the exact
bytes you received, and refuse a `webhook-timestamp` more than a few minutes old.

With the official library, in JavaScript (`npm install standardwebhooks`):

```js
import { Webhook } from "standardwebhooks";

const wh = new Webhook(process.env.WEBHOOK_SECRET); // "whsec_…"
// headers: webhook-id, webhook-timestamp, webhook-signature; body: the raw text.
const event = wh.verify(rawBody, headers); // throws when the signature is wrong
```

In Python (`pip install standardwebhooks`):

```python
from standardwebhooks.webhooks import Webhook

wh = Webhook(os.environ["WEBHOOK_SECRET"])  # "whsec_…"
event = wh.verify(raw_body, headers)  # raises when the signature is wrong
```

By hand, in Node.js without a library:

```js
import { createHmac, timingSafeEqual } from "node:crypto";

function verify(secret, headers, rawBody) {
  const key = Buffer.from(secret.replace(/^whsec_/, ""), "base64");
  const signed = `${headers["webhook-id"]}.${headers["webhook-timestamp"]}.${rawBody}`;
  const want = Buffer.from(createHmac("sha256", key).update(signed).digest("base64"));
  return headers["webhook-signature"].split(" ").some((s) => {
    const got = Buffer.from(s.replace(/^v1,/, ""));
    return got.length === want.length && timingSafeEqual(got, want);
  });
}
```

Or in Python with the standard library:

```python
import base64, hashlib, hmac

def verify(secret, headers, raw_body: bytes) -> bool:
    key = base64.b64decode(secret.removeprefix("whsec_"))
    signed = f"{headers['webhook-id']}.{headers['webhook-timestamp']}.".encode() + raw_body
    want = base64.b64encode(hmac.new(key, signed, hashlib.sha256).digest()).decode()
    return any(
        hmac.compare_digest(s.removeprefix("v1,"), want)
        for s in headers["webhook-signature"].split(" ")
    )
```

**Rotate secret** on the webhook's page makes a new secret (shown once). For 24 hours every
request carries two signatures, one with each secret, so you can update the receiver
without missing a delivery.

## Retries, order and duplicates

Any `2xx` answer is a success. Anything else is a failure: another status (a redirect is
not followed: update the URL instead), a timeout after 15 seconds, a connection or TLS
error. A failed delivery is tried again after 5 seconds, 5 minutes, 30 minutes, 2, 5, 10
and 14 hours, each with a little random delay added: 8 attempts over about 31.5 hours.
When the receiver answers `429` or `503` with `Retry-After`, the next attempt waits at least
that long (at most a day).

Deliveries arrive **at least once** and **in no particular order**. The `webhook-id` is the
same on every attempt and redelivery of an event: keep the ids you have handled and skip a
repeat. To order events, use `timestamp`, and for content the item's `version`.

## When a receiver keeps failing

- A receiver that answers **`410 Gone`** has asked to stop: the webhook is disabled at
  once.
- A receiver that has failed for **more than 5 days, with at least 10 failed attempts in a
  row**, is disabled automatically.

Either way the active administrators get an e-mail (when the site can send mail), the
Webhooks screen shows a banner and `ferrite doctor` warns. Fix the receiver, choose
**Enable**, then **Redeliver failed since…** to send what was missed. Each delivery in the
log can also be redelivered on its own.

The log keeps every delivery and attempt for 30 days.

## Copies of a site do not call your hooks

A copy made with `ferrite restore`, `ferrite import-site` or `ferrite db copy` (a staging
site, say) has its webhooks **paused**: nothing is sent until an administrator opens each
webhook on the copy and enables it. A staging copy never fires the production deploy hook
by accident.

## Testing against a local receiver

Webhooks go only to public `https` addresses: an address on the server's own network,
`localhost` or the cloud metadata service is refused. For development, the operator can
allow `http` and loopback or private addresses in `ferrite.toml`:

```toml
[webhooks]
local_targets = true
```

(or `FERRITE_WEBHOOKS__LOCAL_TARGETS=true`). Link-local and cloud metadata addresses stay
refused. `ferrite config check` then prints `webhooks=local`, and `ferrite doctor` warns:
leave it off on a production site.

## Recipes

**Rebuild a static site.** Point a webhook at the build or deploy hook URL your static host
gives you (Netlify, Vercel and Cloudflare Pages all offer one in the project's settings),
with the `content.published`, `content.unpublished`, `content.trashed` and
`content.deleted` events. A deploy hook is a URL that starts a build when it receives a
`POST`; it does not read the body, so it does not check the signature either: keep its
URL private.

**Purge a CDN.** A small receiver (a serverless function) verifies the signature, then
asks the CDN to purge `data.url` for `content.updated` and `content.published`, and the
archive pages your theme shows.

**Notify a chat.** A receiver turns `comment.created` (with `data.status` `pending`) into a
message to your moderation channel, with a link to `/admin/comments`.

## Through the API

Everything on the screen is in the REST API under `/api/v1/webhooks`, with the
`webhooks:read` and `webhooks:write` token scopes (a token with `system:write` or
`settings:write` alone is not enough). See the [REST API](../03-developers/03-rest-api.md).
AI assistants cannot manage webhooks: the MCP endpoint has no webhook tools, and the scopes
cannot be granted to an assistant.
