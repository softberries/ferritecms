// Menus under the real server (M2b-2 Task 11; 07 §3 Menus): building the theme's primary
// menu from its location (a page, a category and a custom link, one nested from the
// keyboard) and seeing it on the public home page; a pointer drag; Remove with Undo in the
// draft; leaving with unsaved changes; the stale guard (Review Focus 4); Delete with Undo;
// and the review screens. Every menu, item and term is the test's own (E2E data hygiene);
// the seeded `primary` menu is put back afterwards. The tests of this file run one after
// another (one worker per file), so only one of them touches `primary` at a time.

import type { Locator, Page } from "@playwright/test";
import { ContentApi, type Menu } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { mod } from "./support/keys";
import { uid } from "./support/uid";

const header = (page: Page) => page.locator("[data-slot=page-header]");
const tree = (page: Page) => page.locator("[data-slot=menu-items] [data-slot=sortable-tree]");
const addPanel = (page: Page) => page.locator("[data-slot=menu-add-items]");
const staleDialog = (page: Page) => page.locator("[data-slot=stale-dialog]");
const saveButton = (page: Page) => header(page).getByRole("button", { name: "Save", exact: true });

/** Every entry as `level:label`, top to bottom (the label or the muted title). */
const shape = (list: Locator) =>
  list
    .locator("[data-sortable-row]")
    .evaluateAll((rows) =>
      rows.map(
        (r) =>
          `${r.getAttribute("aria-level")}:${r.querySelector("[data-slot=menu-entry-label]")?.textContent ?? ""}`,
      ),
    );

/** A short unique suffix that fits a menu name. */
const short = () =>
  uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-8);

const link = (label: string, url: string, parent: string | null = null, position = 0) => ({
  id: crypto.randomUUID(),
  kind: "url" as const,
  url,
  label,
  parent_id: parent,
  position,
});

/** A menu of custom links made through the API. */
async function newMenu(api: ContentApi, prefix: string, items: Menu["items"]): Promise<Menu> {
  const s = short();
  return api.putMenu(`${prefix}${s}`, { label: `Menu ${s}`, items });
}

async function openMenu(page: Page, name: string): Promise<void> {
  await page.goto(`/admin/structure/menus/${name}`);
  await expect(page.locator("[data-slot=menu-editor]")).toBeVisible();
}

/** A row menu item, waiting for the menu to close again. */
async function rowMenu(page: Page, label: string, item: string): Promise<void> {
  await page.getByRole("button", { name: `Actions for ${label}`, exact: true }).click();
  await page.getByRole("menuitem", { name: item }).click();
  await expect(page.getByRole("menu")).toBeHidden();
}

/** Saves and waits for the `PUT`. */
async function save(page: Page): Promise<void> {
  const put = page.waitForResponse(
    (r) => r.request().method() === "PUT" && r.url().includes("/api/v1/menus/"),
  );
  await saveButton(page).click();
  expect((await put).status()).toBe(200);
  await expect(saveButton(page)).toBeDisabled();
}

