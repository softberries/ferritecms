// Content types and the field builder under the real server (M2b-2 Task 9; 07 §3
// Content types): a type created from the list with fields, then an item written with
// them; fields reordered from the keyboard; a removed field warned about before the save
// while items still save (Review Focus 2); someone else's change caught by the stale
// guard (Review Focus 4); deleting a type with content (or a built-in) refused with the
// server's reason; no Content types for editors; and the review screens. Every type and
// item is created by the test itself (E2E data hygiene) and removed at the end.

import type { Locator, Page } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { mod } from "./support/keys";
import { uid } from "./support/uid";

interface FieldDef {
  name: string;
  label?: string;
  kind: string;
  required?: boolean;
  options?: string[];
  fields?: FieldDef[];
  types?: string[];
}
interface TypeDto {
  name: string;
  label: string;
  has_archive: boolean;
  fields: FieldDef[] | null;
}
type Stored = Item & { fields: Record<string, unknown> };

const SCHEMA: FieldDef[] = [
  { name: "subtitle", label: "Subtitle", kind: "text", required: true },
  { name: "blurb", label: "Blurb", kind: "rich_text" },
  { name: "price", label: "Price", kind: "number" },
  { name: "release", label: "Release", kind: "date" },
  { name: "in_print", label: "In print", kind: "bool" },
  { name: "genre", label: "Genre", kind: "select", options: ["fiction", "essay", "poetry"] },
  { name: "related", label: "Related", kind: "relation" },
  { name: "cover", label: "Cover", kind: "media" },
  {
    name: "chapters",
    label: "Chapters",
    kind: "repeater",
    fields: [
      { name: "title", label: "Chapter title", kind: "text", required: true },
      {
        name: "notes",
        label: "Notes",
        kind: "repeater",
        fields: [{ name: "note", label: "Note", kind: "text" }],
      },
    ],
  },
];

const nav = (page: Page) => page.getByRole("navigation", { name: "Main" });
const header = (page: Page) => page.locator("[data-slot=page-header]");
const saveButton = (page: Page) => header(page).getByRole("button", { name: "Save changes" });
const saveState = (page: Page) => header(page).locator("[data-slot=save-state]");
const fieldsCard = (page: Page) => page.locator("[data-slot=type-fields]");
const settings = (page: Page) => page.locator("[data-slot=type-settings]");
const rows = (page: Page) => fieldsCard(page).locator("[data-slot=builder-row]");
const row = (scope: Page | Locator, name: string) =>
  scope.locator(`[data-slot=builder-row][data-name="${name}"]`).first();
/** The row's own summary button (opens and closes its editor). */
const summary = (r: Locator) => r.locator(":scope > button[aria-expanded]");
const changeDialog = (page: Page) => page.locator("[data-slot=schema-change-dialog]");
const staleDialog = (page: Page) => page.locator("[data-slot=stale-dialog]");

/** Full-page shots: no focused skip link or ring, the window at the top. */
const FULL_PAGE = (page: Page) => ({
  prepare: async () => {
    await page.evaluate(() => {
      const active = document.activeElement;
      if (active instanceof HTMLElement) active.blur();
      window.scrollTo(0, 0);
    });
  },
});

/** A short unique suffix that fits a type name. */
const short = () =>
  uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-8);

async function newType(
  api: ContentApi,
  fields: FieldDef[],
  prefix = "tp",
): Promise<{ name: string; label: string }> {
  const s = short();
  const t = { name: `${prefix}${s}`, label: `Types ${s}` };
  await api.createType(t.name, t.label, false, fields);
  return t;
}

async function getTypeDto(api: ContentApi, name: string): Promise<TypeDto> {
  const res = await api.ctx.get(`/api/v1/types/${name}`);
  expect(res.status()).toBe(200);
  return (await res.json()) as TypeDto;
}

async function openType(page: Page, name: string): Promise<void> {
  await page.goto(`/admin/structure/types/${name}`);
  await expect(page.locator("[data-slot=type-screen]")).toBeVisible();
}

/** The ⌘K palette over the dashboard, its field focused. */
async function openPalette(page: Page): Promise<void> {
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
  await page.keyboard.press(`${await mod(page)}+k`);
  await expect(page.locator("[data-slot=palette-input]")).toBeFocused();
}

async function save(page: Page): Promise<void> {
  await saveButton(page).click();
  await expect(saveState(page)).toHaveAttribute("data-state", "saved");
}

