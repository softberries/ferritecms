+++
title = "Introduction"
description = "A WordPress-style CMS as one Rust binary, with every plugin in a WebAssembly sandbox."
+++

FerriteCMS is a content management system in the style of WordPress, shipped as a single
Rust binary. The web server, the REST API, the admin interface, the built-in themes and the
plugin runtime are compiled into it. Every plugin runs in a WebAssembly sandbox and can only use what its
manifest declares.

**Run it from your AI assistant.** Every site serves a Model Context Protocol (MCP)
endpoint at its address plus `/mcp`. Paste that address into Claude, ChatGPT, Claude Code,
Cursor or VS Code, sign in to your site once and approve what the assistant may do; then
ask it to write and publish posts, organise media, edit menus, fix redirects, moderate
comments or check the site's health. It acts as you: with your role, within the areas you
approved, and its changes are recorded in the audit log under the app's name. See
[AI assistants (MCP)](02-guides/08-ai-assistants.md).

FerriteCMS imports WordPress content (a WXR export file) but does not run WordPress PHP
plugins or themes.

> **Status:** early; current release 0.2.0 (signed binaries for Linux and macOS and a
> Docker image, see [Install](01-getting-started/01-install.md)). Expect changes before 1.0.

## Principles

- **Managed by AI, as you.** The MCP endpoint is built in and on by default. An assistant
  signs in through the site's own consent screen and works through the same permission
  checks and audit log as the admin; a few actions (installing extensions, managing users
  and tokens, backups) stay with a person.
- **One binary, one data folder.** A site on the default setup needs no other service: the
  database is SQLite in the data folder and media files sit next to it. Postgres and
  S3-compatible storage are optional.
- **Plugins are untrusted.** A plugin gets only the host functions its manifest declares.
  It has no SQL access, no file system and no network access unless declared. Every call
  into a plugin has a memory limit, a fuel limit and a deadline, and a failing plugin does
  not break the page.
- **Themes don't run code.** A theme is templates and assets. Templates get data from core
  and a fixed set of helpers; anything that needs logic belongs in a plugin.
- **Familiar to WordPress users.** Posts and pages, content types, categories and tags,
  menus, roles (admin, editor, author, contributor), hooks with priorities and a template
  hierarchy work the way a WordPress user expects.

## Two ways to run a site

Content, the admin, themes and plugins work the same in both. A few operations are for solo
or SQLite sites only: built-in HTTPS (ACME) is refused in cluster mode, where TLS ends at
the load balancer; `ferrite backup` refuses a Postgres site, which relies on the database's
own backups and `ferrite export`; and `ferrite rollback --restore-snapshot` needs SQLite.

| | Solo (default) | Cluster |
|---|---|---|
| Database | SQLite in `<data>/ferrite.db` | PostgreSQL |
| Media | the folder `<data>/media/` | an S3-compatible bucket |
| Nodes | exactly one | one or more, behind a load balancer |
| Coordination | inside the process | through Postgres |

Solo is what `ferrite init` sets up when you give it nothing else. A cluster needs a
Postgres database, shared media storage and the same site key on every node.

## Where to start

- [Getting started](01-getting-started/index.md): install the binary, create a site, learn
  the main ideas.
- [AI assistants (MCP)](02-guides/08-ai-assistants.md): connect an assistant and manage
  the site by asking.
- [Guides](02-guides/index.md): writing, media, content types, themes, plugins and imports.
