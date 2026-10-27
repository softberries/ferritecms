// Custom fields and SEO in the editor under the real server (M2b-2 Task 8; 07 §3
// Editor, Review Focus 2): a type created over the API with every field kind, each input
// filled and still there after a reload; repeater rows reordered from the keyboard; a
// relation picking a post; SEO title and description reaching the published page's head;
// a field change by someone else in the 412 dialog; stored values a changed schema no
// longer has listed and dropped; and the review screens. Every type and item is created
// by the test itself (E2E data hygiene) and removed at the end.

import type { Page, Response } from "@playwright/test";
import { showSection } from "./support/editor-snap";
import { ContentApi, type Item } from "./support/content-api";
import { png } from "./support/files";
import { mod } from "./support/keys";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

type Stored = Item & {
  fields: Record<string, unknown>;
  permalink: string;
  seo: { title: string | null; description: string | null };
};

const SCHEMA = [
  { name: "subtitle", label: "Subtitle", kind: "text", required: true },
  { name: "blurb", label: "Blurb", kind: "rich_text" },
  { name: "price", label: "Price", kind: "number" },
  { name: "release", label: "Release", kind: "date" },
  { name: "in_print", label: "In print", kind: "bool" },
  { name: "genre", label: "Genre", kind: "select", options: ["fiction", "essay"] },
  // Posts only: a search across every type would also ask the types other tests create
  // and delete in parallel (a `404` race, m2b2-exit §2); `post` is never deleted.
  { name: "related", label: "Related", kind: "relation", types: ["post"] },
  { name: "cover", label: "Cover file", kind: "media" },
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

const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });
const panel = (page: Page) => page.locator("[data-slot=editor-panel]");
const card = (page: Page) => page.locator("[data-slot=fields-card]");
const picker = (page: Page) => page.locator("[data-slot=media-picker]");
const indicator = (page: Page) => page.locator("[data-slot=editor-bar] [data-slot=save-state]");
const conflict = (page: Page) => page.locator("[data-slot=conflict-dialog]");
const field = (scope: ReturnType<Page["locator"]>, path: string) =>
  scope.locator(`[data-slot=field][data-path="${path}"]`);

/** A fresh type with every kind (and its own label), for one test. */
async function bookType(api: ContentApi): Promise<{ name: string; label: string }> {
  const short = uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-8);
  const name = `bk${short}`;
  const label = `Books ${short}`;
  await api.createType(name, label, false, SCHEMA);
  return { name, label };
}

async function openItem(page: Page, type: string, id: string): Promise<void> {
  await page.goto(`/admin/content/${type}/${id}`);
  await expect(title(page)).toBeVisible();
  await expect(panel(page)).toBeVisible();
}

async function save(page: Page): Promise<void> {
  await page.keyboard.press(`${await mod(page)}+s`);
  await expect(indicator(page)).toHaveAttribute("data-state", "saved");
}

