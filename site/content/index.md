+++
title = "Introduction"
description = "A WordPress-style CMS as one Rust binary, with every plugin in a WebAssembly sandbox."
+++

FerriteCMS is a content management system in the style of WordPress, shipped as a single
Rust binary. The web server, the REST API, the admin interface, the built-in themes and the
plugin runtime are compiled into it. Every plugin runs in a WebAssembly sandbox and can only use what its
manifest declares.

FerriteCMS imports WordPress content (a WXR export file) but does not run WordPress PHP
plugins or themes.

> **Status:** in development. No version has been released yet: there are no release
> binaries, no published Docker image and no hosted copy of these docs. Building from
> source is the way to run it today ([Install](01-getting-started/01-install.md)).

## Principles

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

Every feature works in both profiles.

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
- [Guides](02-guides/index.md): writing, media, content types, themes, plugins and imports.
