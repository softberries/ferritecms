// Media in the editor under the real server (M2b-2 Task 8; 07 §3 Editor): the featured
// image from the media picker, an inline image from the picker that the published page
// shows from `/_media/` with its alt text, image files dropped on the body uploaded and
// inserted where they were dropped, and the alt text edited on the image. Every item and
// file is created by the test itself (E2E data hygiene) and removed at the end.

import type { JSHandle, Page } from "@playwright/test";
import { showSection } from "./support/editor-snap";
import { ContentApi, type Item, type Media } from "./support/content-api";
import { png, type FixtureFile } from "./support/files";
import { mod } from "./support/keys";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

type Stored = Item & { fields: Record<string, unknown>; permalink: string; body: unknown };

const panel = (page: Page) => page.locator("[data-slot=editor-panel]");
const body = (page: Page) => page.getByRole("textbox", { name: "Body" });
const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });
const picker = (page: Page) => page.locator("[data-slot=media-picker]");
const indicator = (page: Page) => page.locator("[data-slot=editor-bar] [data-slot=save-state]");

/** A word the media search finds on its own. */
const word = () =>
  `w${uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-10)}`;

async function openItem(page: Page, type: string, id: string): Promise<void> {
  await page.goto(`/admin/content/${type}/${id}`);
  await expect(title(page)).toBeVisible();
  await expect(panel(page)).toBeVisible();
}

/** Picks the media item titled `name` in the open picker (search, select, Select). */
async function pickInPicker(page: Page, name: string): Promise<void> {
  await expect(picker(page)).toBeVisible();
  await picker(page).getByRole("searchbox").fill(name);
  const tile = picker(page).locator("[data-tile]").filter({ hasText: name });
  await expect(tile).toHaveCount(1);
  await tile.click();
  await expect(tile).toHaveAttribute("aria-selected", "true");
  await picker(page).getByRole("button", { name: "Select", exact: true }).click();
  await expect(picker(page)).toHaveCount(0);
}

const paragraph = (text: string) => ({
  type: "doc",
  content: [{ type: "paragraph", content: [{ type: "text", text }] }],
});

/** The image nodes of a stored body. */
function images(doc: unknown): { src: string; alt: string }[] {
  const out: { src: string; alt: string }[] = [];
  const walk = (n: unknown) => {
    if (typeof n !== "object" || n === null) return;
    const node = n as {
      type?: string;
      attrs?: { src?: string; alt?: string };
      content?: unknown[];
    };
    if (node.type === "image") out.push({ src: node.attrs?.src ?? "", alt: node.attrs?.alt ?? "" });
    for (const c of node.content ?? []) walk(c);
  };
  walk(doc);
  return out;
}

test("set_featured_image_from_picker", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const name = `Cover ${word()}`;
  const media = await api.upload(png(`${name}.png`, 640, 400), { title: name, alt: "A cover" });
  const item = await api.create("post", `Featured ${uid()}`);
  const page = await as("admin");
  try {
    await openItem(page, "post", item.id);
    const featured = panel(page).locator("[data-slot=panel-featured]");
    await featured.getByRole("button", { name: "Set featured image" }).click();
    await expect(picker(page)).toBeVisible();
    await expect(picker(page).getByRole("heading", { name: "Set featured image" })).toBeVisible();
    // Images only, and a single-select listbox.
    await expect(picker(page).getByRole("listbox")).toHaveAttribute(
      "aria-multiselectable",
      "false",
    );
    await picker(page).getByRole("searchbox").fill(name);
    const tile = picker(page).locator("[data-tile]").filter({ hasText: name });
    await tile.click();
    await snap(page, "editor-media-picker", { fullPage: false });
    await expectAccessible(page);
    // Enter on the tile chooses it.
    await tile.focus();
    await page.keyboard.press("Enter");
    await expect(picker(page)).toHaveCount(0);

    await expect(featured.locator("[data-slot=media-choice]")).toHaveAttribute("data-state", "set");
    await expect(featured.getByRole("img", { name: "A cover" })).toBeVisible();
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(indicator(page)).toHaveAttribute("data-state", "saved");
    const stored = (await api.get("post", item.id)) as Stored;
    expect(stored.fields.featured_image).toBe(media.id);
    await snap(page, "editor-featured-image", showSection(page, "panel-featured"));
    await expectAccessible(page);

    // Remove: `null` (kept as a value).
    await featured.getByRole("button", { name: "Remove" }).click();
    await expect(featured.getByRole("button", { name: "Set featured image" })).toBeVisible();
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(indicator(page)).toHaveAttribute("data-state", "saved");
    expect(((await api.get("post", item.id)) as Stored).fields.featured_image).toBeNull();
  } finally {
    await page.context().close();
    await api.purge(item);
    await api.purgeMedia(media.id);
    await api.dispose();
  }
});

