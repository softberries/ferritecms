// Taxonomies and their terms under the real server (M2b-2 Task 10; 07 §3 Taxonomies): an
// editor builds a category tree (New term, Move to…, a keyboard drag; the tree survives a
// reload; a term with sub-terms is not deletable); a move that would form a cycle (the
// tree was stale) is refused, rolled back and read again; a flat taxonomy's table and its
// filter; an admin creates a taxonomy for a custom type and the editor offers it; the
// palette's New taxonomy; and the review screens. Every taxonomy, term and type is
// created by the test itself (E2E data hygiene) and removed at the end.

import type { Locator, Page } from "@playwright/test";
import { ContentApi, type Term } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { mod } from "./support/keys";
import { uid } from "./support/uid";

const header = (page: Page) => page.locator("[data-slot=page-header]");
const sheet = (page: Page) => page.locator("[data-slot=term-sheet]");
const moveDialog = (page: Page) => page.locator("[data-slot=move-term-dialog]");
const deleteDialog = (page: Page) => page.locator("[data-slot=delete-term-dialog]");
const tree = (page: Page, label: string) => page.getByRole("list", { name: label, exact: true });
/** Every term of the tree as `level:name`, top to bottom. */
const shape = (list: Locator) =>
  list
    .locator("[data-sortable-row]")
    .evaluateAll((rows) =>
      rows.map(
        (r) =>
          `${r.getAttribute("aria-level")}:${r.querySelector("[data-slot=term-name]")?.textContent ?? ""}`,
      ),
    );

/** Full-page shots: no focused ring, the window at the top. */
const FULL_PAGE = (page: Page) => ({
  prepare: async () => {
    await page.evaluate(() => {
      const active = document.activeElement;
      if (active instanceof HTMLElement) active.blur();
      window.scrollTo(0, 0);
    });
  },
});

/** A short unique suffix that fits a taxonomy name. */
const short = () =>
  uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-8);

async function newTaxonomy(
  api: ContentApi,
  hierarchical: boolean,
  prefix: string,
  label: string,
): Promise<{ name: string; label: string }> {
  const s = short();
  const tax = { name: `${prefix}${s}`, label: `${label} ${s}` };
  await api.createTaxonomy(tax.name, tax.label, ["post"], hierarchical);
  return tax;
}

async function openTaxonomy(page: Page, name: string): Promise<void> {
  await page.goto(`/admin/structure/taxonomies/${name}`);
  await expect(page.locator("[data-slot=taxonomy-screen]")).toBeVisible();
}

/** New term from the header: the sheet, a name, Create term; waits for the row. */
async function addTerm(page: Page, name: string): Promise<void> {
  await header(page).getByRole("button", { name: "New term" }).click();
  await expect(sheet(page).getByRole("textbox", { name: "Name" })).toBeFocused();
  await page.keyboard.type(name);
  await sheet(page).getByRole("button", { name: "Create term" }).click();
  await expect(sheet(page)).toHaveCount(0);
}

/** A row menu item, waiting for the menu to close again. */
async function rowMenu(page: Page, name: string, item: string): Promise<void> {
  await page.getByRole("button", { name: `Actions for ${name}`, exact: true }).click();
  await page.getByRole("menuitem", { name: item }).click();
  await expect(page.getByRole("menu")).toBeHidden();
}

/** Move to… `parent` (`null`: the top level) through the parent combobox. */
async function moveTo(page: Page, name: string, parent: string | null): Promise<void> {
  await rowMenu(page, name, "Move to…");
  const dialog = moveDialog(page);
  await dialog.getByRole("button", { name: /^Parent/ }).click();
  await page.getByRole("option", { name: parent ?? "Top level", exact: true }).click();
  const patched = page.waitForResponse(
    (r) => r.request().method() === "PATCH" && r.url().includes("/api/v1/terms/"),
  );
  await dialog.getByRole("button", { name: "Move", exact: true }).click();
  expect((await patched).status()).toBe(200);
  // Closed, and focus is back on the row's menu, where the keyboard user was.
  await expect(dialog).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: `Actions for ${name}`, exact: true }),
  ).toBeFocused();
}

