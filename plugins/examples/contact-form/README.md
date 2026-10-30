# contact-form

An example FerriteCMS plugin: a contact form whose messages are mailed to the site's admins
and kept in an inbox page. It runs in a WebAssembly sandbox and answers three routes:

| Route | What it is |
|---|---|
| `GET /_p/contact-form/` | a standalone form page |
| `POST /_p/contact-form/submit` | where the form posts; a theme can post its own form here too |
| `GET /_p/contact-form/inbox` | the stored messages, for the users listed in the `viewers` setting |

The pages are plain server-rendered HTML with no script (the host serves plugin pages with
`Content-Security-Policy: sandbox allow-forms; form-action 'self'`).

## Install

The plugin is a folder holding `ferrite-plugin.toml` and one `.wasm`. From the plugins release
asset (`ferrite-plugins-<version>.tar.gz`, which holds `contact-form/`), or built from source:

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml -p contact-form
mkdir contact-form-plugin
cp plugins/examples/contact-form/ferrite-plugin.toml contact-form-plugin/
cp plugins/target/wasm32-wasip2/release/contact_form.wasm contact-form-plugin/
```

With the release asset, unpack it (`tar xzf ferrite-plugins-<version>.tar.gz`) and use its
`contact-form/` folder where the commands below say `contact-form-plugin`. Then, with `serve`
stopped if the site uses SQLite:

```sh
ferrite plugin check   --data /path/to/site --dir contact-form-plugin   # prints the permission report
ferrite plugin install --data /path/to/site --dir contact-form-plugin --enable
```

`install` shows the permission report and asks you to approve it (`--yes` approves without
asking; it is required when stdin is not a terminal). Or use the admin's Plugins screen, or the
API: inspect with `POST /api/v1/plugins/inspect`, then `POST /api/v1/plugins` with the report's
`fingerprint` as `approve`. A new plugin is installed disabled unless you pass `--enable`.

Mail needs `[mail] smtp_url` and `from` in `ferrite.toml`. Without them the form still works:
messages are stored, and each post logs that the mail was not sent.

## Permissions

The report lists these, and the plugin asks for nothing else:

- `routes` (high, as every route is): answer the three routes above. `/` and `/inbox` take the
  default limit of 10 requests a minute per visitor; `/submit` takes **10 posts an hour per
  visitor** and accepts posts from any origin (`cross_origin`, see below).
- `kv` (low): keep the inbox in the plugin's own key/value store.
- `mail = "admins"` (high): mail the site's active admins. The plugin never sees their
  addresses.
- `options.read = ["site.title"]` (low): name the site on the form page.

It has no network access and no access to content.

## Put the form on a page

**From a theme.** Paste this into a template, for example `page-contact.html` (the template of
the page with the slug `contact`):

```html
<form method="post" action="{{ plugin_route("contact-form", "/submit") }}">
  <input type="hidden" name="return" value="{{ request.path }}">
  <label for="cf-name">Name</label>
  <input id="cf-name" name="name" autocomplete="name" required>
  <label for="cf-email">E-mail</label>
  <input id="cf-email" name="email" type="email" autocomplete="email" required>
  <label for="cf-subject">Subject (optional)</label>
  <input id="cf-subject" name="subject">
  <label for="cf-message">Message</label>
  <textarea id="cf-message" name="message" required></textarea>
  <div style="position:absolute;left:-10000px" aria-hidden="true">
    <label for="cf-website">Leave this field empty</label>
    <input id="cf-website" name="website" tabindex="-1" autocomplete="off">
  </div>
  <button type="submit">Send</button>
</form>
```

`plugin_route` gives `/_p/contact-form/submit`, and `return` brings the visitor back to the page
the form was on. The `website` field is a trap for bots: people never see it, so a post that
fills it is dropped (see "Spam and abuse"). Keep it hidden with CSS, not with `type="hidden"`,
which a bot can tell apart. The page itself stays cacheable: the form holds nothing per
visitor.

**Without a theme change.** Link to `/_p/contact-form/`, the plugin's own form page. It is
sandboxed, so a browser posts its form with `Origin: null`. That is why `/submit` is
`cross_origin`: without it the host would refuse those posts (403). A `cross_origin` route is
always served anonymously, which costs nothing here (the form needs no user), and a site that
posts to it from elsewhere is no worse than the spam any contact form gets.

## What a visitor gets

| Post | Answer |
|---|---|
| valid | 200, a thank-you page with the `success_message` setting and a link back to `return` (else `/`) |
| the honeypot `website` filled | the same thank-you page; nothing is stored or sent |
| a field missing or out of its limits | 422, the form again, filled in, listing each problem by field (for example "Message is required."), with a link back |
| not `application/x-www-form-urlencoded` (a `charset` parameter is fine) | 415 |
| a body over 96 KiB | 413 (the largest valid form is about 68 KB; the host itself takes up to 1 MiB) |
| a body that does not decode (a bad `%` escape, not UTF-8, more than 100 fields) | 400 |
| neither stored nor mailed (storing off, or failed, and the mail refused) | 503, "could not be sent right now" |
| the 11th post within the hour from one visitor | 429 from the host, with `Retry-After` |

`return` is used only when it is a path on this site: it starts with `/`, not `//`, holds only
visible ASCII without `\`, and is at most 512 bytes (the rule the host applies to a redirect's
`Location`). `//evil.com`, `/\evil.com`, `javascript:…` and full URLs give the back link `/`.
It is checked as sent, never repaired.