test("build_primary_menu_and_see_it_on_the_site", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const s = short();
  const pageItem = await api.createIn("page", `About ${s}`, "published");
  const term = await api.createTerm("category", `Topic ${s}`);
  // The site links a term only while it has a published item.
  const filed = await api.createIn("post", `Filed ${s}`, "published", { terms: [term.id] });
  const seeded = await api.getMenu("primary");
  await api.deleteMenu("primary");
  const page = await as("editor");
  const list = tree(page);
  try {
    // The location has no menu now: create one from it.
    await page.goto("/admin/structure/menus");
    const location = page.locator("[data-slot=menu-location][data-name=primary]");
    await expect(location).toContainText("Nothing shown here yet");
    await location.getByRole("button", { name: "Create a menu for Primary navigation" }).click();
    await expect(page).toHaveURL(/\/admin\/structure\/menus\/primary$/);
    await expect(page.locator("[data-slot=menu-items] [data-slot=empty-state]")).toContainText(
      "No items yet",
    );

    // A page from the content search.
    await addPanel(page).getByRole("searchbox", { name: "Search content" }).fill(`About ${s}`);
    await addPanel(page)
      .getByRole("checkbox", { name: `About ${s}` })
      .check();
    await addPanel(page).getByRole("button", { name: "Add 1 item" }).click();
    await expect.poll(() => shape(list)).toEqual([`1:About ${s}`]);

    // A category from the terms tab.
    await addPanel(page).getByRole("tab", { name: "Terms" }).click();
    await addPanel(page).getByRole("combobox", { name: "Taxonomy" }).click();
    await page.getByRole("option", { name: "Categories" }).click();
    await addPanel(page).getByRole("searchbox", { name: "Filter Categories" }).fill(`Topic ${s}`);
    await addPanel(page)
      .getByRole("checkbox", { name: `Topic ${s}` })
      .check();
    await addPanel(page).getByRole("button", { name: "Add 1 item" }).click();

    // A custom link.
    await addPanel(page).getByRole("tab", { name: "Custom link" }).click();
    const address = addPanel(page).getByRole("textbox", { name: "Address" });
    await address.fill("javascript:alert(1)");
    await addPanel(page).getByRole("button", { name: "Add link" }).click();
    await expect(addPanel(page)).toContainText("Use http://, https:// or a path starting with /");
    await address.fill(`https://example.com/${s}`);
    await addPanel(page).getByRole("textbox", { name: "Label" }).fill(`Elsewhere ${s}`);
    await addPanel(page).getByRole("button", { name: "Add link" }).click();
    await expect
      .poll(() => shape(list))
      .toEqual([`1:About ${s}`, `1:Topic ${s}`, `1:Elsewhere ${s}`]);

    // Nest the category under the page from the keyboard: its menu, then Indent.
    const actions = page.getByRole("button", { name: `Actions for Topic ${s}`, exact: true });
    await actions.focus();
    await page.keyboard.press("Enter");
    // Edit, Move up, Move down, Indent: the arrows walk the open menu.
    for (const item of ["Edit", "Move up", "Move down"]) {
      await expect(page.getByRole("menuitem", { name: item })).toBeFocused();
      await page.keyboard.press("ArrowDown");
    }
    const indent = page.getByRole("menuitem", { name: "Indent" });
    await expect(indent).toBeFocused();
    await page.keyboard.press("Enter");
    await expect
      .poll(() => shape(list))
      .toEqual([`1:About ${s}`, `2:Topic ${s}`, `1:Elsewhere ${s}`]);
    await expect(page.locator("[data-slot=sortable-status]")).toContainText(
      `Topic ${s} is now under About ${s}`,
    );

    // Save from the keyboard.
    const put = page.waitForResponse(
      (r) => r.request().method() === "PUT" && r.url().endsWith("/api/v1/menus/primary"),
    );
    await page.keyboard.press(`${await mod(page)}+s`);
    expect((await put).status()).toBe(200);
    await expect(saveButton(page)).toBeDisabled();

    // The public home page shows them, nested and in order.
    await page.goto("/");
    const nav = page.getByRole("navigation", { name: "Primary" });
    await expect(nav.locator(":scope > ul > li > a")).toHaveText([`About ${s}`, `Elsewhere ${s}`]);
    await expect(nav.locator(".menu__sub a")).toHaveText([`Topic ${s}`]);
    await expect(nav.getByRole("link", { name: `Elsewhere ${s}` })).toHaveAttribute(
      "href",
      `https://example.com/${s}`,
    );
  } finally {
    await page.context().close();
    if (seeded) await api.putMenu("primary", { label: seeded.label, items: seeded.items });
    await api.purge(pageItem);
    await api.purge(filed);
    await api.deleteTerm(term.id);
    await api.dispose();
  }
});