const byName = (terms: Term[], name: string) => {
  const term = terms.find((t) => t.name === name);
  if (!term) throw new Error(`no term ${name}`);
  return term;
};

test("editor_builds_a_category_tree", async ({ as, site, backend }) => {
  test.setTimeout(120_000);
  const admin = await ContentApi.as(backend, site.url, "admin");
  const tax = await newTaxonomy(admin, true, "kd", "Kinds");
  const page = await as("editor");
  const list = tree(page, tax.label);
  try {
    // Editors reach the terms from the list, without the taxonomy's own controls.
    await page.goto("/admin/structure/taxonomies");
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Taxonomies");
    await expect(header(page).getByRole("button", { name: "New taxonomy" })).toHaveCount(0);
    await page.getByRole("table", { name: "Taxonomies" }).getByText(tax.label).click();
    await expect(page).toHaveURL(new RegExp(`/admin/structure/taxonomies/${tax.name}$`));
    await expect(header(page).getByRole("button", { name: "Settings" })).toHaveCount(0);
    await expect(page.locator("[data-slot=empty-state]")).toContainText("No terms yet");

    // Three terms; siblings are listed by name whatever the order they were made in.
    await addTerm(page, "Pears");
    await addTerm(page, "Fruit");
    await addTerm(page, "Apples");
    await expect.poll(() => shape(list)).toEqual(["1:Apples", "1:Fruit", "1:Pears"]);

    // Move to… nests Apples under Fruit (the keyboard path to any level).
    await moveTo(page, "Apples", "Fruit");
    await expect.poll(() => shape(list)).toEqual(["1:Fruit", "2:Apples", "1:Pears"]);

    // A keyboard drag: Pears up, beside Fruit's children, lands under Fruit.
    const patched = page.waitForResponse(
      (r) => r.request().method() === "PATCH" && r.url().includes("/api/v1/terms/"),
    );
    const handle = list.getByRole("button", { name: "Reorder Pears", exact: true });
    await handle.focus();
    await expect(handle).toBeFocused();
    await page.keyboard.press("Space");
    await expect(page.locator("body")).toContainText("Picked up Pears");
    // dnd-kit's keyboard sensor listens for the arrows from a timer after the pick-up;
    // a timer queued now runs after it.
    await page.evaluate(() => new Promise((r) => setTimeout(r, 0)));
    await page.keyboard.press("ArrowUp");
    await expect(page.locator("body")).toContainText("Pears is over position 2 of 3.");
    await page.keyboard.press("Space");
    await expect(page.locator("body")).toContainText("Pears is now under Fruit.");
    expect((await patched).status()).toBe(200);
    await expect.poll(() => shape(list)).toEqual(["1:Fruit", "2:Apples", "2:Pears"]);

    // The server has it, and a reload shows the same tree.
    const terms = await admin.listTerms(tax.name);
    const fruit = byName(terms, "Fruit");
    expect(byName(terms, "Apples").parent_id).toBe(fruit.id);
    expect(byName(terms, "Pears").parent_id).toBe(fruit.id);
    await page.reload();
    await expect.poll(() => shape(list)).toEqual(["1:Fruit", "2:Apples", "2:Pears"]);

    // A term with sub-terms cannot be deleted: the confirm says so and offers no delete.
    await rowMenu(page, "Fruit", "Delete");
    await expect(deleteDialog(page)).toContainText("Fruit has sub-terms");
    await expect(deleteDialog(page)).toContainText("Move its 2 sub-terms");
    await expect(deleteDialog(page).getByRole("button", { name: "Delete term" })).toHaveCount(0);
    await deleteDialog(page).getByRole("button", { name: "Close" }).click();
    await expect(
      page.getByRole("button", { name: "Actions for Fruit", exact: true }),
    ).toBeFocused();

    // Add sub-term opens the sheet with the parent set; a leaf deletes after a confirm.
    await rowMenu(page, "Fruit", "Add sub-term");
    await expect(sheet(page).getByRole("button", { name: /^Parent/ })).toContainText("Fruit");
    await expect(sheet(page).getByRole("textbox", { name: "Name" })).toBeFocused();
    await page.keyboard.type("Quince");
    await sheet(page).getByRole("button", { name: "Create term" }).click();
    await expect.poll(() => shape(list)).toEqual(["1:Fruit", "2:Apples", "2:Pears", "2:Quince"]);
    await rowMenu(page, "Quince", "Delete");
    await expect(deleteDialog(page)).toContainText("Items filed under it lose this term");
    await deleteDialog(page).getByRole("button", { name: "Delete term" }).click();
    await expect.poll(() => shape(list)).toEqual(["1:Fruit", "2:Apples", "2:Pears"]);
    expect((await admin.listTerms(tax.name)).map((t) => t.name)).not.toContain("Quince");
  } finally {
    await page.context().close();
    await admin.removeTaxonomy(tax.name);
    await admin.dispose();
  }
});

