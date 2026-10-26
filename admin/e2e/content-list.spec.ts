// The content list under the real server (plan Task 13; 07 §3, 11 §6–§8): status tabs
// whose counts match the rows, search and the Mine and term filters, an author who sees
// only what the server allows (Review Focus 4), trash with Undo that restores the old
// status (F36), bulk publish with one stale item, the list keys, infinite paging and the
// review screens. Count-asserting tests use a content type of their own (F35), removed
// with everything in it at the end so later screenshots keep a clean sidebar.

import type { Locator, Page } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { SCHEMES, VIEWPORTS, screenFile } from "./support/snap";
import { uid } from "./support/uid";

/** A slug-safe type name of this run (`[a-z][a-z0-9_-]{0,31}`). */
const typeName = () => uid("t").slice(0, 32);
/** A word the full-text search can find on its own. */
const word = () =>
  `w${uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-10)}`;

const table = (page: Page, label: string) => page.getByRole("table", { name: label });
const rows = (page: Page, label: string) => table(page, label).locator("tbody tr[data-row]");
const tabs = (page: Page) => page.getByRole("navigation", { name: "Status" });
const tab = (page: Page, name: string) =>
  tabs(page).getByRole("link", { name: new RegExp(`^${name}\\b`) });
const tabCount = (page: Page, name: string) => tab(page, name).locator("[data-slot=tab-count]");
const bulkBar = (page: Page) => page.getByRole("region", { name: "Selection" });

async function openList(page: Page, type: string, label: string, query = ""): Promise<void> {
  await page.goto(`/admin/content/${type}${query}`);
  await expect(
    page.getByRole("heading", { level: 1, name: new RegExp(`^${label}`) }),
  ).toBeVisible();
  await loaded(page, label);
}

/** The table has left its loading state and its rows are the current filter's. */
async function loaded(page: Page, label: string): Promise<void> {
  await expect(table(page, label)).not.toHaveAttribute("aria-busy", "true");
  await expect(page.locator("[data-slot=data-table].opacity-60")).toHaveCount(0);
}

/** A row by its item's title. */
const row = (page: Page, label: string, title: string): Locator =>
  rows(page, label).filter({ has: page.getByRole("link", { name: title, exact: true }) });

async function rowAction(page: Page, label: string, title: string, action: string) {
  const r = row(page, label, title);
  await r.hover();
  await r.getByRole("button", { name: `Actions for ${title}` }).click();
  await page.getByRole("menuitem", { name: action }).click();
}