test("create_type_with_fields_then_write_an_item", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const page = await as("admin");
  const s = short();
  const label = `Recipes ${s}`;
  const name = `recipes_${s}`;
  try {
    await page.goto("/admin/structure/types");
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Content types");
    await header(page).getByRole("button", { name: "New type" }).click();
    const dialog = page.locator("[data-slot=new-type-dialog]");
    await dialog.getByRole("textbox", { name: "Label" }).fill(label);
    // The name follows the label until it is typed.
    await expect(dialog.getByRole("textbox", { name: "Name" })).toHaveValue(name);
    await dialog.getByRole("switch", { name: "Archive page" }).click();
    await dialog.getByRole("button", { name: "Create type" }).click();
    await expect(page).toHaveURL(new RegExp(`/admin/structure/types/${name}$`));
    // The new type is in the nav at once.
    await expect(nav(page).getByRole("link", { name: label })).toBeVisible();

    // A number field: the cursor lands in the new field's label; the name follows it.
    await fieldsCard(page).getByRole("button", { name: "Add field" }).click();
    let editor = fieldsCard(page).locator("[data-slot=field-editor]").last();
    await expect(editor.getByRole("textbox", { name: "Label" })).toBeFocused();
    await page.keyboard.type("Servings");
    await expect(editor.getByRole("textbox", { name: "Name" })).toHaveValue("servings");
    await editor.getByRole("combobox", { name: "Kind" }).click();
    await page.getByRole("option", { name: "Number" }).click();
    await editor.getByRole("button", { name: "Done" }).click();
    // A choice with two options (Enter adds the next).
    await fieldsCard(page).getByRole("button", { name: "Add field" }).click();
    editor = fieldsCard(page).locator("[data-slot=field-editor]").last();
    await expect(editor.getByRole("textbox", { name: "Label" })).toBeFocused();
    await page.keyboard.type("Cuisine");
    await editor.getByRole("combobox", { name: "Kind" }).click();
    await page.getByRole("option", { name: "Choice" }).click();
    await editor.getByRole("button", { name: "Add option" }).click();
    await expect(editor.getByRole("textbox", { name: "Option 1" })).toBeFocused();
    await page.keyboard.type("Italian");
    await page.keyboard.press("Enter");
    await expect(editor.getByRole("textbox", { name: "Option 2" })).toBeFocused();
    await page.keyboard.type("Thai");
    await save(page);

    const stored = await getTypeDto(api, name);
    expect(stored.has_archive).toBe(true);
    expect(stored.fields).toEqual([
      {
        name: "servings",
        label: "Servings",
        kind: "number",
        required: false,
        options: [],
        fields: [],
      },
      {
        name: "cuisine",
        label: "Cuisine",
        kind: "select",
        required: false,
        options: ["Italian", "Thai"],
        fields: [],
      },
    ]);

    // Write an item: the editor shows the new fields.
    await nav(page).getByRole("link", { name: label }).click();
    await expect(page).toHaveURL(new RegExp(`/admin/content/${name}$`));
    await page.goto(`/admin/content/${name}/new`);
    const title = page.getByRole("textbox", { name: "Title", exact: true });
    await expect(title).toBeFocused();
    const itemTitle = `Soup ${uid()}`;
    await page.keyboard.type(itemTitle);
    const panel = page.locator("[data-slot=editor-panel]");
    await panel.getByRole("textbox", { name: "Servings" }).fill("4");
    await panel.getByRole("combobox", { name: "Cuisine" }).click();
    await page.getByRole("option", { name: "Thai" }).click();
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(page).toHaveURL(new RegExp(`/admin/content/${name}/[0-9a-f-]{36}$`));
    const id = new URL(page.url()).pathname.split("/").at(-1) ?? "";
    await expect(page.locator("[data-slot=editor-bar] [data-slot=save-state]")).toHaveAttribute(
      "data-state",
      "saved",
    );
    const item = (await api.get(name, id)) as Stored;
    expect(item.title).toBe(itemTitle);
    expect(item.fields).toEqual({ servings: 4, cuisine: "Thai" });
  } finally {
    await page.context().close();
    await api.removeType(name);
    await api.dispose();
  }
});