test("cycle_is_refused_and_rolled_back", async ({ as, site, backend, guard }) => {
  // The refused move is a 422 by design: the browser logs that failed load.
  guard.allowFailedResource(/\/api\/v1\/terms\/[0-9a-f-]+$/);
  const api = await ContentApi.as(backend, site.url, "admin");
  const tax = await newTaxonomy(api, true, "cy", "Cycles");
  const alpha = await api.createTerm(tax.name, "Alpha");
  const beta = await api.createTerm(tax.name, "Beta");
  const page = await as("admin");
  const list = tree(page, tax.label);
  try {
    await openTaxonomy(page, tax.name);
    await expect.poll(() => shape(list)).toEqual(["1:Alpha", "1:Beta"]);
    // Someone else puts Alpha under Beta; this tree has not seen it.
    await api.patchTerm(alpha.id, { parent_id: beta.id });

    // Drag Beta one level in, under Alpha: on the server that is a cycle.
    const handle = await list
      .getByRole("button", { name: "Reorder Beta", exact: true })
      .boundingBox();
    if (!handle) throw new Error("no layout");
    const [x, y] = [handle.x + handle.width / 2, handle.y + handle.height / 2];
    const patched = page.waitForResponse(
      (r) => r.request().method() === "PATCH" && r.url().endsWith(`/api/v1/terms/${beta.id}`),
    );
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + 6, y, { steps: 2 });
    await page.mouse.move(x + 30, y, { steps: 6 });
    await expect(list.locator("[data-placeholder]")).toHaveCSS("margin-inline-start", "24px");
    await page.mouse.up();
    expect((await patched).status()).toBe(422);

    // The move is undone with the server's reason, and the tree is read again.
    const toast = page.locator("[data-sonner-toast]").filter({ hasText: "Beta was not moved" });
    await expect(toast).toContainText("Alpha is now under Beta, so Beta cannot go under it");
    await expect.poll(() => shape(list)).toEqual(["1:Beta", "2:Alpha"]);
    expect((await api.listTerms(tax.name)).find((t) => t.id === beta.id)?.parent_id ?? null).toBe(
      null,
    );
  } finally {
    await page.context().close();
    await api.removeTaxonomy(tax.name);
    await api.dispose();
  }
});

