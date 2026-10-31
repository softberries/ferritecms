+++
description = "The FerriteCMS REST API: signing in, CSRF, API tokens and scopes, errors and the OpenAPI document."
+++

# REST API

Everything the admin does goes through the REST API under `/api/v1`, so a script or
another front end can do the same. Requests and responses are JSON. The examples use a
site at `http://localhost:8080`.

## The OpenAPI document

The full list of endpoints, with their inputs and outputs, is an OpenAPI 3.1 document
served without signing in:

```sh
curl -s http://localhost:8080/api/v1/openapi.json
```

`GET /api/v1/meta` (also open) answers `{"api":"v1","version":…,"site_url":…}`.

## Two ways to authenticate

- **A session**, as the admin uses: sign in with a login and password, then send the
  cookie with each request, plus a CSRF token on every request that changes something.
- **An API token**, for scripts: `Authorization: Bearer fer_…`. No cookie and no CSRF
  token. A request with an `Authorization` header never falls back to the cookie.

### Sign in with a session

```sh
curl -s -c cookies.txt \
  -H 'Origin: http://localhost:8080' \
  -H 'Content-Type: application/json' \
  -d '{"login":"me","password":"…"}' \
  http://localhost:8080/api/v1/auth/login
```

The answer is `{"status":"session","user":{…},"csrf_token":"…"}` and sets the session
cookie: `ferrite_session` on an `http://` site, `__Host-ferrite_session` (with `Secure`)
on an `https://` one. A session ends after 7 days without use and after 30 days in any
case. An account with two-factor sign-in answers `{"status":"totp_required",
"challenge":…}` instead; finish with `POST /api/v1/auth/login/totp`.

The login request needs an `Origin` header equal to the site's origin. Without it the
answer is `403` with the problem type `/api/v1/problems/csrf`.

### CSRF

With a session, every request other than `GET` and `HEAD` needs both:

- `X-CSRF-Token: <csrf_token>` from the login answer (also returned by `GET
  /api/v1/auth/me`);
- an `Origin` header equal to the site's origin (or, without one, a same-origin
  `Referer`).

Otherwise it is refused with `403 csrf` before the body is read. Bearer requests need
neither. The API sends no CORS headers: a browser app on another origin uses tokens.

## API tokens and scopes

Create a token for a user with `POST /api/v1/users/{id}/tokens` (from a session, so with
the CSRF token):

```sh
curl -s -b cookies.txt \
  -H 'Origin: http://localhost:8080' \
  -H "X-CSRF-Token: $CSRF" \
  -H 'Content-Type: application/json' \
  -d '{"name":"read-only","scopes":["content:read"]}' \
  http://localhost:8080/api/v1/users/$USER_ID/tokens
```

Here `$CSRF` is the login answer's `csrf_token` and `$USER_ID` its `user.id`. The answer
holds `token` (`fer_` and 43 more characters), shown only this once; the site
stores only its hash. `expires_at` (optional) sets an end. A user can hold at most 50
tokens. Tokens are also managed in the admin under **Account**. Use one as `$TOKEN`:

```sh
curl -s -H "Authorization: Bearer $TOKEN" 'http://localhost:8080/api/v1/content/post?limit=2'
```

A scope is `<area>:read` or `<area>:write`; `write` includes `read`. The areas are
`content`, `media`, `terms`, `menus`, `comments`, `users`, `settings`, `redirects`,
`extensions`, `import` and `system`. A token can hold only scopes its creator holds, and
the role still applies: an admin's `content:read` token can list drafts but cannot
create or publish (`403` with the detail `missing content:write`), and a contributor's
token cannot publish whatever its scopes. A session holds every scope.

Tokens are independent of the password: changing or resetting it keeps them. Disabling
or deleting the user removes them.

## Roles

| Can | admin | editor | author | contributor |
|---|---|---|---|---|
| Use the admin | yes | yes | yes | yes |
| Create and edit own content | yes | yes | yes | while draft or pending |
| Publish own content | yes | yes | yes | no |
| Edit and publish others' content | yes | yes | no | no |
| Upload media | yes | yes | yes | no |
| Manage terms, menus and comments | yes | yes | no | no |
| Manage users, settings, themes, plugins, import, redirects and the system | yes | no | no | no |

## Errors

Errors are `application/problem+json` (RFC 9457) with `type`
(`/api/v1/problems/<slug>`), `title`, `status`, `detail` and, for invalid input,
`errors` (`[{field, detail}]`):

| Status | Slug | When |
|---|---|---|
| 401 | `unauthenticated` | no valid session or token |
| 403 | `forbidden`, `csrf` | the role or scope does not allow it; a failed CSRF or origin check |
| 404 | `not-found` | unknown, or not readable by you |
| 412 | `precondition-failed` | the item changed since you read it (`current` holds it) |
| 422 | `validation` | invalid input |
| 429 | `rate-limited` | too many attempts; see `Retry-After` |

## Conventions

- Ids are UUID strings; times are RFC 3339 in UTC.
- Lists take `?limit=` (1 to 200, default 50) and `?cursor=`, and answer
  `{items, next_cursor}`.
- Content carries a `version`. Updates send `If-Match: "<version>"`; without it the
  answer is `428`, with a stale one `412`.
- JSON bodies need `Content-Type: application/json` and are at most 4 MiB.

## Rate limits

Sign-in attempts are limited to 10 a minute per client address and 5 a minute per login
name; two-factor codes to 5 a minute per user; unknown or expired tokens to 10 a minute
per address. The counts are kept in the database, so they hold across cluster nodes.

Behind a reverse proxy, list the proxy in `server.trusted_proxies`
([Configuration](../04-operations/01-configuration.md)). Otherwise every request seems
to come from the proxy, and 10 failed sign-ins a minute from anyone block everyone.