Every value the plugin shows (what the visitor typed, the settings, the stored messages) is
HTML-escaped, although the sandbox would not run a script anyway.

## Fields

| Field | Rule |
|---|---|
| `name` | required, 1 to 100 characters |
| `email` | required, 3 to 254 characters, exactly one `@` with text on both sides, no whitespace or control character |
| `subject` | optional, up to 150 characters |
| `message` | required, 10 to 5000 characters |
| `website` | the honeypot: must be empty |
| `return` | optional: the page to go back to |

Lengths count characters, not bytes, after cleaning: CR and LF are removed from every field
except `message`; in `name` and `subject` any other control character (a tab, say) and the
Unicode line and paragraph separators become a space, and Unicode format characters (category
Cf: bidirectional overrides such as U+202E, the zero-width space, the soft hyphen) are
removed, so a name or subject cannot be made to display as something else. The zero-width
non-joiner and joiner (U+200C, U+200D) stay: Persian and Indic names and emoji sequences need
them, and they cannot reorder text. Each field is trimmed. The message keeps its line breaks,
as LF (CR LF and a lone CR become LF), its tabs and its format characters (it is only ever shown escaped), and loses any other control character.
Repeated keys count their first value; unknown fields are ignored.

## The mail

One mail per message to every active admin (in Bcc, at most 50), with the subject

```
[<site title>] <subject_prefix> <subject>
```

The host adds the `[<site title>] ` part (the `site.title` option, else the configured title)
and a footer `Sent by plugin contact-form`. Without a subject, the first 60 characters of the
message stand in. The plugin makes the subject one line (control characters and line
separators become spaces, runs of spaces one space) and cuts it to the 200 characters the host
allows a plugin, so nothing a visitor types can add a mail header. The body names the sender,
their e-mail, the subject, the page (`return`, else the `Referer`) and the time (UTC, to the
second), then the message.

- **There is no Reply-To.** The plugin interface cannot set one, so replying to the mail
  writes to the site's own `from` address. Copy the visitor's address from the body.
- **At most 10 mails per plugin per hour**, a limit of the host shared by every visitor. The
  11th message within an hour is stored but not mailed (logged). Spam that gets past the
  honeypot can use up that hour; the inbox still keeps those messages, as far as it holds
  them (the newest 100 within 900 KiB, see "What is stored"). A flood from many addresses can
  push earlier messages out of the inbox as well. With `store_messages` off, every post after
  the 10th mail in an hour gets the 503 page, since it is neither stored nor mailed.
- The host queues the mail and sends it afterwards. The plugin learns only that the mail was
  accepted, not that it arrived.

## The inbox and who may open it

Open `/_p/contact-form/inbox`. Route requests carry the signed-in user's id and nothing else
(no role), so the plugin keeps its own list: the **Viewers** setting.

1. Sign in to the admin, then open the inbox. Signed out you get a 403 that says to sign in
   first.
2. A signed-in user who is not listed gets a 403 that shows **their own user id**.
3. Paste that id into **Viewers** on the plugin's page in the Plugins screen. Ids are
   separated by commas or new lines and compare exactly. An empty list means nobody, so a fresh
   install shows the inbox to no one.
4. Open the page again.

The page lists 10 messages at a time, newest first (`?page=2` for the next ten; a number past
the last page shows the last one): the sender, their address, the time, the subject, the page
it came from and the message.

**Messages cannot be deleted one by one.** A delete button would be a signed-in post from the
plugin's sandboxed page, which browsers send with `Origin: null`; the host accepts such posts
only on `cross_origin` routes, and serves those anonymously, so the plugin could not tell who
asked. The oldest messages drop out as new ones come; `--purge` on removal deletes everything.