test("flat_tags_table_filter", async ({ as, site, backend }) => {
  const admin = await ContentApi.as(backend, site.url, "admin");
  const tax = await newTaxonomy(admin, false, "lb", "Labels");
  await admin.createTerm(tax.name, "Sourdough");
  await admin.createTerm(tax.name, "Rye bread");
  await admin.createTerm(tax.name, "Croissant", { description: "Laminated pastry" });
  const page = await as("editor");
  const table = page.getByRole("table", { name: tax.label });
  const names = () =>
    table
      .locator("tbody tr")
      .evaluateAll((rows) => rows.map((r) => r.querySelector("td span")?.textContent ?? ""));
  const filter = page.getByRole("searchbox", { name: `Filter ${tax.label}` });
  try {
    await openTaxonomy(page, tax.name);
    await expect.poll(names).toEqual(["Croissant", "Rye bread", "Sourdough"]);

    // The filter runs over names, slugs and descriptions, case aside.
    await filter.fill("BREAD");
    await expect.poll(names).toEqual(["Rye bread"]);
    await expect(page.getByRole("status").filter({ hasText: "Showing 1 of 3" })).toBeVisible();
    await filter.fill("pastry");
    await expect.poll(names).toEqual(["Croissant"]);
    await filter.fill("nothing-like-this");
    await expect(page.locator("[data-slot=empty-state]")).toContainText(
      "No terms match “nothing-like-this”",
    );
    await page
      .locator("[data-slot=empty-state]")
      .getByRole("button", { name: "Clear the filter" })
      .click();
    await expect.poll(names).toEqual(["Croissant", "Rye bread", "Sourdough"]);

    // A new tag: its slug is derived from the name; no parent in a flat taxonomy.
    await header(page).getByRole("button", { name: "New term" }).click();
    await expect(sheet(page).getByRole("button", { name: /^Parent/ })).toHaveCount(0);
    await sheet(page).getByRole("textbox", { name: "Name" }).fill("Bagel");
    await sheet(page).getByRole("button", { name: "Create term" }).click();
    await expect.poll(names).toEqual(["Bagel", "Croissant", "Rye bread", "Sourdough"]);
    const bagel = byName(await admin.listTerms(tax.name), "Bagel");
    expect(bagel.slug).toBe("bagel");

    // A row opens its sheet; the save goes through the stale guard and lands.
    await table.getByRole("row", { name: /Bagel/ }).click();
    await expect(sheet(page).getByRole("textbox", { name: "Slug" })).toHaveValue("bagel");
    await sheet(page).getByRole("textbox", { name: "Description" }).fill("Boiled, then baked");
    await sheet(page).getByRole("button", { name: "Save changes" }).click();
    await expect(sheet(page)).toHaveCount(0);
    expect(byName(await admin.listTerms(tax.name), "Bagel").description).toBe("Boiled, then baked");
  } finally {
    await page.context().close();
    await admin.removeTaxonomy(tax.name);
    await admin.dispose();
  }
});

test("term_stale_guard", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const tax = await newTaxonomy(api, false, "sg", "Stale");
  const term = await api.createTerm(tax.name, "Original");
  const page = await as("admin");
  const stale = page.locator("[data-slot=stale-dialog]");
  try {
    await openTaxonomy(page, tax.name);
    await page
      .getByRole("table", { name: tax.label })
      .getByRole("row", { name: /Original/ })
      .click();
    await sheet(page).getByRole("textbox", { name: "Name" }).fill("Mine");
    // Someone else renames it meanwhile.
    await api.patchTerm(term.id, { name: "Theirs" });
    await sheet(page).getByRole("button", { name: "Save changes" }).click();
    await expect(stale).toBeVisible();
    await stale.getByRole("button", { name: "Reload" }).click();
    await expect(sheet(page).getByRole("textbox", { name: "Name" })).toHaveValue("Theirs");
    expect(byName(await api.listTerms(tax.name), "Theirs").id).toBe(term.id);
    // Overwrite writes.
    await sheet(page).getByRole("textbox", { name: "Name" }).fill("Mine");
    await api.patchTerm(term.id, { description: "changed again" });
    await sheet(page).getByRole("button", { name: "Save changes" }).click();
    await stale.getByRole("button", { name: "Overwrite" }).click();
    await expect(sheet(page)).toHaveCount(0);
    expect(byName(await api.listTerms(tax.name), "Mine").id).toBe(term.id);
  } finally {
    await page.context().close();
    await api.removeTaxonomy(tax.name);
    await api.dispose();
  }
});