test("tabs_counts_and_filtering", async ({ as, site, backend }) => {
  const admin = await ContentApi.as(backend, site.url, "admin");
  const editor = await ContentApi.as(backend, site.url, "editor");
  const type = typeName();
  const label = `Tabs ${type.slice(-6)}`;
  const unique = word();
  try {
    await admin.createType(type, label);
    await admin.createIn(type, "Alpha draft", "draft");
    await admin.createIn(type, "Beta draft", "draft");
    await admin.createIn(type, "Gamma pending", "pending");
    await admin.createIn(type, "Delta published", "published");
    await admin.createIn(type, "Epsilon scheduled", "scheduled");
    await admin.createIn(type, "Zeta trashed", "trash");
    await editor.createIn(type, `Eta ${unique} by the editor`, "draft");

    const page = await as("admin");
    await openList(page, type, label);
    const expected = {
      All: 6,
      Draft: 3,
      Pending: 1,
      Scheduled: 1,
      Published: 1,
      Private: 0,
      Trash: 1,
    };
    // The header names the type and its count (every status but trash).
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(`${label}6`);
    for (const [name, count] of Object.entries(expected)) {
      await expect(tabCount(page, name)).toHaveText(String(count));
    }
    // Each tab lists exactly as many rows as its count says, and is the one current tab.
    for (const [name, count] of Object.entries(expected)) {
      await tab(page, name).click();
      await expect(tab(page, name)).toHaveAttribute("aria-current", "page");
      await expect(tabs(page).locator("[aria-current=page]")).toHaveCount(1);
      await loaded(page, label);
      await expect(rows(page, label)).toHaveCount(count);
    }
    await tab(page, "Private").click();
    await expect(
      page.getByRole("heading", { name: `No private ${label.toLowerCase()}` }),
    ).toBeVisible();
    // Defaults stay out of the URL.
    await tab(page, "All").click();
    await expect(page).toHaveURL(`${site.url}/admin/content/${type}`);

    // Search narrows (debounced; the URL follows).
    const search = page.getByRole("searchbox", { name: `Search ${label.toLowerCase()}` });
    await search.fill(unique);
    await expect(page).toHaveURL(new RegExp(`\\?q=${unique}$`));
    await loaded(page, label);
    await expect(rows(page, label)).toHaveCount(1);
    await expect(row(page, label, `Eta ${unique} by the editor`)).toBeVisible();
    // A search excludes Mine (07 §1): the chip is shown disabled.
    await expect(page.getByRole("button", { name: "Mine" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await page.getByRole("button", { name: "Clear search" }).click();
    await expect(page).toHaveURL(`${site.url}/admin/content/${type}`);

    // Mine narrows to the admin's own items (`author=<me.id>`, F34).
    const requests: string[] = [];
    page.on("request", (r) => requests.push(r.url()));
    await page.getByRole("button", { name: "Mine" }).click();
    await expect(page.getByRole("button", { name: "Mine" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page).toHaveURL(/\?mine=true$/);
    await loaded(page, label);
    await expect(rows(page, label)).toHaveCount(5);
    await expect(row(page, label, `Eta ${unique} by the editor`)).toHaveCount(0);
    expect(requests.some((u) => u.includes(`author=${admin.me.id}`))).toBe(true);
    // Mine within a tab.
    await tab(page, "Draft").click();
    await loaded(page, label);
    await expect(rows(page, label)).toHaveCount(2);
  } finally {
    await admin.removeType(type);
    await admin.dispose();
    await editor.dispose();
  }
});

test("numeric_search_survives_a_reload", async ({ as, site, backend }) => {
  // The router keeps free text as typed (M2b-2 Task 14): `?q=2024` stays the text "2024",
  // not a number, through a reload and in the request.
  const admin = await ContentApi.as(backend, site.url, "admin");
  const item = await admin.create("post", `Annual report 2024 ${uid()}`);
  const page = await as("admin");
  await openList(page, "post", "Posts", "?q=2024");
  const search = page.getByRole("searchbox", { name: "Search posts" });
  await expect(search).toHaveValue("2024");
  const sent = page.waitForRequest(
    (r) => r.url().includes("/api/v1/content/post?") && new URL(r.url()).searchParams.has("q"),
  );
  await page.reload();
  expect(new URL((await sent).url()).searchParams.get("q")).toBe("2024");
  await loaded(page, "Posts");
  await expect(search).toHaveValue("2024");
  expect(new URL(page.url()).search).toBe("?q=2024");
  await expect(row(page, "Posts", item.title)).toBeVisible();
  await admin.dispose();
});

test("author_counts_match_list", async ({ as, site, backend }) => {
  // `author2` (F35): no other test writes as this account.
  const admin = await ContentApi.as(backend, site.url, "admin");
  const mine = await ContentApi.as(backend, site.url, "author2");
  const other = await ContentApi.as(backend, site.url, "author");
  const type = typeName();
  const label = `Authors ${type.slice(-6)}`;
  try {
    await admin.createType(type, label);
    await mine.createIn(type, "My first draft", "draft");
    await mine.createIn(type, "My second draft", "draft");
    await mine.createIn(type, "My published piece", "published");
    await other.createIn(type, "Someone else's draft", "draft");
    await other.createIn(type, "Someone else's review", "pending");
    await admin.createIn(type, "The editor's published piece", "published");

    const page = await as("author2");
    await openList(page, type, label, "?status=draft");
    await expect(tabCount(page, "Draft")).toHaveText("2");
    await expect(rows(page, label)).toHaveCount(2);
    await expect(page.getByText("Someone else's draft")).toHaveCount(0);
    // Only published items of others are readable; their drafts and reviews are not.
    await expect(tabCount(page, "Pending")).toHaveText("0");
    await expect(tabCount(page, "Published")).toHaveText("2");
    await tab(page, "All").click();
    await loaded(page, label);
    await expect(tabCount(page, "All")).toHaveText("4");
    await expect(rows(page, label)).toHaveCount(4);
    await expect(page.getByText("Someone else's")).toHaveCount(0);
    // Actions come from `allowed`: another author's published item has no trash.
    const theirs = row(page, label, "The editor's published piece");
    await theirs.hover();
    await theirs.getByRole("button", { name: "Actions for The editor's published piece" }).click();
    await expect(page.getByRole("menuitem", { name: "Preview" })).toBeVisible();
    await expect(page.getByRole("menuitem", { name: "Move to trash" })).toHaveCount(0);
    await expect(page.getByRole("menuitem", { name: "Edit" })).toHaveCount(0);
    await page.keyboard.press("Escape");
  } finally {
    await admin.removeType(type);
    await Promise.all([admin.dispose(), mine.dispose(), other.dispose()]);
  }
});

test("trash_with_undo", async ({ as, site, backend }) => {
  // A published post: Undo must bring it back published, not as a draft (F36).
  const editor = await ContentApi.as(backend, site.url, "editor");
  const unique = word();
  const title = `Undo ${unique} published`;
  try {
    const created = await editor.createIn("post", title, "published");
    const page = await as("editor");
    await openList(page, "post", "Posts", `?q=${unique}`);
    await expect(rows(page, "Posts")).toHaveCount(1);
    await expect(row(page, "Posts", title).getByText("Published").first()).toBeVisible();

    await rowAction(page, "Posts", title, "Move to trash");
    const toast = page.locator("[data-sonner-toast]").filter({ hasText: "1 item moved to trash" });
    await expect(toast).toBeVisible();
    await expect(rows(page, "Posts")).toHaveCount(0);
    expect((await editor.get("post", created.id)).status).toBe("trash");

    await toast.getByRole("button", { name: "Undo" }).click();
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "1 item restored" }),
    ).toBeVisible();
    await expect(rows(page, "Posts")).toHaveCount(1);
    await expect(row(page, "Posts", title).locator("[data-slot=status-badge]")).toHaveText(
      "Published",
    );
    await expect.poll(async () => (await editor.get("post", created.id)).status).toBe("published");
    // F36: publishing again keeps the first `published_at`.
    expect(created.published_at).toBeTruthy();
    expect((await editor.get("post", created.id)).published_at).toBe(created.published_at);
  } finally {
    await editor.dispose();
  }
});

test("undo_after_leaving_the_list", async ({ as, site, backend }) => {
  // The toast outlives the route: Undo must act on the post's own type, not the open one.
  const editor = await ContentApi.as(backend, site.url, "editor");
  const unique = word();
  const title = `Leave ${unique} published`;
  try {
    const created = await editor.createIn("post", title, "published");
    const page = await as("editor");
    await openList(page, "post", "Posts", `?q=${unique}`);
    await rowAction(page, "Posts", title, "Move to trash");
    const toast = page.locator("[data-sonner-toast]").filter({ hasText: "1 item moved to trash" });
    await expect(toast).toBeVisible();
    const requests: string[] = [];
    page.on("request", (r) => {
      if (r.method() === "POST") requests.push(new URL(r.url()).pathname);
    });
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("link", { name: "Pages" })
      .click();
    await expect(page.getByRole("heading", { level: 1, name: /^Pages/ })).toBeVisible();
    await toast.getByRole("button", { name: "Undo" }).click();
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "1 item restored" }),
    ).toBeVisible();
    await expect.poll(async () => (await editor.get("post", created.id)).status).toBe("published");
    expect(requests).toContain(`/api/v1/content/post/${created.id}/restore`);
    expect(requests.some((p) => p.startsWith("/api/v1/content/page/"))).toBe(false);
    await expect(page.locator("[data-slot=failure-report]")).toHaveCount(0);
  } finally {
    await editor.dispose();
  }
});