test("fill_every_field_kind_and_reload", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const type = await bookType(api);
  const post = await api.create("post", `Related ${uid()}`);
  const fileName = `cover-${uid()}`;
  const media = await api.upload(png(`${fileName}.png`, 320, 200), { title: fileName });
  const item = await api.create(type.name, `Book ${uid()}`, { fields: { subtitle: "First" } });
  const page = await as("admin");
  try {
    await openItem(page, type.name, item.id);
    // More than 4 fields: a card under the body on a wide screen.
    await expect(card(page)).toBeVisible();
    const c = card(page);

    await c.getByRole("textbox", { name: /^Subtitle/ }).fill("A subtitle");
    await c.getByRole("textbox", { name: "Blurb" }).click();
    await page.keyboard.type("A rich blurb.");
    // Only a point separates decimals: a comma is refused, never guessed at.
    await c.getByRole("textbox", { name: "Price" }).fill("12,5");
    await expect(field(c, "fields.price").locator("[data-slot=field-error]")).toHaveText(
      "Use a point (.) for decimals, without thousands separators",
    );
    await c.getByRole("textbox", { name: "Price" }).fill("12.5");
    // The date: a day in the calendar and a 24-hour time.
    await c.getByRole("button", { name: "Release" }).click();
    const calendar = page.locator("[data-slot=calendar]");
    await calendar.getByRole("button", { name: /15/ }).first().click();
    await page.getByLabel("Time").fill("10:30");
    await page.getByRole("button", { name: "Done" }).click();
    await c.getByRole("switch", { name: "In print" }).click();
    await c.getByRole("combobox", { name: "Genre" }).click();
    await page.getByRole("option", { name: "essay" }).click();
    await c.getByRole("button", { name: "Related" }).click();
    await page.getByPlaceholder("Search Posts…").fill(post.title);
    await page.getByRole("option", { name: new RegExp(post.title) }).click();
    // A `media` field takes any kind.
    await c.getByRole("button", { name: "Cover file" }).click();
    await expect(picker(page)).toBeVisible();
    await expect(picker(page).getByRole("heading", { name: "Choose a file" })).toBeVisible();
    await picker(page).getByRole("searchbox").fill(fileName);
    await picker(page).locator("[data-tile]").filter({ hasText: fileName }).dblclick();
    await expect(picker(page)).toHaveCount(0);
    // Repeater rows, nested.
    const chapters = field(c, "fields.chapters");
    await chapters.getByRole("button", { name: "Add row" }).last().click();
    await field(c, "fields.chapters[0].title").getByRole("textbox").fill("Opening");
    await field(c, "fields.chapters[0].notes").getByRole("button", { name: "Add row" }).click();
    await field(c, "fields.chapters[0].notes[0].note").getByRole("textbox").fill("A note");

    await save(page);
    const stored = (await api.get(type.name, item.id)) as Stored;
    expect(stored.fields).toMatchObject({
      subtitle: "A subtitle",
      price: 12.5,
      in_print: true,
      genre: "essay",
      related: post.id,
      cover: media.id,
      chapters: [{ title: "Opening", notes: [{ note: "A note" }] }],
    });
    expect(stored.fields.blurb).toMatchObject({ type: "doc" });
    expect(JSON.stringify(stored.fields.blurb)).toContain("A rich blurb.");
    expect(String(stored.fields.release)).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:00Z$/);

    await page.reload();
    await expect(card(page)).toBeVisible();
    await expect(c.getByRole("textbox", { name: /^Subtitle/ })).toHaveValue("A subtitle");
    await expect(c.getByRole("textbox", { name: "Blurb" })).toContainText("A rich blurb.");
    await expect(c.getByRole("textbox", { name: "Price" })).toHaveValue("12.5");
    await expect(c.getByRole("button", { name: "Release" })).toContainText("10:30");
    await expect(c.getByRole("switch", { name: "In print" })).toBeChecked();
    await expect(c.getByRole("combobox", { name: "Genre" })).toContainText("essay");
    await expect(c.getByRole("button", { name: "Related" })).toContainText(post.title);
    await expect(field(c, "fields.cover")).toContainText(fileName);
    await expect(field(c, "fields.chapters[0].title").getByRole("textbox")).toHaveValue("Opening");
    await expect(field(c, "fields.chapters[0].notes[0].note").getByRole("textbox")).toHaveValue(
      "A note",
    );
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.purge(post);
    await api.purgeMedia(media.id);
    await api.dispose();
  }
});

test("repeater_reorder_by_keyboard", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const type = await bookType(api);
  const item = await api.create(type.name, `Book ${uid()}`, {
    fields: {
      subtitle: "S",
      chapters: [
        { title: "Alpha", notes: [] },
        { title: "Beta", notes: [] },
        { title: "Gamma", notes: [] },
      ],
    },
  });
  const page = await as("admin");
  try {
    await openItem(page, type.name, item.id);
    const list = card(page).getByRole("list", { name: "Rows of Chapters" });
    await list.getByRole("button", { name: "Reorder Chapters, row 1: Alpha" }).focus();
    await page.keyboard.press("Space");
    await expect(page.locator("body")).toContainText("Picked up Chapters, row 1: Alpha");
    // The sensor measures the rows once the drag has started (a frame or two).
    await expect(list.locator("[data-dragging]")).toHaveCount(1);
    await page.evaluate(
      () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))),
    );
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("body")).toContainText("is over position 2 of 3");
    await page.keyboard.press("Space");
    await expect(page.locator("body")).toContainText("dropped at position 2 of 3");
    await expect(field(card(page), "fields.chapters[0].title").getByRole("textbox")).toHaveValue(
      "Beta",
    );
    await save(page);
    const stored = (await api.get(type.name, item.id)) as Stored;
    expect((stored.fields.chapters as { title: string }[]).map((r) => r.title)).toEqual([
      "Beta",
      "Alpha",
      "Gamma",
    ]);
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.dispose();
  }
});