test("insert_inline_image_and_render", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const name = `Inline ${word()}`;
  const alt = `A gradient ${uid()}`;
  const media = await api.upload(png(`${name}.png`, 800, 500), { title: name, alt });
  const item = await api.create("post", `Inline ${uid()}`, { body: paragraph("Before.") });
  const page = await as("admin");
  try {
    await openItem(page, "post", item.id);
    await body(page).getByText("Before.").click();
    await page.keyboard.press("End");
    await page.keyboard.press("Enter");
    // The slash menu's "Image" opens the picker.
    await page.keyboard.type("/image");
    await expect(page.getByRole("listbox", { name: "Insert a block" })).toBeVisible();
    await page.keyboard.press("Enter");
    await pickInPicker(page, name);
    const figure = body(page).locator("[data-slot=editor-image]");
    await expect(figure).toHaveAttribute("data-state", "ready");
    await expect(figure.getByRole("img", { name: alt })).toBeVisible();

    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(indicator(page)).toHaveAttribute("data-state", "saved");
    const saved = (await api.get("post", item.id)) as Stored;
    expect(images(saved.body)).toEqual([{ src: `ferrite:media/${media.id}`, alt }]);

    const published = (await api.transition(saved, "publish")) as Stored;
    const html = await (await page.request.get(published.permalink)).text();
    const tag = /<img\b[^>]*>/.exec(html)?.[0] ?? "";
    expect(tag).toMatch(/src="\/_media\/[^"]+"/);
    expect(tag).toContain(`alt="${alt}"`);
  } finally {
    await page.context().close();
    await api.purge(item);
    await api.purgeMedia(media.id);
    await api.dispose();
  }
});

/** Drops `files` on the body at the given text through a real `DataTransfer`. */
async function dropOnBody(page: Page, at: string, files: FixtureFile[]): Promise<void> {
  const data: JSHandle<DataTransfer> = await page.evaluateHandle(
    (list) => {
      const dt = new DataTransfer();
      for (const f of list) {
        const bytes = Uint8Array.from(atob(f.b64), (c) => c.charCodeAt(0));
        dt.items.add(new File([bytes], f.name, { type: f.mimeType }));
      }
      return dt;
    },
    files.map((f) => ({ name: f.name, mimeType: f.mimeType, b64: f.buffer.toString("base64") })),
  );
  const target = body(page).getByText(at);
  const box = await target.boundingBox();
  if (!box) throw new Error("drop target not visible");
  const point = { clientX: box.x + 4, clientY: box.y + box.height / 2 };
  await body(page).dispatchEvent("dragover", { dataTransfer: data, ...point });
  await body(page).dispatchEvent("drop", { dataTransfer: data, ...point });
}