## Settings

Set them on the plugin's page in the admin's Plugins screen, or with
`PATCH /api/v1/plugins/contact-form/settings`.

| Setting | Type | Default | Effect |
|---|---|---|---|
| `viewers` | textarea | empty (nobody) | ids of the users who may open the inbox |
| `subject_prefix` | text | `[Contact]` | put before the subject of each mail; empty for none |
| `success_message` | textarea | empty: "Your message was sent. Thank you for getting in touch." | the thank-you text; line breaks are kept |
| `store_messages` | bool | true | keep messages in the inbox; off, they are only mailed, and a post whose mail is refused (no `[mail]`, the 10-an-hour limit) gets the 503 page |

## What is stored

One blob under the plugin's KV key `inbox`; there is no other key. It is JSON lines: a version
line, then one message per line, newest first:

```
{"v":1}
{"at":"2026-10-30T12:34:56Z","name":"Ann","email":"ann@example.org","subject":"Hello","page":"/contact/","message":"…"}
```

- It holds the latest **100** messages, and at most **900 KiB**: past either, the oldest go.
  A hundred messages of 5000 plain characters take about 660 KB, so ordinary messages always
  keep a hundred; the largest valid message (every character four bytes in UTF-8) takes about
  23 KB, and a blob of only those holds 40.
- A blob whose version line is not this plugin's (not JSON, another shape) is logged and
  started again with the next message. A blob written by a newer version (`"v"` above 1) is
  left alone: the message is mailed but not stored, and the inbox page says so. A stored line
  that is not a message is shown as unreadable and kept until it drops out. Lines written by
  hand are never decoded past a size bound: a version line over 1 KiB makes the blob not this
  plugin's, and a message line over 32 KiB (the plugin writes at most about 23 KB) is shown as
  unreadable, so a huge line cannot exhaust a call's fuel.
- The host answers "no blob" both for a missing key and for a read that failed. Before it
  writes a first blob the plugin checks with a listing that the key is really missing; if it
  is there, the message is not stored (logged), so a failed read never wipes the inbox. The
  mail still goes out and the visitor is thanked.
- A submit and another at the same moment (two nodes, say) can both read the inbox and write
  it back, and then one of the two messages is not stored. Both are mailed. An atomic
  `kv` operation would need a new plugin world version.
- The plugin data belongs to the name `contact-form`: install an update or a reinstall only
  from a source you trust.

## Spam and abuse

- **The honeypot**: a filled `website` gets the thank-you page and nothing is stored or
  mailed, whatever else the post holds, so a bot learns nothing.
- **10 posts an hour per visitor**, counted by the host per client address (IPv6 per /64)
  across nodes.
- **Header injection**: CR and LF never reach the mail subject (above), and the host refuses a
  subject with any control character as a second check.
- There is no CAPTCHA and no content filter: a person, or a bot that leaves the honeypot alone,
  gets through.

## Fuel

A plugin call is bounded in guest instructions (fuel; the site default is 50 million), and
reading and writing JSON is what costs. So a submit adds its line to the blob without decoding
the stored messages (it finds them by their line breaks), and the inbox page decodes only the
ten messages it shows. Measured:

| Call | Fuel |
|---|---|
| a submit onto a full blob (900 KiB of the largest messages), itself of the largest size | about 9 million |
| a submit onto 99 messages of 5000 plain characters | about 4 million |
| the inbox page of ten messages every character of which is `"` (an escape both in JSON and in HTML) | about 19 million |
| the inbox page of ten of the largest four-byte messages | about 16 million |

So the default leaves at least 2.6 times what the dearest call needs. The test suite runs
these worst cases at half the default limit.

## Limits of this example

The plugin runs in world `ferrite:plugin/plugin@0.1.0`, which constrains it:

- **No Reply-To** in mail, and mail only to the admins.
- **No admin page.** Plugins cannot add screens, and a route sees only the user id, not the
  role: the inbox is a route with its own viewer list.
- **No per-message delete** (above) and **no export**.
- **One form.** The fields are fixed; there are no custom fields, file uploads or several forms
  with their own recipients.
- **No shortcode.** A form cannot be put into post content; it lives in the theme or on the
  plugin's own page.

## Uninstall and data

`ferrite plugin remove contact-form --data /path/to/site` (or the admin) removes the plugin; its
settings and the inbox are kept for 30 days, so a reinstall in that time finds them, and
`--purge` deletes them at once. Nothing else is stored.