test("reorder_fields_by_keyboard", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const type = await newType(api, [
    { name: "alpha", label: "Alpha", kind: "text" },
    { name: "beta", label: "Beta", kind: "number" },
    { name: "gamma", label: "Gamma", kind: "bool" },
  ]);
  const page = await as("admin");
  try {
    await openType(page, type.name);
    const list = fieldsCard(page).getByRole("list", { name: "Fields", exact: true });
    await list.getByRole("button", { name: "Reorder Alpha" }).focus();
    await page.keyboard.press("Space");
    await expect(page.locator("body")).toContainText("Picked up Alpha, position 1 of 3");
    await expect(list.locator("[data-dragging]")).toHaveCount(1);
    await page.evaluate(
      () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))),
    );
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("body")).toContainText("Alpha is over position 2 of 3");
    await page.keyboard.press("Space");
    await expect(page.locator("body")).toContainText("Alpha dropped at position 2 of 3");
    await expect(rows(page)).toHaveCount(3);
    expect(
      await rows(page).evaluateAll((els) => els.map((e) => e.getAttribute("data-name"))),
    ).toEqual(["beta", "alpha", "gamma"]);
    // Reordering alone changes nothing for stored items: no warning before the save.
    await save(page);
    await expect(changeDialog(page)).toHaveCount(0);
    expect((await getTypeDto(api, type.name)).fields?.map((f) => f.name)).toEqual([
      "beta",
      "alpha",
      "gamma",
    ]);
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.dispose();
  }
});

test("removing_a_field_warns_and_items_still_save", async ({
  as,
  site,
  siteKey,
  snap,
  expectAccessible,
}) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const type = await newType(api, [
    { name: "subtitle", label: "Subtitle", kind: "text", required: true },
    { name: "genre", label: "Genre", kind: "select", options: ["fiction", "essay"] },
  ]);
  const item = await api.create(type.name, `Book ${uid()}`, {
    fields: { subtitle: "Kept", genre: "fiction" },
  });
  const page = await as("admin");
  try {
    await openType(page, type.name);
    const genre = row(fieldsCard(page), "genre");
    await summary(genre).click();
    await genre.getByRole("button", { name: "Remove field" }).click();
    await expect(rows(page)).toHaveCount(1);
    await saveButton(page).click();

    // The warning names the field and says what happens to stored values.
    const dialog = changeDialog(page);
    await expect(dialog).toBeVisible();
    await expect(dialog.locator("[data-change=removed]")).toContainText("genre");
    await expect(dialog).toContainText(
      "existing values stay stored until each item is saved, and are then removed",
    );
    // The last chip is whole: with the list scrolled to its end, the chip lies inside the
    // list's visible box (its client area), so no edge is clipped (M2b-2 minor).
    const clipped = await dialog
      .locator("code")
      .last()
      .evaluate((chip) => {
        const scroller = chip.closest("section")?.parentElement;
        if (!scroller) return null;
        scroller.scrollTop = scroller.scrollHeight;
        const box = scroller.getBoundingClientRect();
        const top = box.top + scroller.clientTop;
        const bottom = top + scroller.clientHeight;
        const r = chip.getBoundingClientRect();
        return { above: top - r.top, below: r.bottom - bottom };
      });
    expect(clipped).not.toBeNull();
    expect(clipped?.above).toBeLessThanOrEqual(0);
    expect(clipped?.below).toBeLessThanOrEqual(0);
    await snap(page, "types-schema-change-removed", { fullPage: false });
    await expectAccessible(page);
    await dialog.getByRole("button", { name: "Save changes" }).click();
    await expect(saveState(page)).toHaveAttribute("data-state", "saved");
    expect((await getTypeDto(api, type.name)).fields?.map((f) => f.name)).toEqual(["subtitle"]);
    // Still stored on the item until it is saved.
    expect(((await api.get(type.name, item.id)) as Stored).fields.genre).toBe("fiction");

    // The item still saves: the editor lists the stale value and drops it.
    await page.goto(`/admin/content/${type.name}/${item.id}`);
    const title = page.getByRole("textbox", { name: "Title", exact: true });
    await expect(title).toBeVisible();
    await expect(page.locator("[data-slot=stale-keys]")).toContainText(
      "Not in the type any more: genre, removed when you save",
    );
    await title.fill(`${item.title} (saved)`);
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(page.locator("[data-slot=editor-bar] [data-slot=save-state]")).toHaveAttribute(
      "data-state",
      "saved",
    );
    const after = (await api.get(type.name, item.id)) as Stored;
    expect(after.title).toBe(`${item.title} (saved)`);
    expect(after.fields).toEqual({ subtitle: "Kept" });
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.dispose();
  }
});