test("drop_image_into_editor_uploads_and_inserts", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const name = `dropped-${word()}`;
  const item = await api.create("post", `Drop ${uid()}`, {
    body: {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "First paragraph." }] },
        { type: "paragraph", content: [{ type: "text", text: "Second paragraph." }] },
      ],
    },
  });
  const page = await as("admin");
  let mediaId: string | null = null;
  try {
    await openItem(page, "post", item.id);
    await dropOnBody(page, "Second paragraph.", [png(`${name}.png`, 320, 200)]);
    // The upload lands as an image where it was dropped: before the second paragraph.
    const figure = body(page).locator("[data-slot=editor-image]");
    await expect(figure).toHaveAttribute("data-state", "ready");
    await expect(body(page).locator("[data-slot=editor-upload-placeholder]")).toHaveCount(0);
    const order = await body(page)
      .locator(":scope > *")
      .evaluateAll((els) =>
        els.map((e) =>
          e.matches("[data-slot=editor-image]") || e.querySelector("[data-slot=editor-image]")
            ? "image"
            : e.textContent,
        ),
      );
    expect(order).toEqual(["First paragraph.", "image", "Second paragraph."]);

    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(indicator(page)).toHaveAttribute("data-state", "saved");
    const listed = await api.listMedia({ q: name });
    expect(listed).toHaveLength(1);
    mediaId = (listed[0] as Media).id;
    const saved = (await api.get("post", item.id)) as Stored;
    expect(images(saved.body).map((i) => i.src)).toEqual([`ferrite:media/${mediaId}`]);
  } finally {
    await page.context().close();
    await api.purge(item);
    if (mediaId) await api.purgeMedia(mediaId);
    await api.dispose();
  }
});

test("edit_image_alt", async ({ as, site, backend, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const name = `Alt ${word()}`;
  const media = await api.upload(png(`${name}.png`, 400, 300), { title: name });
  const item = await api.create("post", `Alt ${uid()}`, {
    body: {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "Text." }] },
        { type: "image", attrs: { src: `ferrite:media/${media.id}`, alt: "", title: null } },
      ],
    },
  });
  const page = await as("admin");
  try {
    await openItem(page, "post", item.id);
    const figure = body(page).locator("[data-slot=editor-image]");
    await expect(figure).toHaveAttribute("data-state", "ready");
    // Without alt text the image is decorative, and the button says so.
    const button = figure.getByRole("button", { name: /Edit the alt text/ });
    await expect(button).toHaveText("Decorative");
    await button.click();
    const field = page.getByRole("textbox", { name: "Alternative text" });
    await expect(field).toBeFocused();
    const alt = `A described image ${uid()}`;
    await field.fill(alt);
    await expectAccessible(page);
    await page.getByRole("button", { name: "Done" }).click();
    await expect(button).toHaveText("Alt text");
    await expect(figure.getByRole("img", { name: alt })).toBeVisible();

    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(indicator(page)).toHaveAttribute("data-state", "saved");
    const saved = (await api.get("post", item.id)) as Stored;
    expect(images(saved.body)).toEqual([{ src: `ferrite:media/${media.id}`, alt }]);

    // Esc leaves the text as it was.
    await button.click();
    await page.getByRole("textbox", { name: "Alternative text" }).fill("Not this");
    await page.keyboard.press("Escape");
    await expect(figure.getByRole("img", { name: alt })).toBeVisible();
  } finally {
    await page.context().close();
    await api.purge(item);
    await api.purgeMedia(media.id);
    await api.dispose();
  }
});

/** Holds every `POST /media` until the test ends (the upload stays running). */
async function holdUploads(page: Page): Promise<void> {
  await page.route("**/api/v1/media", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    await new Promise((r) => setTimeout(r, 60_000));
    await route.abort().catch(() => {});
  });
}