test("settings_saved_then_reopened_save_without_a_stale_prompt", async ({ as, site, backend }) => {
  // The dialog's next opening starts from the saved taxonomy (final review B7).
  const api = await ContentApi.as(backend, site.url, "admin");
  const tax = await newTaxonomy(api, false, "rs", "Resave");
  const page = await as("admin");
  const dialog = page.locator("[data-slot=taxonomy-settings-dialog]");
  try {
    await openTaxonomy(page, tax.name);
    for (const label of [`First ${uid()}`, `Second ${uid()}`]) {
      await header(page).getByRole("button", { name: "Settings" }).click();
      await dialog.getByRole("textbox", { name: "Label" }).fill(label);
      await dialog.getByRole("button", { name: "Save changes" }).click();
      await expect(dialog).toHaveCount(0);
      await expect(page.locator("[data-slot=stale-dialog]")).toHaveCount(0);
      await expect(header(page)).toContainText(label);
    }
  } finally {
    await page.context().close();
    await api.removeTaxonomy(tax.name);
    await api.dispose();
  }
});

test("admin_creates_taxonomy_for_a_custom_type", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const s = short();
  const type = { name: `bk${s}`, label: `Books ${s}` };
  await api.createType(type.name, type.label);
  const label = `Genres ${s}`;
  const name = `genres_${s}`;
  const page = await as("admin");
  try {
    await page.goto("/admin/structure/taxonomies");
    await header(page).getByRole("button", { name: "New taxonomy" }).click();
    const dialog = page.locator("[data-slot=new-taxonomy-dialog]");
    await dialog.getByRole("textbox", { name: "Label" }).fill(label);
    // The name follows the label until it is typed.
    await expect(dialog.getByRole("textbox", { name: "Name" })).toHaveValue(name);
    await dialog.getByRole("switch", { name: "Terms can have a parent" }).click();
    await dialog.getByRole("checkbox", { name: new RegExp(`^${type.label}`) }).check();
    await dialog.getByRole("button", { name: "Create taxonomy" }).click();
    await expect(page).toHaveURL(new RegExp(`/admin/structure/taxonomies/${name}$`));
    await expect(header(page)).toContainText(`Applies to ${type.label}`);

    // The editor offers the new taxonomy for items of the type.
    await page.goto(`/admin/content/${type.name}/new`);
    await expect(
      page.locator(`[data-slot=editor-panel] [data-slot=panel-terms][data-taxonomy=${name}]`),
    ).toContainText(label);
  } finally {
    await page.context().close();
    await api.removeType(type.name, [name]);
    await api.dispose();
  }
});

test("palette_opens_new_taxonomy", async ({ as }) => {
  const page = await as("admin");
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
  await page.keyboard.press(`${await mod(page)}+k`);
  await expect(page.locator("[data-slot=palette-input]")).toBeFocused();
  await page.keyboard.type("new taxonomy");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/admin\/structure\/taxonomies\?new=true$/);
  await expect(page.locator("[data-slot=new-taxonomy-dialog]")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page).toHaveURL(/\/admin\/structure\/taxonomies$/);
});

