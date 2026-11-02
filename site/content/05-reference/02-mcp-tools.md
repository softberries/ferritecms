+++
title = "MCP tools"
description = "Every tool the FerriteCMS MCP server offers an AI assistant: what it does, the scope it needs and whether it reads or writes."
+++

# MCP tools

These are the tools an AI assistant sees when it connects to a site's `/mcp` endpoint:
56 of them, in 13 areas. To connect an assistant, see
[AI assistants (MCP)](../02-guides/08-ai-assistants.md).

How to read the tables:

- **Scope** is the permission a tool needs. A tool runs as the person who connected the
  assistant, so it can do only what that person's role allows *and* what the connection
  was approved for (the areas on the consent screen, or a personal token's scopes). A
  `write` scope includes `read`. A call without the permission comes back as an error that
  names what is missing.
- **Access** says whether a tool only reads or also changes the site. Writing tools are
  annotated for the client: the ones that remove or replace something (trash, unpublish,
  delete, moderating a comment, replacing a menu or a redirect, changing settings or a
  content type, turning a plugin off) are marked destructive, so clients can ask you first. Changes
  are live as soon as the tool returns.
- Bodies are Markdown both ways (`body_markdown`); an image in a body is
  `![alt](ferrite:media/<id>)`, and anything Markdown cannot express travels as a
  `ferrite-block` fence that the assistant keeps as it is. Updates take the `version` the
  assistant read, so a change made meanwhile by someone else is never overwritten.
- Successful writes are recorded in the audit log as `mcp.<tool>` under the app's name.
- Each user can make 120 tool calls a minute.

Installing or removing plugins and themes, managing users, tokens, sessions and connected
apps, switching the MCP endpoint, imports, backups, restores and retrying jobs are not
offered: they stay with a person in the admin.

## Site

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `site_overview` | Describes the site and the caller: URLs, version, active theme, content types with counts, taxonomies, menus, role and scopes. | none (each section needs its own read scope) | read |
| `site_search` | Full-text search over the content the caller may read. | `content:read` | read |

## Content

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `content_list` | Lists items of one content type as summaries, with filters and paging. | `content:read` | read |
| `content_get` | Returns one item whole, with its body as Markdown, custom fields, SEO and the `version` every write needs. | `content:read` | read |
| `content_create` | Creates a new draft item from a title and a Markdown body. | `content:write` | write |
| `content_update` | Changes an item; what is left out is kept, and a stale `version` is refused with the current item. | `content:write` | write |
| `content_publish` | Publishes a draft, pending or scheduled item now. | `content:write` | write |
| `content_unpublish` | Takes a published item off the site and turns it back into a draft. | `content:write` | write |
| `content_submit_for_review` | Offers a draft for review as `pending`. | `content:write` | write |
| `content_schedule` | Schedules a draft or pending item to publish at a future time. | `content:write` | write |
| `content_trash` | Moves an item to the trash. | `content:write` | write |
| `content_restore` | Brings a trashed item back as a draft. | `content:write` | write |
| `content_revisions` | Lists an item's saved revisions. | `content:read` | read |
| `content_restore_revision` | Brings an earlier revision back as a new version of the item. | `content:write` | write |
| `content_preview_link` | Returns the preview link of an item, for a signed-in browser. | `content:read` | read |

## Media

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `media_list` | Lists media files, filtered by kind, title words, uploader or trash. | `media:read` | read |
| `media_get` | Returns one media item with its URL, resized copies and the Markdown reference for a post body. | `media:read` | read |
| `media_upload` | Adds a file from base64 data (up to 6 MiB) or from a public `https` URL. | `media:write` | write |
| `media_update` | Changes a media item's title or alternative text. | `media:write` | write |
| `media_trash` | Moves a media item to the trash. | `media:write` | write |
| `media_restore` | Brings a trashed media item back. | `media:write` | write |

## Taxonomies and terms

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `taxonomies_list` | Lists the taxonomies, such as categories and tags. | `content:read` | read |
| `terms_list` | Lists the terms of one taxonomy, with filters and paging. | `terms:read` | read |
| `term_create` | Creates a term in a taxonomy. | `terms:write` | write |
| `term_update` | Changes a term's name, slug, parent or description. | `terms:write` | write |
| `term_delete` | Deletes a term; the content that had it stays. | `terms:write` | write |

## Menus

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `menus_list` | Lists the menus and the locations the active theme declares. | `menus:read` | read |
| `menu_get` | Returns one menu as a nested tree of entries. | `menus:read` | read |
| `menu_set` | Replaces a menu's whole item tree. | `menus:write` | write |

## Content types

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `content_types_list` | Lists the content types with their labels and flags. | `content:read` | read |
| `content_type_get` | Returns one content type with its custom field schema. | `content:read` | read |
| `content_type_create` | Registers a new content type, optionally with custom fields. | `settings:write` | write |
| `content_type_update` | Changes a content type's label, archive flag or field schema. | `settings:write` | write |

## Redirects

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `redirects_list` | Lists redirects, with a text filter and paging. | `redirects:read` | read |
| `redirect_create` | Redirects an old path to a new place, permanently by default; replaces an existing redirect from the same path. | `redirects:write` | write |
| `redirect_update` | Changes a redirect's source, target or status code. | `redirects:write` | write |
| `redirect_delete` | Deletes a redirect. | `redirects:write` | write |

## Comments

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `comments_list` | Lists comments newest first with their post and the counts per status. | `comments:read` | read |
| `comment_moderate` | Sets a comment to approved, pending, spam or trash. | `comments:write` | write |
| `comment_reply` | Posts a comment or a reply on a post as the signed-in user. | `comments:write` | write |

## Settings

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `settings_get` | Reads every site setting. | `settings:read` | read |
| `settings_update` | Changes site settings in one step; the `mcp.enabled` switch can only be changed in the admin. | `settings:write` | write |

## Appearance

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `themes_list` | Lists the installed and built-in themes and which one is active. | `extensions:read` | read |
| `theme_activate` | Makes an installed or built-in theme the active one. | `extensions:write` | write |
| `theme_settings_get` | Reads a theme's settings schema and current values. | `extensions:read` | read |
| `theme_settings_update` | Changes a theme's settings. | `extensions:write` | write |

## Plugins

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `plugins_list` | Lists the installed plugins with their state and approved permissions. | `extensions:read` | read |
| `plugin_enable` | Turns on an installed plugin. | `extensions:write` | write |
| `plugin_disable` | Turns off an installed plugin; its settings and data are kept. | `extensions:write` | write |
| `plugin_settings_get` | Reads a plugin's settings; a secret shows only whether it is set. | `extensions:read` | read |
| `plugin_settings_update` | Changes a plugin's settings; a secret can be set or cleared but never read back. | `extensions:write` | write |

## Users

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `users_list` | Lists user accounts with their role and status, never sign-in data. | `users:read` | read |
| `user_get` | Reads one user account. | `users:read` | read |

## System

| Tool | What it does | Scope | Access |
|---|---|---|---|
| `system_status` | Reports versions, mode, backends, schema state, job counts and nodes. | `system:read` | read |
| `jobs_list` | Lists background jobs, with filters by state and kind. | `system:read` | read |
| `audit_list` | Reads the audit log, with filters by actor, action and target. | `system:read` | read |

## Resources and prompts

Besides tools, the server offers **resources**, which an assistant reads as Markdown:

- `ferrite://site`: the site overview (the same facts as `site_overview`);
- `ferrite://content/{type}/{id}`: one item as Markdown, with its fields in TOML front
  matter;
- `ferrite://guide/writing`: the site's writing guide: how Markdown maps to blocks, the
  active theme's image sizes, titles, the SEO fields and publishing.

And **prompts**, ready-made requests a client can offer you (in many clients as slash
commands):

- `write_post` (`topic`, optional `tone`): drafts a post and saves it as a draft for you
  to review;
- `review_seo` (`item`): reviews one item's title, description and headings, with
  suggestions;
- `site_health`: sums up the site's status, dead jobs and queued ones with a last error,
  and plugin failures;
- `tidy_media`: finds images without alt text and proposes some.
