+++
title = "Accessibility checks and content health"
description = "Checks while you write (alt text, headings, link text, tables, broken links), an optional publish gate per content type, and a daily report of the whole site."
+++

# Accessibility checks and content health

FerriteCMS checks what you write for problems a reader would meet, and checks the whole
published site once a day. Both use the same rules, so the editor, the daily report and
an AI assistant always agree.

## While you write

The editor checks the item about 1.5 seconds after you stop typing, and again when you
open the publish dialog. Problems are marked in the text (a wavy underline, or an outline
around an image or a table) with a small button that says what is wrong, and listed
under **Checks** in the side panel, grouped by severity:

- **Errors**: an image with no alt text, an empty heading, a link without text, a table
  without a header row or column, an image that points at missing or trashed media, a link
  to a page of this site that does not exist.
- **Warnings**: an empty alt text on an image that is not marked decorative, alt text that
  is a file name ("IMG_1234.jpg"), a heading that skips a level (a `###` straight after
  the title), vague link text ("click here", "read more", also in German, French, Spanish
  and Polish), an empty table header cell.
- **Notices**: a link whose text is a bare address, a link to a page that redirects, a
  link to a heading that is not on the page, a raw HTML block (it is not checked).

**Show** selects the problem in the text; for an image, **Add alt text** opens its alt
text popover.

### Decorative images

An image that only decorates (a divider, a background pattern) should have an empty alt
text so screen readers skip it. Open the image's **Alt text** popover and turn on
**Decorative image**: the alt text is cleared and the image is no longer reported. An
empty alt text *without* the switch is reported as a warning, because nobody can tell
"decorative" from "forgotten".

## The publish gate

Each content type can block publishing while accessibility errors remain: in
**Structure → Content types**, open the type and turn on **Block publishing on
accessibility errors**. Then:

- the publish dialog lists the errors and its Publish button stays disabled until they are
  fixed;
- publishing or scheduling through the API or an AI assistant is refused the same way
  (the API answers `422` with problem type `content-quality` and the findings);
- an edit of an already published item is refused only when it *adds* errors, so a typo
  in an older page can always be fixed;
- items published by the scheduler at their time, imports and `seed demo` are never
  refused; the daily report lists their problems.

Only accessibility errors block publishing. Broken links do not: the page they point at
may be published together with this one.

## Suppressing a finding

When a finding is fine as it is, choose **Suppress…** and say why (a reason is
required). The finding stays listed as suppressed, with the reason, and counts nowhere: not
in the counts, not in the gate, not in the daily report. **Restore** under "Suppressed"
takes it back. On a type with the gate on, only editors and administrators may suppress
an error. Every suppression is in the audit log (`quality.suppress`).

## The daily report

**Content health** (`g h`, for editors and administrators) shows the report of the newest
run, at 04:00 UTC each day or when someone presses **Run now**:

- every published and scheduled item checked as in the editor;
- **broken links** to pages of this site, and pages that redirect;
- **orphans**: published pages (and items of types without an archive) that nothing links
  to: not the front page, not in a menu of their language, not linked from another item,
  and not under a parent that is reachable. A theme without the page tree or breadcrumbs of
  the starter themes may hide pages the report counts as reachable;
- **redirects** that lead to another redirect, in a loop, or to a page that does not exist;
- **stale** items, not edited for a year (Settings → Content health sets the days, 30 to
  3650).

The tabs group the findings (Accessibility, Links, Media, Site, Freshness); filter by
type, language and severity. A row opens the item's editor at the finding. The dashboard
shows the totals in its **Content health** card.

### External links

Off by default. With **Settings → Content health → Check external links** on, the daily
run also asks the sites you link to whether each page still exists. The server then
contacts those sites from its own address, so they learn that your site links to them.
It is careful about it: it reads each site's `robots.txt` and skips what it disallows,
asks one site at a time with two seconds between requests, checks at most 1 000 links a
day and reuses a good answer for a week, contacts only public addresses on the standard
ports, and never reads a page's content. A missing page (`404`, `410`) or a name that
does not exist is reported at once; a server error or a timeout only after it failed twice
at least 20 hours apart.

## For AI assistants

Two MCP tools use the same checks: `content_check` checks an item, or a draft before it is
written, and returns each problem with how to fix it; `health_report` reads the daily
report. The site's writing guide tells assistants to run `content_check` before
publishing. No tool suppresses a finding or starts a run; see
[AI assistants](08-ai-assistants.md).