test("screens", async ({ as, site, backend, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  const api = await ContentApi.as(backend, site.url, "admin");
  const cats = await newTaxonomy(api, true, "sc", "Topics");
  const tags = await newTaxonomy(api, false, "st", "Keywords");
  const travel = await api.createTerm(cats.name, "Travel");
  const europe = await api.createTerm(cats.name, "Europe", { parent_id: travel.id });
  await api.createTerm(cats.name, "Portugal", { parent_id: europe.id });
  await api.createTerm(cats.name, "Asia", { parent_id: travel.id });
  await api.createTerm(cats.name, "Food", { description: "Recipes and restaurants" });
  await api.createTerm(cats.name, "Photography");
  for (const [n, d] of [
    ["Beginner", "For first-timers"],
    ["Budget", ""],
    ["Long read", "More than ten minutes"],
    ["Interview", ""],
  ] as const) {
    await api.createTerm(tags.name, n, d ? { description: d } : {});
  }
  const page = await as("admin");
  try {
    // The list, with lazily loaded term counts.
    await page.goto("/admin/structure/taxonomies");
    const table = page.getByRole("table", { name: "Taxonomies" });
    await expect(table.getByRole("row", { name: new RegExp(cats.label) })).toContainText("6 terms");
    await expect(table.getByRole("row", { name: new RegExp(tags.label) })).toContainText("4 terms");
    await snap(page, "taxonomies-list");
    await expectAccessible(page);

    await header(page).getByRole("button", { name: "New taxonomy" }).click();
    await page
      .locator("[data-slot=new-taxonomy-dialog]")
      .getByRole("textbox", { name: "Label" })
      .fill("Venues");
    await snap(page, "taxonomies-new-dialog", { fullPage: false });
    await expectAccessible(page);
    await page.keyboard.press("Escape");

    // The tree, then its row menu.
    await openTaxonomy(page, cats.name);
    const list = tree(page, cats.label);
    await expect
      .poll(() => shape(list))
      .toEqual(["1:Food", "1:Photography", "1:Travel", "2:Asia", "2:Europe", "3:Portugal"]);
    await snap(page, "taxonomies-tree", FULL_PAGE(page));
    await expectAccessible(page);
    await page.getByRole("button", { name: "Actions for Europe", exact: true }).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await snap(page, "taxonomies-tree-menu", { fullPage: false });
    await expectAccessible(page);
    await page.getByRole("menuitem", { name: "Move to…" }).click();
    await moveDialog(page)
      .getByRole("button", { name: /^Parent/ })
      .click();
    await expect(page.getByRole("option", { name: "Portugal" })).toHaveCount(0);
    await snap(page, "taxonomies-move-to", { fullPage: false });
    await expectAccessible(page);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("listbox")).toHaveCount(0);
    await moveDialog(page).getByRole("button", { name: "Cancel" }).click();
    await expect(moveDialog(page)).toHaveCount(0);

    // The term sheet (edit) and the blocked delete.
    await list.locator("[data-slot=term-name]", { hasText: "Europe" }).click();
    await expect(sheet(page).getByRole("textbox", { name: "Name" })).toHaveValue("Europe");
    await snap(page, "taxonomies-term-sheet", { fullPage: false });
    await expectAccessible(page);
    await page.keyboard.press("Escape");
    await expect(sheet(page)).toHaveCount(0);
    await rowMenu(page, "Travel", "Delete");
    await expect(deleteDialog(page)).toContainText("Move its 2 sub-terms");
    await snap(page, "taxonomies-delete-blocked", { fullPage: false });
    await expectAccessible(page);
    await page.keyboard.press("Escape");

    // The taxonomy's settings.
    await header(page).getByRole("button", { name: "Settings" }).click();
    await expect(page.locator("[data-slot=taxonomy-settings-dialog]")).toBeVisible();
    await snap(page, "taxonomies-settings", { fullPage: false });
    await expectAccessible(page);
    await page.keyboard.press("Escape");

    // The flat taxonomy's table, filtered.
    await openTaxonomy(page, tags.name);
    await expect(page.getByRole("table", { name: tags.label }).locator("tbody tr")).toHaveCount(4);
    await snap(page, "taxonomies-tags", FULL_PAGE(page));
    await expectAccessible(page);
    await page.getByRole("searchbox", { name: `Filter ${tags.label}` }).fill("b");
    await expect(page.getByRole("table", { name: tags.label }).locator("tbody tr")).toHaveCount(2);
    await snap(page, "taxonomies-tags-filtered", FULL_PAGE(page));
    await expectAccessible(page);

    // An empty taxonomy.
    const empty = await newTaxonomy(api, true, "se", "Empty");
    try {
      await openTaxonomy(page, empty.name);
      await expect(page.locator("[data-slot=empty-state]")).toBeVisible();
      await snap(page, "taxonomies-empty");
      await expectAccessible(page);
    } finally {
      await api.removeTaxonomy(empty.name);
    }
  } finally {
    await page.context().close();
    await api.removeTaxonomy(cats.name);
    await api.removeTaxonomy(tags.name);
    await api.dispose();
  }
});