test("contributor_trash_asks_first_and_offers_no_undo", async ({
  as,
  site,
  backend,
  snap,
  expectAccessible,
}) => {
  // A contributor may trash their own drafts but never restore them (07 §1), so there is
  // nothing to undo: the list asks first and the toast has no Undo.
  const contributor = await ContentApi.as(backend, site.url, "contributor");
  const editor = await ContentApi.as(backend, site.url, "editor");
  const unique = word();
  const title = `Contrib ${unique} draft`;
  try {
    const created = await contributor.createIn("post", title, "draft");
    const page = await as("contributor");
    await openList(page, "post", "Posts", `?q=${unique}`);
    await rowAction(page, "Posts", title, "Move to trash");
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("Move to trash?");
    await expect(dialog).toContainText("You won't be able to restore");
    await snap(page, "content-list-trash-confirm");
    await expectAccessible(page);
    // Cancel leaves it alone.
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    expect((await editor.get("post", created.id)).status).toBe("draft");

    await rowAction(page, "Posts", title, "Move to trash");
    await page.getByRole("alertdialog").getByRole("button", { name: "Move to trash" }).click();
    const toast = page.locator("[data-sonner-toast]").filter({ hasText: "1 item moved to trash" });
    await expect(toast).toBeVisible();
    await expect(toast).toContainText("an editor can");
    await expect(toast.getByRole("button", { name: "Undo" })).toHaveCount(0);
    await snap(page, "content-list-trash-no-undo");
    await expect(rows(page, "Posts")).toHaveCount(0);
    expect((await editor.get("post", created.id)).status).toBe("trash");
  } finally {
    await Promise.all([contributor.dispose(), editor.dispose()]);
  }
});