test("closing_the_picker_during_an_upload_asks", async ({ as, site, backend, guard }) => {
  guard.allowFailedResource(/\/api\/v1\/media$/);
  const api = await ContentApi.as(backend, site.url, "admin");
  const item = await api.create("post", `Picker upload ${uid()}`);
  const page = await as("admin");
  try {
    await openItem(page, "post", item.id);
    await holdUploads(page);
    await panel(page).getByRole("button", { name: "Set featured image" }).click();
    await picker(page).getByRole("tab", { name: "Upload" }).click();
    const name = `${uid("held")}.png`;
    await picker(page)
      .locator("input[type=file]")
      .setInputFiles({ name, mimeType: "image/png", buffer: png(name, 200, 120).buffer });
    await expect(
      picker(page).locator("[data-slot=upload-row]").filter({ hasText: name }),
    ).toHaveAttribute("data-state", "uploading");

    // Esc asks first; Keep uploading keeps the picker and the upload.
    const ask = page.locator("[data-slot=picker-uploads-dialog]");
    await page.keyboard.press("Escape");
    await expect(ask).toBeVisible();
    await expect(ask).toContainText("Uploads are still running");
    await ask.getByRole("button", { name: "Keep uploading" }).click();
    await expect(ask).toHaveCount(0);
    await expect(picker(page)).toBeVisible();
    await expect(
      picker(page).locator("[data-slot=upload-row]").filter({ hasText: name }),
    ).toHaveAttribute("data-state", "uploading");

    // Cancel asks too; Cancel uploads closes it.
    await picker(page).getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(ask).toBeVisible();
    await ask.getByRole("button", { name: "Cancel uploads" }).click();
    await expect(picker(page)).toHaveCount(0);
  } finally {
    await page.context().close();
    await api.purge(item);
    await api.dispose();
  }
});