test("relation_limited_to_types", async ({ as, site, siteKey, snap, expectAccessible }) => {
  // P52: the builder sets a relation's target types (here: pages only), the change says
  // that stored values may now be refused, and the editor's picker searches only pages.
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const type = await newType(api, [{ name: "related", label: "Related", kind: "relation" }]);
  const token = uid("shelf");
  const post = await api.createIn("post", `Post ${token}`, "published");
  const target = await api.createIn("page", `Page ${token}`, "published");
  const item = await api.create(type.name, `Book ${uid()}`);
  const page = await as("admin");
  try {
    await openType(page, type.name);
    const related = row(fieldsCard(page), "related");
    await summary(related).click();
    const targets = related.getByRole("group", { name: "Can point at" });
    await expect(targets).toBeVisible();
    // Every content type but media.
    await expect(targets.getByRole("checkbox", { name: /Media/ })).toHaveCount(0);
    await targets.getByRole("checkbox", { name: /Pages/ }).click();
    await snap(page, "types-relation-targets", FULL_PAGE(page));
    await expectAccessible(page);
    await saveButton(page).click();
    const dialog = changeDialog(page);
    await expect(dialog.locator("[data-change=targetsNarrowed]")).toContainText("related");
    await expect(dialog.locator("[data-change=targetsNarrowed]")).toContainText(
      "cannot be saved until the relation is changed",
    );
    await snap(page, "types-schema-change-narrowed", { fullPage: false });
    await expectAccessible(page);
    await dialog.getByRole("button", { name: "Save changes" }).click();
    await expect(saveState(page)).toHaveAttribute("data-state", "saved");
    expect((await getTypeDto(api, type.name)).fields?.[0]?.types).toEqual(["page"]);

    // The editor's picker searches pages only.
    const searched: string[] = [];
    page.on("request", (r) => {
      const url = new URL(r.url());
      if (url.searchParams.get("q") === token) searched.push(url.pathname);
    });
    await page.goto(`/admin/content/${type.name}/${item.id}`);
    await expect(page.getByRole("textbox", { name: "Title", exact: true })).toBeVisible();
    const trigger = page.locator("[data-slot=relation-trigger]");
    await trigger.click();
    const answered = page.waitForResponse((r) => new URL(r.url()).searchParams.get("q") === token);
    // The search names the one type it covers.
    await page.getByPlaceholder("Search Pages…").fill(token);
    await answered;
    const option = page.getByRole("option", { name: new RegExp(target.title) });
    await expect(option).toBeVisible();
    await expect(page.getByRole("option", { name: new RegExp(post.title) })).toHaveCount(0);
    expect(searched).toEqual(["/api/v1/content/page"]);
    await expectAccessible(page);
    await option.click();
    await expect(trigger).toContainText(target.title);
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(page.locator("[data-slot=editor-bar] [data-slot=save-state]")).toHaveAttribute(
      "data-state",
      "saved",
    );
    expect(((await api.get(type.name, item.id)) as Stored).fields.related).toBe(target.id);
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.purge(post);
    await api.purge(target);
    await api.dispose();
  }
});

test("type_stale_guard", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const type = await newType(api, [{ name: "alpha", label: "Alpha", kind: "text" }]);
  const a = await as("admin");
  const b = await as("admin");
  const label = (page: Page) => settings(page).getByRole("textbox", { name: "Label" });
  const archive = (page: Page) => settings(page).getByRole("switch", { name: "Archive page" });
  try {
    await openType(a, type.name);
    await openType(b, type.name);

    // A saves; B's save asks, and Reload shows A's version without writing B's.
    await label(a).fill(`${type.label} by A`);
    await save(a);
    await archive(b).click();
    await saveButton(b).click();
    await expect(staleDialog(b)).toBeVisible();
    await expect(staleDialog(b)).toContainText("Changed by someone else");
    await staleDialog(b).getByRole("button", { name: "Reload" }).click();
    await expect(staleDialog(b)).toHaveCount(0);
    await expect(label(b)).toHaveValue(`${type.label} by A`);
    await expect(archive(b)).not.toBeChecked();
    let stored = await getTypeDto(api, type.name);
    expect([stored.label, stored.has_archive]).toEqual([`${type.label} by A`, false]);

    // A saves again; B's next save asks again, and only Overwrite writes.
    await label(a).fill(`${type.label} by A again`);
    await save(a);
    await archive(b).click();
    await saveButton(b).click();
    await expect(staleDialog(b)).toBeVisible();
    await staleDialog(b).getByRole("button", { name: "Overwrite" }).click();
    await expect(saveState(b)).toHaveAttribute("data-state", "saved");
    stored = await getTypeDto(api, type.name);
    expect([stored.label, stored.has_archive]).toEqual([`${type.label} by A`, true]);
  } finally {
    await a.context().close();
    await b.context().close();
    await api.removeType(type.name);
    await api.dispose();
  }
});