test("phone_tabs_keep_the_current_tab_in_view", async ({ as, site, backend }) => {
  const admin = await ContentApi.as(backend, site.url, "admin");
  const type = typeName();
  const label = `Phone ${type.slice(-6)}`;
  try {
    await admin.createType(type, label);
    await admin.createIn(type, "Trashed on a phone", "trash");
    const page = await as("admin");
    await page.setViewportSize(VIEWPORTS.mobile);
    await openList(page, type, label);
    const inView = async (name: string) =>
      tab(page, name).evaluate((el) => {
        const r = el.getBoundingClientRect();
        return r.left >= 0 && r.right <= window.innerWidth;
      });
    // Trash starts off-screen at 390 px; after going there it is scrolled into view.
    expect(await inView("Trash")).toBe(false);
    await page.goto(`/admin/content/${type}?status=trash`);
    await loaded(page, label);
    await expect(tabs(page).locator("[aria-current=page]")).toHaveCount(1);
    await expect.poll(() => inView("Trash")).toBe(true);
    // And back to All from the strip itself.
    await tab(page, "All").click();
    await expect(tabs(page).locator("[aria-current=page]")).toHaveCount(1);
    await expect.poll(() => inView("All")).toBe(true);
  } finally {
    await admin.removeType(type);
    await admin.dispose();
  }
});