test("relation_picks_a_post", async ({ as, site, backend, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const type = await bookType(api);
  const post = await api.createIn("post", `Linked ${uid()}`, "published");
  const item = await api.create(type.name, `Book ${uid()}`, { fields: { subtitle: "S" } });
  const page = await as("admin");
  try {
    await openItem(page, type.name, item.id);
    const trigger = card(page).getByRole("button", { name: "Related" });
    await expect(trigger).toContainText("Choose an item");
    await trigger.click();
    await page.getByPlaceholder("Search Posts…").fill(post.title);
    // Results by type, each with its status.
    const option = page.getByRole("option", { name: new RegExp(post.title) });
    await expect(option).toContainText("Published");
    await expectAccessible(page);
    await option.click();
    await expect(trigger).toContainText(post.title);
    await save(page);
    expect(((await api.get(type.name, item.id)) as Stored).fields.related).toBe(post.id);

    // A target that is gone keeps the value and says so.
    await api.purge(post);
    await page.reload();
    await expect(card(page).getByRole("button", { name: "Related" })).toContainText(
      `Unknown item ${post.id.slice(0, 8)}`,
    );
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.dispose();
  }
});

test("seo_fields_reach_the_head", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const item = await api.createIn("post", `Seo ${uid()}`, "published");
  const page = await as("admin");
  try {
    await openItem(page, "post", item.id);
    const seo = panel(page).locator("[data-slot=panel-seo]");
    const seoTitle = `Search title ${uid()}`;
    const description = `A description for search results ${uid()}.`;
    await seo.getByRole("textbox", { name: "SEO title" }).fill(seoTitle);
    await seo.getByRole("textbox", { name: "Meta description" }).fill(description);
    const preview = seo.locator("[data-slot=search-preview]");
    await expect(preview.locator("[data-slot=search-preview-title]")).toHaveText(seoTitle);
    await expect(preview.locator("[data-slot=search-preview-description]")).toHaveText(description);
    await expect(seo.locator("[data-slot=seo-title-count]")).toHaveText(
      `${[...seoTitle].length} / 60`,
    );
    await snap(page, "editor-seo-preview", showSection(page, "panel-seo"));
    await expectAccessible(page);

    await save(page);
    const stored = (await api.get("post", item.id)) as Stored;
    expect(stored.seo).toEqual({ title: seoTitle, description });
    const html = await (await page.request.get(stored.permalink)).text();
    expect(/<title>([^<]*)<\/title>/.exec(html)?.[1]).toBe(seoTitle);
    expect(html).toContain(`<meta name="description" content="${description}"`);

    // Reset to defaults clears both (the page then uses the item's own).
    await seo.getByRole("button", { name: "Reset to defaults" }).click();
    await save(page);
    expect(((await api.get("post", item.id)) as Stored).seo).toEqual({
      title: null,
      description: null,
    });
  } finally {
    await page.context().close();
    await api.purge(item);
    await api.dispose();
  }
});

test("field_conflict_shows_in_412_dialog", async ({ as, site, backend, guard }) => {
  // The refused save is a 412 by design: the browser logs that failed load.
  guard.allowFailedResource(/\/api\/v1\/content\/[a-z0-9_-]+\/[0-9a-f-]+$/);
  const api = await ContentApi.as(backend, site.url, "admin");
  const type = await bookType(api);
  const item = await api.create(type.name, `Book ${uid()}`, {
    fields: { subtitle: "Shared", price: 1 },
  });
  const a = await as("editor");
  const b = await as("admin");
  try {
    await openItem(a, type.name, item.id);
    await openItem(b, type.name, item.id);
    await card(a).getByRole("textbox", { name: "Price" }).fill("2");
    await save(a);
    await card(b).getByRole("textbox", { name: "Price" }).fill("3");
    await b.keyboard.press(`${await mod(b)}+s`);
    await expect(conflict(b)).toBeVisible();
    const fields = conflict(b).locator("[data-field=fields]");
    await expect(fields).toBeVisible();
    await expect(fields.locator("[data-side=theirs]")).toContainText("price");
    await expect(fields.locator("[data-side=theirs]")).toContainText("2");
    await expect(fields.locator("[data-side=mine]")).toContainText("3");
    await conflict(b).getByRole("button", { name: "Keep mine" }).click();
    await expect(conflict(b)).toHaveCount(0);
    await expect(indicator(b)).toHaveAttribute("data-state", "saved");
    expect(((await api.get(type.name, item.id)) as Stored).fields.price).toBe(3);
  } finally {
    await a.context().close();
    await b.context().close();
    await api.removeType(type.name);
    await api.dispose();
  }
});