test("delete_type_with_content_is_refused_with_reason", async ({
  as,
  site,
  siteKey,
  guard,
  snap,
  expectAccessible,
}) => {
  // The refused deletes are 422s by design: the browser logs those failed loads.
  guard.allowFailedResource(/\/api\/v1\/types\/[a-z0-9_-]+$/);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const type = await newType(api, [{ name: "alpha", label: "Alpha", kind: "text" }]);
  const item = await api.create(type.name, `Kept ${uid()}`);
  const page = await as("admin");
  const danger = page.locator("[data-slot=type-danger]");
  const confirm = page.locator("[data-slot=delete-type-dialog]");
  try {
    await openType(page, type.name);
    await danger.getByRole("button", { name: "Delete type" }).click();
    await expect(confirm).toContainText("This cannot be undone");
    await confirm.getByRole("button", { name: "Delete type" }).click();
    await expect(danger.locator("[data-slot=delete-refused]")).toContainText(
      "It still has items (in some status, the trash included). Delete them first.",
    );
    await expect(page).toHaveURL(new RegExp(`/admin/structure/types/${type.name}$`));
    expect((await api.ctx.get(`/api/v1/types/${type.name}`)).status()).toBe(200);
    await snap(page, "types-delete-refused");
    await expectAccessible(page);

    // A built-in type: the confirm says it cannot be deleted and offers only Close.
    await openType(page, "post");
    await danger.getByRole("button", { name: "Delete type" }).click();
    await expect(confirm).toContainText("Posts cannot be deleted");
    await expect(confirm).toContainText("Built-in types cannot be deleted.");
    await expect(confirm.getByRole("button", { name: "Delete type" })).toHaveCount(0);
    await snap(page, "types-delete-built-in", { fullPage: false });
    await expectAccessible(page);
    await confirm.getByRole("button", { name: "Close" }).click();
    await expect(confirm).toHaveCount(0);
    expect((await api.ctx.get("/api/v1/types/post")).status()).toBe(200);

    // Without its items the type goes, and the list no longer has it.
    await api.purge(item);
    await openType(page, type.name);
    await danger.getByRole("button", { name: "Delete type" }).click();
    await confirm.getByRole("button", { name: "Delete type" }).click();
    await expect(page).toHaveURL(/\/admin\/structure\/types$/);
    await expect(page.getByRole("table", { name: "Content types" })).toBeVisible();
    await expect(page.getByRole("table", { name: "Content types" })).not.toContainText(type.label);
    await expect(nav(page).getByRole("link", { name: type.label })).toHaveCount(0);
    expect((await api.ctx.get(`/api/v1/types/${type.name}`)).status()).toBe(404);
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.dispose();
  }
});

test("editor_role_has_no_types_screen", async ({ as }) => {
  const page = await as("editor");
  await page.goto("/admin/");
  await expect(nav(page).getByRole("link", { name: "Media" })).toBeVisible();
  await expect(nav(page).getByRole("link", { name: "Content types" })).toHaveCount(0);
  // Reached by its address anyway: the screen says no (the server refuses the writes).
  await page.goto("/admin/structure/types");
  await expect(page.getByText("You don't have access to this")).toBeVisible();
  await expect(page.getByRole("button", { name: "New type" })).toHaveCount(0);
  await page.goto("/admin/structure/types/post");
  await expect(page.getByText("You don't have access to this")).toBeVisible();
  // Nor does the palette offer it.
  await openPalette(page);
  const palette = page.getByRole("dialog");
  await page.keyboard.type("content type");
  await expect(palette.getByRole("option", { name: "New content type" })).toHaveCount(0);
});

test("palette_opens_new_type", async ({ as }) => {
  const page = await as("admin");
  await openPalette(page);
  await page.keyboard.type("new content type");
  await page.getByRole("option", { name: "New content type" }).click();
  await expect(page).toHaveURL(/\/admin\/structure\/types\?new=true$/);
  await expect(page.locator("[data-slot=new-type-dialog]")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page).toHaveURL(/\/admin\/structure\/types$/);
});