test("bulk_publish_partial_failure", async ({ as, site, backend, guard }) => {
  // The browser logs the stale item's 412 as a console error; it is the point here.
  guard.allow(/status of 412/);
  const admin = await ContentApi.as(backend, site.url, "admin");
  const editor = await ContentApi.as(backend, site.url, "editor");
  const type = typeName();
  const label = `Bulk ${type.slice(-6)}`;
  try {
    await admin.createType(type, label);
    const drafts: Item[] = [];
    for (const t of ["First", "Second", "Third", "Fourth"]) {
      drafts.push(await editor.createIn(type, `${t} bulk draft`, "draft"));
    }
    const page = await as("editor");
    await openList(page, type, label, "?status=draft");
    await expect(rows(page, label)).toHaveCount(4);
    await table(page, label).getByRole("checkbox", { name: "Select all rows" }).check();
    await expect(bulkBar(page)).toContainText("4 selected");

    // Meanwhile someone saves the second draft: the list's version is stale.
    const stale = drafts[1];
    if (!stale) throw new Error("no second draft");
    await editor.patch(stale, { excerpt: "Changed elsewhere" });

    await bulkBar(page).getByRole("button", { name: "Publish" }).click();
    const report = page.getByRole("alert").filter({ hasText: "couldn't be published" });
    await expect(report).toContainText("1 of 4 couldn't be published");
    await expect(report.getByRole("listitem")).toHaveCount(1);
    await expect(report.getByRole("listitem")).toContainText("Second bulk draft");
    await expect(report.getByRole("listitem")).toContainText("Someone changed it");
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "3 items published" }),
    ).toBeVisible();
    // The published ones leave the Draft tab; the stale one stays, still selected.
    await expect(rows(page, label)).toHaveCount(1);
    await expect(bulkBar(page)).toContainText("1 selected");
    for (const d of drafts) {
      const now = await editor.get(type, d.id);
      expect(now.status, d.title).toBe(d.id === stale.id ? "draft" : "published");
    }
    await report.getByRole("button", { name: "Dismiss" }).click();
    await expect(report).toHaveCount(0);
  } finally {
    await admin.removeType(type);
    await Promise.all([admin.dispose(), editor.dispose()]);
  }
});

test("keyboard_list_navigation", async ({ as, site, backend }) => {
  const admin = await ContentApi.as(backend, site.url, "admin");
  const editor = await ContentApi.as(backend, site.url, "editor");
  const type = typeName();
  const label = `Keys ${type.slice(-6)}`;
  try {
    await admin.createType(type, label);
    // Newest first (order=updated): Three, Two, One.
    const one = await editor.createIn(type, "One by keyboard", "draft");
    await editor.createIn(type, "Two by keyboard", "draft");
    const three = await editor.createIn(type, "Three by keyboard", "draft");
    const page = await as("editor");
    await openList(page, type, label);
    const all = rows(page, label);
    await expect(all).toHaveCount(3);

    // `/` focuses the search, but not while typing in it.
    await page.locator("body").click({ position: { x: 5, y: 300 } });
    await page.keyboard.press("/");
    const search = page.getByRole("searchbox", { name: `Search ${label.toLowerCase()}` });
    await expect(search).toBeFocused();
    await page.keyboard.press("/");
    await expect(search).toHaveValue("/");
    await search.fill("");

    await all.nth(0).focus();
    await page.keyboard.press("j");
    await expect(all.nth(1)).toBeFocused();
    await page.keyboard.press("j");
    await expect(all.nth(2)).toBeFocused();
    await page.keyboard.press("k");
    await page.keyboard.press("k");
    await expect(all.nth(0)).toBeFocused();
    await page.keyboard.press("x");
    await expect(all.nth(0)).toHaveAttribute("data-state", "selected");
    await expect(bulkBar(page)).toContainText("1 selected");
    // Esc clears the selection; x again selects.
    await page.keyboard.press("Escape");
    await expect(bulkBar(page)).toHaveCount(0);
    await page.keyboard.press("x");
    await expect(bulkBar(page)).toContainText("1 selected");
    // `#` trashes the selection, with Undo.
    await page.keyboard.press("#");
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "1 item moved to trash" }),
    ).toBeVisible();
    await expect(all).toHaveCount(2);
    expect((await editor.get(type, three.id)).status).toBe("trash");

    // Enter opens the focused row's editor.
    await all.nth(1).focus();
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(`${site.url}/admin/content/${type}/${one.id}`);
    // Loaded (the item and its autosave slot) before the type is removed below.
    await expect(page.getByRole("textbox", { name: "Body" })).toBeVisible();
  } finally {
    await admin.removeType(type);
    await Promise.all([admin.dispose(), editor.dispose()]);
  }
});

