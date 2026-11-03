+++
title = "Releases and reviews"
description = "Preview while you write, group changes into releases applied together now or on a schedule, preview the whole site as a release will make it, share signed preview links and require approvals before publishing."
+++

# Releases and reviews

FerriteCMS has three tools for an editorial process: a **live preview** beside the editor,
**releases** that apply several changes together, and **approval workflows** that make a
content type wait for a reviewer before anything of it is published.

## Live preview

In the editor, **Preview** (or `Ctrl`/`⌘` + `Alt` + `P`) shows the item as the active theme
renders it, with what you typed so far. On a wide screen the preview sits beside the text
and the divider between them can be dragged (or moved with the arrow keys); on a narrower
screen a **Write | Preview** switch swaps one for the other. The preview can show a desktop,
tablet or phone width.

The preview follows your autosaved working copy: while it is open the editor autosaves
two seconds after you stop typing, and the preview reloads after each save. The title,
excerpt, body, custom fields and SEO fields show as typed; the address, parent, menu order
and terms show as saved, since they change the URL. A new item needs one save first.

The preview runs in a sandboxed frame, so nothing a theme or plugin prints there can reach
the admin. It loads through a short-lived preview link of your own.

## Releases

A release is a named set of changes that goes live together, in one step. Each entry is one
item with:

- a **staged change**: new title, body, fields, SEO and so on, held in the release until it
  is applied (nothing is live before);
- and/or an action: **publish** a draft or pending item, or **unpublish** a published one.

Create releases under **Releases** (`g r`), or from the editor with **Add to release…**,
which can stage your working copy. **Preview site** opens the whole site as it will look
once the release is applied: the home page, listings, the page tree, menus, feeds and the
sitemap, with a bar at the top that says it is a preview. **Exit preview** in that bar
leaves it.

**Apply now** writes every entry in one transaction: if one entry cannot be applied (a slug
taken since, an item someone else changed), nothing is written and the release tells you
which entry failed. **Check** shows the same report without writing anything.

If someone changes an item after it was staged, its entry shows a **conflict**: open it,
compare, and **Rebase** to stage on top of the current version.

**Schedule…** applies the release at a set time. A scheduled release locks its items: they
cannot be edited, published or trashed until the release is applied or unscheduled. If a
scheduled release cannot be applied (for example an import changed one of its items), it is
marked **failed** with a report, its items are unlocked and its author and scheduler get a
mail.

Who may do what: everyone who writes content may create releases and stage their own
drafts; publishing through a release needs the right to publish those items. Authors see
their own releases, editors and administrators all of them.

## Sharing previews

**Share preview…** (in the editor or on a release) makes a link that opens the preview in
any browser without signing in, for 1 hour, 24 hours or 7 days. The link is shown once;
copy it then. A shared preview always shows its preview bar, never counts as a page view
and is never cached or indexed. You can revoke a link at any time, and it stops working
when the person who made it is disabled or loses the right to see the item.

## Approval workflows

An administrator can give a content type a **workflow** on its screen under **Content
types**: one or more review steps, each naming who may approve (editors, administrators
or named people) and how many approvals it needs. The preset **One editor approves** is
one step for editors and administrators.

In a type with a workflow:

1. The author **submits for review**, optionally naming reviewers and a note. Reviewers get
   a mail.
2. Reviewers find it under **Reviews** (`g v`) and **Approve** or **Request changes**, with
   a comment. Nobody approves their own item or their own submission.
3. Once every step is approved, the item can be published (or scheduled, or added to a
   release). An edit after the approval makes it stale: submit it again.

A published item of such a type is changed through a release: **Save as change** stages
the edit, which is reviewed as its own entry. Editors and administrators who may approve in
the workflow can still fix a published item in place; such edits are recorded in the audit
log.

An administrator can publish without the approvals in an emergency by giving a reason;
the override is recorded in the audit log.

## Imports and scheduled releases

Imports do not wait for release locks. If an import changes an item of a scheduled release,
the release fails with a conflict at its time instead of overwriting the imported text;
rebase the entry and schedule it again.

## AI assistants

Assistants can create releases, stage changes, check, schedule and apply them, and make
preview links. Approvals stay with people: an assistant can submit for review and list the
queue, but not approve, request changes or override. See
[AI assistants (MCP)](08-ai-assistants.md).
