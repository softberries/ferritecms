+++
description = "Write a FerriteCMS plugin in Rust: the WebAssembly world, the manifest, capabilities, limits, the SDK and the build."
+++

# Plugin authoring

A plugin is a WebAssembly component plus a manifest, `ferrite-plugin.toml`. FerriteCMS
runs every plugin in a sandbox (wasmtime with the Component Model): the plugin can call
only the host functions its manifest declares, every call has a memory, fuel and time
limit, and a fresh instance serves each call, so nothing carries over from one request to
the next. A plugin that fails or runs out of time is skipped and the page renders without
it.

Plugins are written in Rust with the `ferrite-sdk` crate and built for the
`wasm32-wasip2` target. `ferrite-sdk` is not published on crates.io: depend on it by path
or git from a clone of the repository.

## The world

The interface is the WIT package `ferrite:plugin@0.1.0` in the repository's `wit/`
folder, world `ferrite:plugin/plugin@0.1.0`. A plugin exports three functions:

- `apply-filter(hook, value, request)`: change a value (head tags, HTML, text) and
  return it;
- `on-action(hook, event)`: react to an event;
- `handle-request(request)`: answer one of the plugin's own routes.

The host offers these imports; a plugin gets each one only with the capability listed:

| Import | Functions | Needs |
|---|---|---|
| `log` | `log` | always granted |
| `settings` | `get` (the plugin's own settings) | always granted |
| `content` | `get-post`, `get-post-by-slug`, `list-posts` | `content.read` |
| `meta` | `get`, `set` | `content.meta.read` / `content.meta.write` |
| `options` | `get` | `options.read` |
| `kv` | `get`, `set`, `delete`, `list-keys` | `kv` |
| `http` | `fetch` | `http` |
| `mail` | `notify-admins` | `mail = "admins"` |

## Hooks

| Hook | Kind | Needs |
|---|---|---|
| `render.head` | filter: the page's head tags | `render.head = true` |
| `content.body` | filter: an item's body HTML | `render.html = "sanitized"` or `"raw"` |
| `render.footer` | filter: HTML before `</body>` | `render.html = "sanitized"` or `"raw"` |
| `content.excerpt` | filter: an excerpt's text | `render.text = true` |
| `post.saved` | action | nothing |
| `post.published` | action | nothing |
| `user.created` | action | nothing |
| `request.served` | action, batched page requests | nothing |

Each hook in `[hooks]` has a priority; lower numbers run first.

## The manifest

```toml
[plugin]
name        = "my-plugin"                    # 2 to 48 characters of a-z, 0-9 and -
version     = "0.1.0"                        # semver
publisher   = "example.com"
world       = "ferrite:plugin/plugin@0.1.0"
description = "What it does"                 # optional
license     = "MIT"                          # optional
homepage    = "https://example.com/plugin"   # optional, http:// or https://

[capabilities]
content.read       = ["post", "page"]        # content types; ["*"] for all
content.meta.read  = ["seo.*"]               # meta keys; a trailing .* matches a prefix
content.meta.write = ["seo.*"]
options.read       = ["site.title", "site.url"]
kv                 = true                    # the plugin's own key-value storage
http               = ["api.example.com"]     # plain host names, HTTPS only
render.head        = true
render.text        = true
render.html        = "off"                   # "off", "sanitized" or "raw"
routes             = true                    # answer under /_p/my-plugin/
mail               = "admins"                # mail the site's admins, no one else

[hooks]
"render.head"    = 10
"post.published" = 50

[routes]
"/submit" = { methods = ["POST"], rate_limit = "10/min" }

[settings]
api_key  = { type = "secret", label = "API key" }
show_tag = { type = "bool", default = true, label = "Show the tag" }

[limits]
memory = "16MB"
fuel   = 20000000
```

Only `[plugin]` with `name`, `version`, `publisher` and `world` is required. Unknown keys
are refused, and a hook whose capability is not declared is refused too.

- **`http`** takes host names only: no scheme, port, path or wildcard, and no bare IP
  address or `localhost`. Requests go over HTTPS to port 443 only, and a host name that
  resolves to any address that is not public (private, loopback, link-local and the
  like) is refused.
- **`[routes]`** keys start with `/` and are served at `/_p/<name><path>`. `methods`
  defaults to `["GET"]`, `rate_limit` (`N/s`, `N/min` or `N/h`) to `10/min` per client
  address, and `cross_origin = true` lets other sites post to the route (such a request
reaches the plugin anonymously). Responses are
  never cached and are served with a sandboxing `Content-Security-Policy`.
- **`[settings]`** types are those of theme settings plus `secret`, which the admin never
  shows back. The plugin reads its settings through the `settings` import.
- **`[limits]`** may lower the site's defaults freely. Asking for more needs the admin's
  approval at install, up to 256 MiB of memory and 10 times `plugins.default_fuel`.

## Limits on every call

The site's defaults are in `[plugins]` of `ferrite.toml`
([Configuration](../04-operations/01-configuration.md)):

| Limit | Default |
|---|---|
| Memory | `default_memory = "32MB"` |
| Fuel (WebAssembly instructions) | `default_fuel = 50000000` |
| Time for a filter while a page renders | `render_deadline_ms = 200` |
| Time for an action | `action_deadline_ms = 5000` |
| Time for a route | 2 s (not configurable) |
| All filters of one page together | `render_budget_ms = 1000` |
| Calls running at once on a node | `max_concurrent_calls`, default the larger of 8 and 4 × CPUs |

Within one call: 100 units for `content`, `meta`, `options` and `settings` calls (a
`meta.get` takes two), 100 key-value operations moving at most 1 MiB, 1 MiB written
through `meta.set`, 3 HTTP requests (5 s each, 1 MiB out, 2 MiB in) and one mail. Past
a budget the import returns an error, and the plugin decides what to do. A plugin's
key-value storage holds at most 10 MiB in 10,000 keys.

A failed call (a trap, running out of fuel or time, invalid output) is counted. After
`disable_after_failures` failures (20) within `failure_window` (10 minutes) the plugin is
disabled on every node, with the reason shown in the admin. A call skipped because the
node was busy or the page's budget was spent is not counted.

## A minimal plugin

This is the `hello` example from `plugins/examples/hello/`. `Cargo.toml`:

```toml
[package]
name = "hello"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
ferrite-sdk = { path = "../../../crates/ferrite-sdk" }
```

`src/lib.rs`:

```rust
use ferrite_sdk::{Ctx, Head, Level, Result};

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    ferrite_sdk::log(Level::Debug, "hello: render.head");
    head.meta("generator", "ferrite-hello");
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
```

`ferrite-plugin.toml`:

```toml
[plugin]
name      = "hello"
version   = "0.1.0"
publisher = "ferritecms.dev"
world     = "ferrite:plugin/plugin@0.1.0"

[capabilities]
render.head = true

[hooks]
"render.head" = 10
```

`export_plugin!` maps hook names to plain functions. It always takes `filters:` (which
may be empty), then optionally `actions:` and `routes:`:

```rust
ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
    actions: { "post.published" => on_publish },
    routes: { "/" => page },
}
```

A filter function takes the value and a `&Ctx` and returns the same type: `Head` for
`render.head`, `String` for HTML and text hooks. `Ctx` gives the request
(`ctx.request()`), the current item (`ctx.current_post()`), key-value storage
(`ctx.kv()`), HTTP (`ctx.http()`) and settings (`ctx.setting(key)`). `Head` adds tags
with `meta`, `og`, `link`, `link_typed`, `json_ld` and `title`, and asks what is already
there with `has_meta_name`, `has_meta_property` and `meta_property`.

Helper modules: `form::parse_urlencoded`, `html::escape`, `options::get`,
`settings::{text, bool, number}`, `time::unix_now`, `mail::notify_admins`,
`viewers::allow` (is this user listed in a setting) and `route::html` (an HTML route
response). The three shipped plugins, `seo`, `analytics` and `contact-form` in
`plugins/examples/`, use them all.

## Build and install

From a clone of the repository, build the component:

```sh
cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml -p hello
```

The result is `plugins/target/wasm32-wasip2/release/hello.wasm`. The `--target` flag is
all it takes: no `cargo component` is needed. Put the manifest and the `.wasm` in one
folder, then check it against a site; this prints the permission report and stores
nothing:

```sh
mkdir hello-plugin
cp plugins/examples/hello/ferrite-plugin.toml plugins/target/wasm32-wasip2/release/hello.wasm hello-plugin/
ferrite plugin check --data ./mysite --dir ./hello-plugin
```

Then install and enable it (with SQLite, `serve` stopped):

```sh
ferrite plugin install --data ./mysite --dir ./hello-plugin --enable
```

Every page now has `<meta name="generator" content="ferrite-hello">` in its head. See
[Plugins](../02-guides/05-plugins.md) for the permission report, settings and removal.