test("drag_reorder_with_pointer", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const menu = await newMenu(api, "dr", [
    link("Alpha", "/alpha", null, 0),
    link("Beta", "/beta", null, 1),
    link("Gamma", "/gamma", null, 2),
  ]);
  const page = await as("admin");
  const list = tree(page);
  try {
    await openMenu(page, menu.name);
    await expect.poll(() => shape(list)).toEqual(["1:Alpha", "1:Beta", "1:Gamma"]);
    const from = await list
      .getByRole("button", { name: "Reorder Gamma", exact: true })
      .boundingBox();
    const to = await list.getByRole("button", { name: "Reorder Alpha", exact: true }).boundingBox();
    if (!from || !to) throw new Error("no layout");
    const x = from.x + from.width / 2;
    await page.mouse.move(x, from.y + from.height / 2);
    await page.mouse.down();
    await page.mouse.move(x, from.y + from.height / 2 - 6, { steps: 2 });
    await page.mouse.move(x, to.y + to.height / 2 - 4, { steps: 12 });
    await page.mouse.up();
    await expect.poll(() => shape(list)).toEqual(["1:Gamma", "1:Alpha", "1:Beta"]);
    // dnd-kit swallows the clicks of the first 50 ms after a pointer drop (so the drop is
    // no click on the row); save from the keyboard instead of clicking at once.
    const put = page.waitForResponse(
      (r) => r.request().method() === "PUT" && r.url().endsWith(`/api/v1/menus/${menu.name}`),
    );
    await page.keyboard.press(`${await mod(page)}+s`);
    expect((await put).status()).toBe(200);
    await expect(saveButton(page)).toBeDisabled();
    const stored = await api.getMenu(menu.name);
    expect(stored?.items.filter((i) => !i.parent_id).map((i) => [i.label, i.position])).toEqual([
      ["Gamma", 0],
      ["Alpha", 1],
      ["Beta", 2],
    ]);
  } finally {
    await page.context().close();
    await api.deleteMenu(menu.name);
    await api.dispose();
  }
});

test("remove_with_undo_before_save", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const parent = link("Parent", "/parent", null, 1);
  const menu = await newMenu(api, "rm", [
    link("First", "/first", null, 0),
    parent,
    link("Child", "/child", parent.id, 0),
  ]);
  const page = await as("admin");
  const list = tree(page);
  try {
    await openMenu(page, menu.name);
    await expect.poll(() => shape(list)).toEqual(["1:First", "1:Parent", "2:Child"]);

    // Remove moves the children up; Undo puts both back and the draft is clean again.
    await rowMenu(page, "Parent", "Remove");
    await expect.poll(() => shape(list)).toEqual(["1:First", "1:Child"]);
    await expect(saveButton(page)).toBeEnabled();
    const toast = page.locator("[data-sonner-toast]").filter({ hasText: "Removed Parent" });
    await expect(toast).toContainText("its 1 sub-item moved up");
    await toast.getByRole("button", { name: "Undo" }).click();
    await expect.poll(() => shape(list)).toEqual(["1:First", "1:Parent", "2:Child"]);
    await expect(saveButton(page)).toBeDisabled();
    // Sonner hands focus back once the toast has gone; a menu opened before would close.
    await expect(toast).toHaveCount(0);

    // A removal that is saved has no Undo any more.
    await rowMenu(page, "First", "Remove");
    await expect.poll(() => shape(list)).toEqual(["1:Parent", "2:Child"]);
    await save(page);
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "Removed First" }),
    ).toHaveCount(0);
    const stored = await api.getMenu(menu.name);
    expect(stored?.items.map((i) => i.label)).toEqual(["Parent", "Child"]);
  } finally {
    await page.context().close();
    await api.deleteMenu(menu.name);
    await api.dispose();
  }
});

test("leave_with_unsaved_changes_asks", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const menu = await newMenu(api, "lv", [link("Only", "/only")]);
  const page = await as("admin");
  const dialog = page.locator("[data-slot=menu-leave-dialog]");
  const sidebarLink = page
    .locator("[data-slot=sidebar]")
    .getByRole("link", { name: "Taxonomies", exact: true });
  try {
    await openMenu(page, menu.name);
    await page
      .locator("[data-slot=menu-settings]")
      .getByRole("textbox", { name: "Label" })
      .fill("Renamed");
    await sidebarLink.click();
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Stay" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(page).toHaveURL(new RegExp(`/admin/structure/menus/${menu.name}$`));

    // Discard makes it clean: leaving no longer asks.
    await header(page).getByRole("button", { name: "Discard" }).click();
    await expect(saveButton(page)).toBeDisabled();
    await rowMenu(page, "Only", "Remove");
    await sidebarLink.click();
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Leave" }).click();
    await expect(page).toHaveURL(/\/admin\/structure\/taxonomies$/);
    expect((await api.getMenu(menu.name))?.items.map((i) => i.label)).toEqual(["Only"]);
  } finally {
    await page.context().close();
    await api.deleteMenu(menu.name);
    await api.dispose();
  }
});