test("screens", async ({ as, site, siteKey, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const type = await newType(api, SCHEMA, "bk");
  const page = await as("admin");
  try {
    // The list: built-ins first, then custom types.
    await page.goto("/admin/structure/types");
    const table = page.getByRole("table", { name: "Content types" });
    await expect(table.getByRole("row", { name: new RegExp(type.label) })).toContainText(
      "9 fields",
    );
    await expect(table).not.toContainText("media");
    await snap(page, "types-list");
    await expectAccessible(page);

    await header(page).getByRole("button", { name: "New type" }).click();
    const dialog = page.locator("[data-slot=new-type-dialog]");
    await dialog.getByRole("textbox", { name: "Label" }).fill("Events");
    await expect(dialog.getByRole("textbox", { name: "Name" })).toHaveValue("events");
    await snap(page, "types-new-dialog", { fullPage: false });
    await expectAccessible(page);
    await page.keyboard.press("Escape");

    // The type with its builder.
    await openType(page, type.name);
    await expect(rows(page)).toHaveCount(9);
    await snap(page, "types-builder");
    await expectAccessible(page);

    // A choice field open: its options as a sortable list.
    const genre = row(fieldsCard(page), "genre");
    await summary(genre).click();
    await expect(genre.locator("[data-slot=options-editor]")).toBeVisible();
    await expect(genre.getByRole("textbox", { name: "Option 3" })).toHaveValue("poetry");
    // A duplicate option is caught before the save.
    await genre.getByRole("textbox", { name: "Option 3" }).fill("essay");
    await saveButton(page).click();
    await expect(genre.locator("[data-slot=form-error]")).toHaveText(
      "This option is already listed",
    );
    await expect(genre.getByRole("textbox", { name: "Option 3" })).toBeFocused();
    // The "Fix the marked fields" toast would cover a row on a phone: it goes first.
    await expect(page.locator("[data-sonner-toast]")).toHaveCount(0, { timeout: 10_000 });
    await snap(page, "types-field-select", FULL_PAGE(page));
    await expectAccessible(page);
    await genre.getByRole("textbox", { name: "Option 3" }).fill("poetry");
    await summary(genre).click();

    // A repeater open, with its nested repeater open too.
    const chapters = row(fieldsCard(page), "chapters");
    await summary(chapters).click();
    const notes = row(chapters, "notes");
    await summary(notes).click();
    await expect(notes.locator("[data-slot=field-editor]")).toBeVisible();
    // No third level: a repeater inside Notes is not offered.
    await row(notes, "note").locator(":scope > button[aria-expanded]").click();
    await notes
      .locator("[data-slot=field-editor]")
      .last()
      .getByRole("combobox", { name: "Kind" })
      .click();
    await expect(page.getByRole("option", { name: "Repeater" })).toHaveAttribute(
      "data-disabled",
      "",
    );
    await page.keyboard.press("Escape");
    await summary(row(notes, "note")).click();
    await snap(page, "types-field-repeater", FULL_PAGE(page));
    await expectAccessible(page);

    // A kind change on a saved field warns in place.
    const price = row(fieldsCard(page), "price");
    await summary(price).click();
    await price.getByRole("combobox", { name: "Kind" }).click();
    await page.getByRole("option", { name: "Text", exact: true }).click();
    await expect(price.locator("[data-slot=kind-warning]")).toContainText(
      "existing values may no longer be valid",
    );

    // Every kind of change named before the save.
    const blurb = row(fieldsCard(page), "blurb");
    await summary(blurb).click();
    await blurb.getByRole("textbox", { name: "Name" }).fill("summary");
    const inPrint = row(fieldsCard(page), "in_print");
    await summary(inPrint).click();
    await inPrint.getByRole("switch", { name: "Required" }).click();
    const cover = row(fieldsCard(page), "cover");
    await summary(cover).click();
    await cover.getByRole("button", { name: "Remove field" }).click();
    await saveButton(page).click();
    const change = changeDialog(page);
    await expect(change.locator("[data-change=removed]")).toContainText("cover");
    await expect(change.locator("[data-change=renamed]")).toContainText("blurb → summary");
    await expect(change.locator("[data-change=newlyRequired]")).toContainText("in_print");
    await expect(change.locator("[data-change=kindChanged]")).toContainText("price");
    await snap(page, "types-schema-change", { fullPage: false });
    await expectAccessible(page);
    // Keep editing: nothing is written.
    await change.getByRole("button", { name: "Keep editing" }).click();
    await expect(change).toHaveCount(0);
    expect((await getTypeDto(api, type.name)).fields?.map((f) => f.name)).toContain("cover");
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.dispose();
  }
});