test("infinite_paging", async ({ as, site, backend }) => {
  const admin = await ContentApi.as(backend, site.url, "admin");
  const editor = await ContentApi.as(backend, site.url, "editor");
  const type = typeName();
  const label = `Paging ${type.slice(-6)}`;
  try {
    await admin.createType(type, label);
    for (let batch = 0; batch < 6; batch++) {
      await Promise.all(
        Array.from({ length: 10 }, (_, i) =>
          editor.create(type, `Paged item ${String(batch * 10 + i + 1).padStart(2, "0")}`),
        ),
      );
    }
    const page = await as("editor");
    const lists: string[] = [];
    page.on("request", (r) => {
      if (new URL(r.url()).pathname === `/api/v1/content/${type}`) lists.push(r.url());
    });
    await openList(page, type, label);
    await expect(rows(page, label)).toHaveCount(50);
    expect(lists[0]).toContain("limit=50");
    await expect(page.getByRole("button", { name: "Load more" })).toBeVisible();
    // Scrolling to the end of the frame loads the next page by itself.
    await page.locator("[data-slot=list-sentinel]").scrollIntoViewIfNeeded();
    await expect(rows(page, label)).toHaveCount(60);
    await expect(page.getByText("All 60 items")).toBeVisible();
    // Exactly one next-page request: the sentinel and the button never fetch twice.
    await page.waitForLoadState("networkidle");
    expect(lists.filter((u) => u.includes("cursor="))).toHaveLength(1);
  } finally {
    await admin.removeType(type);
    await Promise.all([admin.dispose(), editor.dispose()]);
  }
});

/** The inline opacity of every list row inserted while `act` runs (Motion writes `initial`). */
async function insertedRowOpacity(page: Page, act: () => Promise<void>): Promise<string[]> {
  await page.evaluate(() => {
    const w = window as unknown as { __rows: string[]; __rowObserver?: MutationObserver };
    w.__rows = [];
    w.__rowObserver = new MutationObserver((records) => {
      for (const r of records) {
        for (const node of r.addedNodes) {
          if (node instanceof HTMLElement && node.matches("tr[data-row]")) {
            w.__rows.push(node.style.opacity);
          }
        }
      }
    });
    const body = document.querySelector("[data-slot=data-table] tbody");
    if (body) w.__rowObserver.observe(body, { childList: true });
  });
  await act();
  return page.evaluate(() => {
    const w = window as unknown as { __rows: string[]; __rowObserver?: MutationObserver };
    w.__rowObserver?.disconnect();
    return w.__rows;
  });
}

test("row_motion_respects_reduced_motion", async ({ as, site, backend }) => {
  const admin = await ContentApi.as(backend, site.url, "admin");
  const type = typeName();
  const label = `Motion ${type.slice(-6)}`;
  try {
    await admin.createType(type, label);
    await admin.createIn(type, "Stays put", "draft");
    await admin.createIn(type, "Comes and goes", "draft");
    const page = await as("admin");
    const roundTrip = async () => {
      await rowAction(page, label, "Comes and goes", "Move to trash");
      const toast = page.locator("[data-sonner-toast]").filter({ hasText: "moved to trash" });
      await expect(rows(page, label)).toHaveCount(1);
      await toast.getByRole("button", { name: "Undo" }).click();
      await expect(rows(page, label)).toHaveCount(2);
    };
    // Motion allowed: the row that comes back fades in from 0.
    await openList(page, type, label);
    expect(await insertedRowOpacity(page, roundTrip)).toEqual(["0"]);
    // Reduced motion: it is simply there (no enter, exit or layout animation).
    await page.emulateMedia({ reducedMotion: "reduce" });
    await openList(page, type, label);
    const still = await insertedRowOpacity(page, roundTrip);
    expect(still).toHaveLength(1);
    expect(["", "1"]).toContain(still[0]);
  } finally {
    await admin.removeType(type);
    await admin.dispose();
  }
});