test("menu_stale_guard", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const menu = await newMenu(api, "sg", [
    link("Home", "/", null, 0),
    link("Blog", "/blog/", null, 1),
  ]);
  const a = await as("admin");
  const b = await as("admin");
  try {
    await openMenu(a, menu.name);
    await openMenu(b, menu.name);

    // A adds a link and saves; B's save asks, and Reload shows A's menu without writing.
    await addPanel(a).getByRole("tab", { name: "Custom link" }).click();
    await addPanel(a).getByRole("textbox", { name: "Address" }).fill("/from-a");
    await addPanel(a).getByRole("textbox", { name: "Label" }).fill("From A");
    await addPanel(a).getByRole("button", { name: "Add link" }).click();
    await save(a);
    await rowMenu(b, "Blog", "Remove");
    await saveButton(b).click();
    await expect(staleDialog(b)).toBeVisible();
    await expect(staleDialog(b)).toContainText("Changed by someone else");
    await staleDialog(b).getByRole("button", { name: "Reload" }).click();
    await expect(staleDialog(b)).toHaveCount(0);
    await expect.poll(() => shape(tree(b))).toEqual(["1:Home", "1:Blog", "1:From A"]);
    expect((await api.getMenu(menu.name))?.items.map((i) => i.label)).toEqual([
      "Home",
      "Blog",
      "From A",
    ]);

    // A changes it again; B's next save asks again, and only Overwrite writes.
    await rowMenu(a, "Home", "Remove");
    await save(a);
    await rowMenu(b, "Blog", "Remove");
    await saveButton(b).click();
    await expect(staleDialog(b)).toBeVisible();
    await staleDialog(b).getByRole("button", { name: "Overwrite" }).click();
    await expect(saveButton(b)).toBeDisabled();
    expect((await api.getMenu(menu.name))?.items.map((i) => i.label)).toEqual(["Home", "From A"]);
  } finally {
    await a.context().close();
    await b.context().close();
    await api.deleteMenu(menu.name);
    await api.dispose();
  }
});

test("delete_menu_with_undo", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const menu = await newMenu(api, "dl", [link("Kept", "/kept")]);
  const page = await as("admin");
  try {
    await openMenu(page, menu.name);
    const dialog = page.locator("[data-slot=delete-menu-dialog]");
    const unsaved = "Your unsaved changes to it are discarded too.";
    await header(page).getByRole("button", { name: "Delete menu" }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog).not.toContainText(unsaved);
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    // With unsaved edits, the confirm says they go too (final review B8).
    await rowMenu(page, "Kept", "Remove");
    await header(page).getByRole("button", { name: "Delete menu" }).click();
    await expect(dialog).toContainText(unsaved);
    await dialog.getByRole("button", { name: "Delete menu" }).click();
    await expect(page).toHaveURL(/\/admin\/structure\/menus$/);
    expect(await api.getMenu(menu.name)).toBeNull();
    const toast = page.locator("[data-sonner-toast]").filter({ hasText: `${menu.label} deleted` });
    await toast.getByRole("button", { name: "Undo" }).click();
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: `${menu.label} is back` }),
    ).toBeVisible();
    expect((await api.getMenu(menu.name))?.items.map((i) => i.label)).toEqual(["Kept"]);
    await expect(page.locator(`[data-slot=menu-row][data-name=${menu.name}]`)).toBeVisible();
  } finally {
    await page.context().close();
    await api.deleteMenu(menu.name);
    await api.dispose();
  }
});