test("leaving_during_a_drop_upload_asks", async ({ as, site, backend, guard }) => {
  guard.allowFailedResource(/\/api\/v1\/media$/);
  const api = await ContentApi.as(backend, site.url, "admin");
  const item = await api.create("post", `Drop leave ${uid()}`, { body: paragraph("Here.") });
  const page = await as("admin");
  try {
    await openItem(page, "post", item.id);
    await holdUploads(page);
    await dropOnBody(page, "Here.", [png(`${uid("held")}.png`, 200, 120)]);
    await expect(body(page).locator("[data-slot=editor-upload-placeholder]")).toHaveCount(1);

    const dialog = page.locator("[data-slot=leave-dialog]");
    await page.getByRole("link", { name: "Pages" }).first().click();
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("Images are still uploading");
    await dialog.getByRole("button", { name: "Stay" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(body(page).locator("[data-slot=editor-upload-placeholder]")).toHaveCount(1);

    await page.getByRole("link", { name: "Pages" }).first().click();
    await dialog.getByRole("button", { name: "Leave and cancel uploads" }).click();
    await expect(page).toHaveURL(/\/admin\/content\/page$/);
  } finally {
    await page.context().close();
    await api.purge(item);
    await api.dispose();
  }
});

/** Holds every `POST /media` until `release()` is called, then lets them through. */
async function holdUploadsUntilReleased(page: Page): Promise<{ release: () => void }> {
  let release = () => {};
  const released = new Promise<void>((r) => {
    release = r;
  });
  await page.route("**/api/v1/media", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    await released;
    await route.continue();
  });
  return { release: () => release() };
}

test("first_save_while_a_drop_uploads_keeps_the_image", async ({ as, site, backend }) => {
  // A running drop upload survives the hand-over from `/new` to `/$id` (final review A1),
  // and its image lands without taking the focus from the title (A2).
  const api = await ContentApi.as(backend, site.url, "admin");
  const name = `handover-${word()}`;
  const page = await as("admin");
  let id: string | null = null;
  let mediaId: string | null = null;
  try {
    await page.goto("/admin/content/post/new");
    await title(page).fill(`Hand-over ${uid()}`);
    await body(page).click();
    await page.keyboard.type("Before the image.");
    const held = await holdUploadsUntilReleased(page);
    await dropOnBody(page, "Before the image.", [png(`${name}.png`, 240, 160)]);
    const placeholders = body(page).locator("[data-slot=editor-upload-placeholder]");
    await expect(placeholders).toHaveCount(1);

    // The first save moves the item to `/$id`; the upload still runs there.
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(page).toHaveURL(/\/admin\/content\/post\/[0-9a-f-]{36}$/);
    id = /\/post\/([0-9a-f-]{36})$/.exec(page.url())?.[1] ?? null;
    await expect(indicator(page)).toHaveAttribute("data-state", "saved");
    await expect(placeholders).toHaveCount(1);

    // Typing in the title while the upload finishes: the focus stays there.
    await title(page).click();
    await page.keyboard.press("End");
    held.release();
    const figure = body(page).locator("[data-slot=editor-image]");
    await expect(figure).toHaveAttribute("data-state", "ready");
    await expect(placeholders).toHaveCount(0);
    await expect(title(page)).toBeFocused();
    await page.keyboard.type(" typed on");
    await expect(title(page)).toHaveValue(/ typed on$/);
    // Where it was dropped: before the paragraph.
    const order = await body(page)
      .locator(":scope > *")
      .evaluateAll((els) =>
        els.map((e) =>
          e.matches("[data-slot=editor-image]") || e.querySelector("[data-slot=editor-image]")
            ? "image"
            : e.textContent,
        ),
      );
    expect(order).toEqual(["image", "Before the image."]);

    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(indicator(page)).toHaveAttribute("data-state", "saved");
    const listed = await api.listMedia({ q: name });
    expect(listed).toHaveLength(1);
    mediaId = (listed[0] as Media).id;
    if (!id) throw new Error("no item id in the address");
    const saved = (await api.get("post", id)) as Stored;
    expect(images(saved.body).map((i) => i.src)).toEqual([`ferrite:media/${mediaId}`]);
  } finally {
    await page.context().close();
    if (id) await api.purge(await api.get("post", id));
    if (mediaId) await api.purgeMedia(mediaId);
    await api.dispose();
  }
});

test("take_theirs_while_a_drop_uploads_keeps_the_image", async ({ as, site, backend, guard }) => {
  // A reopen of the screen (Take theirs) while a drop upload runs: the upload goes on and
  // its image lands at the end of the reopened body (final review A1).
  guard.allowFailedResource(/\/api\/v1\/content\/post\/[0-9a-f-]+$/);
  const api = await ContentApi.as(backend, site.url, "admin");
  const name = `reopen-${word()}`;
  const item = await api.create("post", `Reopen ${uid()}`, { body: paragraph("Here.") });
  const page = await as("admin");
  let mediaId: string | null = null;
  try {
    await openItem(page, "post", item.id);
    // Someone else saves meanwhile.
    const theirs = `Their title ${uid()}`;
    await api.patch(item, { title: theirs });
    const held = await holdUploadsUntilReleased(page);
    await dropOnBody(page, "Here.", [png(`${name}.png`, 240, 160)]);
    const placeholders = body(page).locator("[data-slot=editor-upload-placeholder]");
    await expect(placeholders).toHaveCount(1);
    await body(page).getByText("Here.").click();
    await page.keyboard.press("End");
    await page.keyboard.type(" Mine.");
    await page.keyboard.press(`${await mod(page)}+s`);
    const dialog = page.locator("[data-slot=conflict-dialog]");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Take theirs" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(title(page)).toHaveValue(theirs);
    // The reopened screen shows the running upload again.
    await expect(placeholders).toHaveCount(1);

    held.release();
    const figure = body(page).locator("[data-slot=editor-image]");
    await expect(figure).toHaveAttribute("data-state", "ready");
    await expect(placeholders).toHaveCount(0);
    const order = await body(page)
      .locator(":scope > *")
      .evaluateAll((els) =>
        els.map((e) =>
          e.matches("[data-slot=editor-image]") || e.querySelector("[data-slot=editor-image]")
            ? "image"
            : e.textContent,
        ),
      );
    expect(order).toEqual(["Here.", "image"]);
    const listed = await api.listMedia({ q: name });
    expect(listed).toHaveLength(1);
    mediaId = (listed[0] as Media).id;
  } finally {
    await page.context().close();
    await api.purge(item);
    if (mediaId) await api.purgeMedia(mediaId);
    await api.dispose();
  }
});