test("screens", async ({ as, site, backend, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  const api = await ContentApi.as(backend, site.url, "admin");
  const type = await bookType(api);
  const post = await api.create("post", `Screens related ${uid()}`);
  const item = await api.create(type.name, `Book ${uid()}`, {
    fields: {
      subtitle: "A subtitle that sets the scene",
      blurb: {
        type: "doc",
        content: [{ type: "paragraph", content: [{ type: "text", text: "A short blurb." }] }],
      },
      price: 24.9,
      release: "2026-11-03T08:30:00Z",
      in_print: true,
      genre: "fiction",
      related: post.id,
      chapters: [
        { title: "The beginning", notes: [{ note: "Draft" }] },
        { title: "The middle", notes: [] },
      ],
    },
  });
  const page = await as("admin");
  try {
    await openItem(page, type.name, item.id);
    await expect(card(page)).toBeVisible();
    await expect(card(page).getByRole("button", { name: "Related" })).toContainText(post.title);
    await snap(page, "editor-fields-card", showSection(page, "fields-card", true));
    await expectAccessible(page);

    // Required and invalid: the save is refused here and the fields say why, also from
    // focus mode (which hides the card): it ends and the first field gets the cursor.
    const slot = (method: string) => (r: Response) =>
      r.request().method() === method && r.url().endsWith(`/${item.id}/autosave`);
    const autosaved = page.waitForResponse(slot("PUT"), { timeout: 60_000 });
    await card(page)
      .getByRole("textbox", { name: /^Subtitle/ })
      .fill("");
    await card(page).getByRole("textbox", { name: "Price" }).fill("cheap");
    await title(page).click();
    await page.keyboard.press(`${await mod(page)}+Shift+f`);
    await expect(page.locator("[data-slot=editor-screen]")).toHaveAttribute(
      "data-focus-mode",
      "true",
    );
    await expect(card(page)).toHaveCount(0);
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(page.locator("[data-slot=editor-screen]")).not.toHaveAttribute("data-focus-mode");
    await expect(card(page).getByRole("textbox", { name: /^Subtitle/ })).toBeFocused();
    await expect(
      field(card(page), "fields.subtitle").locator("[data-slot=field-error]"),
    ).toHaveText("Required");
    await expect(field(card(page), "fields.price").locator("[data-slot=field-error]")).toHaveText(
      "Must be a number",
    );
    await snap(page, "editor-fields-invalid", showSection(page, "fields-card", true));
    await expectAccessible(page);
    // The slot has taken the invalid draft (every 10 s, as on a slow CI runner)...
    await autosaved;
    // ...and typed back as stored, it is deleted: nothing unsaved before the reload
    // below, and no restored draft after it (CI run 40404431115).
    const cleared = page.waitForResponse(slot("DELETE"));
    await card(page)
      .getByRole("textbox", { name: /^Subtitle/ })
      .fill("A subtitle that sets the scene");
    await card(page).getByRole("textbox", { name: "Price" }).fill("24.9");
    expect((await cleared).status()).toBe(204);
    await expect(indicator(page)).toHaveAttribute("data-state", "saved");

    // A field removed from the type: listed, and dropped by the next save.
    // A sub-field removed from a repeater, too.
    await api.patchType(type.name, {
      fields: SCHEMA.filter((f) => f.name !== "genre").map((f) =>
        f.name === "chapters"
          ? { ...f, fields: (f.fields ?? []).filter((sub) => sub.name !== "notes") }
          : f,
      ),
    });
    await page.reload();
    const stale = page.locator("[data-slot=stale-keys]");
    await expect(stale).toContainText("Not in the type any more: genre, removed when you save");
    await expect(stale).toContainText(
      "Not in the type any more: chapters[].notes, removed when you save",
    );
    await snap(page, "editor-fields-stale", showSection(page, "fields-card", true));
    await expectAccessible(page);
    await title(page).fill(`${item.title} (renamed)`);
    await save(page);
    await expect(stale).toHaveCount(0);
    const after = ((await api.get(type.name, item.id)) as Stored).fields;
    expect(after).not.toHaveProperty("genre");
    expect(after.chapters).toEqual([{ title: "The beginning" }, { title: "The middle" }]);
  } finally {
    await page.context().close();
    await api.removeType(type.name);
    await api.purge(post);
    await api.dispose();
  }
});