test("screens", async ({ as, site, backend, guard, snap, expectAccessible }) => {
  // The page entry's target is looked up type by type (posts first): a 404 per miss.
  guard.allowFailedResource(/\/api\/v1\/content\/[a-z0-9_-]+\/[0-9a-f-]+$/);
  const api = await ContentApi.as(backend, site.url, "admin");
  const s = short();
  const about = await api.createIn("page", `Our story ${s}`, "published");
  const draft = await api.createIn("page", `Opening hours ${s}`, "draft");
  const guides = link("Guides", "/guides/", null, 2);
  const menu = await api.putMenu(`social${s}`, {
    label: `Social links ${s}`,
    items: [
      { id: crypto.randomUUID(), kind: "item", target_id: about.id, label: "", position: 0 },
      link("Newsletter", "/newsletter/", null, 1),
      guides,
      link("Getting started", "/guides/start/", guides.id, 0),
      link(
        "A very long label that goes on and on to show how a row truncates it",
        "https://example.com/a/very/long/address/that/keeps/going",
        guides.id,
        1,
      ),
      link("Mastodon", "https://social.example/@site", null, 3),
    ],
  });
  const page = await as("admin");
  try {
    // The list: locations first, then the other menus.
    await page.goto("/admin/structure/menus");
    await expect(page.locator(`[data-slot=menu-row][data-name=${menu.name}]`)).toBeVisible();
    await expect(page.locator("[data-slot=menu-location]")).toHaveCount(2);
    await snap(page, "menus-list");
    await expectAccessible(page);

    // New menu: the name follows the label.
    await header(page).getByRole("button", { name: "New menu" }).click();
    const dialog = page.locator("[data-slot=new-menu-dialog]");
    await dialog.getByRole("textbox", { name: "Label" }).fill(`Legal ${s}`);
    await expect(dialog.getByRole("textbox", { name: "Name" })).toHaveValue(`legal-${s}`);
    await snap(page, "menus-new-dialog", { fullPage: false });
    await expectAccessible(page);
    await dialog.getByRole("button", { name: "Create menu" }).click();

    // The new, empty editor.
    await expect(page).toHaveURL(new RegExp(`/admin/structure/menus/legal-${s}$`));
    await expect(page.locator("[data-slot=menu-items] [data-slot=empty-state]")).toBeVisible();
    await expect(addPanel(page).getByRole("checkbox").first()).toBeVisible();
    await snap(page, "menus-editor-empty");
    await expectAccessible(page);

    // A filled editor with a draft change, and the Add items tabs.
    await openMenu(page, menu.name);
    await expect
      .poll(() => shape(tree(page)))
      .toEqual([
        `1:Our story ${s}`,
        "1:Newsletter",
        "1:Guides",
        "2:Getting started",
        "2:A very long label that goes on and on to show how a row truncates it",
        "1:Mastodon",
      ]);
    await addPanel(page).getByRole("searchbox", { name: "Search content" }).fill(s);
    await addPanel(page)
      .getByRole("checkbox", { name: `Opening hours ${s}` })
      .check();
    await snap(page, "menus-editor-filled");
    await expectAccessible(page);
    await addPanel(page).getByRole("tab", { name: "Terms" }).click();
    await expect(addPanel(page).getByRole("checkbox").first()).toBeVisible();
    await snap(page, "menus-add-terms", {
      prepare: async () => addPanel(page).scrollIntoViewIfNeeded(),
      fullPage: false,
    });
    await expectAccessible(page);
    await addPanel(page).getByRole("tab", { name: "Custom link" }).click();
    await addPanel(page).getByRole("textbox", { name: "Address" }).fill("not a link");
    await addPanel(page).getByRole("button", { name: "Add link" }).click();
    await snap(page, "menus-add-link", {
      prepare: async () => addPanel(page).scrollIntoViewIfNeeded(),
      fullPage: false,
    });
    await expectAccessible(page);

    // A row's menu and the edit dialog.
    await page.getByRole("button", { name: "Actions for Guides", exact: true }).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await snap(page, "menus-row-menu", {
      prepare: async () => page.getByRole("menu").scrollIntoViewIfNeeded(),
      fullPage: false,
    });
    await expectAccessible(page);
    await page.getByRole("menuitem", { name: "Edit" }).click();
    await expect(page.locator("[data-slot=menu-item-dialog]")).toBeVisible();
    await snap(page, "menus-item-dialog", { fullPage: false });
    await expectAccessible(page);
    await page.keyboard.press("Escape");
    await expect(page.locator("[data-slot=menu-item-dialog]")).toHaveCount(0);
  } finally {
    await page.context().close();
    await api.deleteMenu(menu.name);
    await api.deleteMenu(`legal-${s}`);
    await api.purge(about);
    await api.purge(draft);
    await api.dispose();
  }
});