test("dashboard_tiles_open_the_filtered_list", async ({ as, site }) => {
  // The dashboard's tiles (Task 12) link with `?status=`, which this route's search reads.
  const page = await as("editor");
  await page.goto("/admin/");
  await page.getByRole("link", { name: /^Drafts/ }).click();
  await expect(page).toHaveURL(`${site.url}/admin/content/post?status=draft`);
  await expect(tab(page, "Draft")).toHaveAttribute("aria-current", "page");
  await loaded(page, "Posts");
  for (const badge of await rows(page, "Posts").locator("[data-slot=status-badge]").all()) {
    await expect(badge).toHaveText("Draft");
  }
});

test("screens", async ({ as, site, backend, snap, expectAccessible }) => {
  test.setTimeout(300_000);
  const admin = await ContentApi.as(backend, site.url, "admin");
  const editor = await ContentApi.as(backend, site.url, "editor");
  const type = typeName();
  const label = "Field notes";
  const topics = `x${type.replace(/-/g, "").slice(-12)}`;
  const guides = typeName();
  try {
    await admin.createType(type, label);
    await admin.createTaxonomy(topics, "Topics", [type]);
    const design = await admin.createTerm(topics, "Design");
    const ops = await admin.createTerm(topics, "Operations");
    await admin.createTerm(topics, "Performance");
    const spec = [
      ["Designing calm interfaces for busy editors", "published", admin, [design.id]],
      ["Why the sandbox never trusts a plugin", "published", editor, [ops.id]],
      ["A field guide to cache invalidation", "scheduled", admin, [ops.id]],
      ["Notes from the October release", "pending", editor, []],
      ["Typography that scales from phone to wall", "draft", admin, [design.id]],
      ["Measuring what the page cache saves", "draft", editor, [ops.id]],
      ["The case against admin-notice nags", "draft", admin, [design.id]],
      ["An old announcement nobody needs", "trash", editor, []],
    ] as const;
    for (const [title, status, who, terms] of spec) {
      await who.createIn(type, title, status, { terms: [...terms] });
    }

    const page = await as("admin");
    await openList(page, type, label);
    await snap(page, "content-list-all");
    await expectAccessible(page);

    for (const name of ["Draft", "Pending", "Scheduled", "Published", "Private", "Trash"]) {
      await tab(page, name).click();
      await loaded(page, label);
      // Rows enter and leave with Motion (JS) animations that `snap`'s settle cannot
      // see: wait until the leaving rows are gone and the rest stand still.
      await expect
        .poll(() =>
          rows(page, label).evaluateAll((trs) =>
            trs.every((tr) => {
              const cs = getComputedStyle(tr);
              return cs.opacity === "1" && cs.transform === "none";
            }),
          ),
        )
        .toBe(true);
      await snap(page, `content-list-${name.toLowerCase()}`);
      await expectAccessible(page);
    }

    // The row menu and the only confirm (delete permanently), then the empty trash.
    const trashed = "An old announcement nobody needs";
    await rowAction(page, label, trashed, "Delete permanently");
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText(trashed);
    await snap(page, "content-list-delete-confirm");
    await expectAccessible(page);
    await dialog.getByRole("button", { name: "Delete permanently" }).click();
    await expect(page.getByText("Trash is empty")).toBeVisible();
    await expect(tabCount(page, "Trash")).toHaveText("0");
    await snap(page, "content-list-trash-empty");
    await expectAccessible(page);

    // The row menu, open.
    await tab(page, "All").click();
    await loaded(page, label);
    const first = row(page, label, "Designing calm interfaces for busy editors");
    await first.hover();
    await first.getByRole("button", { name: /^Actions for/ }).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await snap(page, "content-list-row-menu");
    await expectAccessible(page);
    await page.keyboard.press("Escape");

    // The term picker; picking a term narrows the list.
    await page.getByRole("button", { name: "Filter by Topics" }).click();
    await expect(page.getByRole("option", { name: "Operations" })).toBeVisible();
    await snap(page, "content-list-term-picker");
    await expectAccessible(page);
    await page.getByRole("option", { name: "Operations" }).click();
    await expect(page).toHaveURL(new RegExp(`term=${ops.id}`));
    await loaded(page, label);
    await expect(rows(page, label)).toHaveCount(3);
    await snap(page, "content-list-filtered");
    await expectAccessible(page);
    await page.getByRole("button", { name: "Remove the Topics filter" }).click();
    await expect(rows(page, label)).toHaveCount(7);

    // A search with no results; the chips it excludes are disabled.
    await page.getByRole("searchbox", { name: "Search field notes" }).fill("zzzznothing");
    await expect(page.getByText("No results for “zzzznothing”")).toBeVisible();
    await snap(page, "content-list-search-empty");
    await expectAccessible(page);
    await page.getByRole("button", { name: "Clear search" }).first().click();
    await loaded(page, label);

    // The selection bar.
    await expect(rows(page, label)).toHaveCount(7);
    await rows(page, label).nth(0).getByRole("checkbox").check();
    await rows(page, label).nth(2).getByRole("checkbox").check();
    await rows(page, label).nth(3).getByRole("checkbox").check();
    await expect(bulkBar(page)).toContainText("3 selected");
    await snap(page, "content-list-selection");
    await expectAccessible(page);

    // The Undo toast after a bulk trash.
    await bulkBar(page)
      .getByRole("button", { name: /Move to trash|^Trash \d/ })
      .click();
    const toast = page.locator("[data-sonner-toast]").filter({ hasText: "moved to trash" });
    await expect(toast).toBeVisible();
    await snap(page, "content-list-undo-toast");
    await toast.getByRole("button", { name: "Undo" }).click();
    await expect(rows(page, label)).toHaveCount(7);

    // A hierarchical type: the parent column and the parent's slug before the child's.
    await admin.createType(guides, "Guides", true);
    const parent = await admin.createIn(guides, "Getting started", "published");
    await admin.createIn(guides, "Installing on a laptop", "published", { parent_id: parent.id });
    await admin.createIn(guides, "Running a cluster", "draft", { parent_id: parent.id });
    await openList(page, guides, "Guides");
    await expect(page.getByRole("columnheader", { name: "Parent" })).toBeVisible();
    await expect(row(page, "Guides", "Running a cluster")).toContainText(
      "getting-started/running-a-cluster",
    );
    await snap(page, "content-list-hierarchical");
    await expectAccessible(page);

    // Loading: the first page held back.
    await page.route(`**/api/v1/content/${type}?*`, async (route) => {
      await new Promise((r) => setTimeout(r, 60_000));
      await route.continue().catch(() => undefined);
    });
    await page.goto(`/admin/content/${type}`);
    await expect(table(page, label)).toHaveAttribute("aria-busy", "true");
    // `snap` waits for the network to go idle, which a held request never does.
    for (const scheme of SCHEMES) {
      await page.emulateMedia({ colorScheme: scheme });
      for (const v of ["desktop", "mobile"] as const) {
        await page.setViewportSize(VIEWPORTS[v]);
        await page.screenshot({
          path: screenFile("content-list-loading", scheme, v),
          fullPage: true,
          animations: "disabled",
        });
      }
    }
    await page.emulateMedia({ colorScheme: "light" });
    await page.setViewportSize(VIEWPORTS.desktop);
    await page.unrouteAll({ behavior: "ignoreErrors" });
  } finally {
    await admin.removeType(guides);
    await admin.removeType(type, [topics]);
    await Promise.all([admin.dispose(), editor.dispose()]);
  }
});
