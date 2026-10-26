// The media library under the real server (M2b-2 Task 7; 07 §3, 11 §6–§8): uploads by
// drop and by the file chooser with progress, variants after the job, an upload whose
// response is lost retried with the same `Idempotency-Key` (Review Focus 1), a refused
// type on its own row, leaving during an upload, alt text, search and kind filters,
// trash with Undo, the Trash view, hostile names as text (Review Focus 3), what a
// contributor sees, and the review screens. Every item is created by the test itself
// with a unique name (E2E data hygiene) and deleted at the end.

import AxeBuilder from "@axe-core/playwright";
import type { JSHandle, Locator, Page } from "@playwright/test";
import { ContentApi, type Media } from "./support/content-api";
import { exe, pdf, png, svg, type FixtureFile } from "./support/files";
import { mod } from "./support/keys";
import { AXE_TAGS, expect, test } from "./support/fixtures";
import { SCHEMES, VIEWPORTS, screenFile } from "./support/snap";
import { uid } from "./support/uid";

/** A word the title search can find on its own. */
const word = () =>
  `w${uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-10)}`;

// Not by role: the open detail sheet hides the page from the accessibility tree.
const heading = (page: Page) => page.locator("#main h1");
const grid = (page: Page) => page.getByRole("listbox", { name: "Media files" });
const tile = (page: Page, text: string): Locator =>
  grid(page).locator("[data-tile]").filter({ hasText: text });
const tray = (page: Page) => page.locator("[data-slot=upload-tray]");
const trayRow = (page: Page, name: string) =>
  tray(page).locator("[data-slot=upload-row]").filter({ hasText: name });
const detail = (page: Page) => page.locator("[data-slot=media-detail]");
const bulkBar = (page: Page) => page.getByRole("region", { name: "Selection" });
const fileInput = (page: Page) => page.locator("[data-slot=media-file-input]");
/** Selects a tile with its (pointer-only) check box. */
async function pick(page: Page, text: string): Promise<void> {
  await tile(page, text).hover();
  await tile(page, text).locator("[data-slot=tile-check]").click();
  await expect(tile(page, text)).toHaveAttribute("aria-selected", "true");
}
/** Scrolls the open detail sheet to its end (variants and actions). */
async function sheetToEnd(page: Page): Promise<void> {
  await detail(page).evaluate((el) => el.scrollTo({ top: el.scrollHeight }));
}
const searchBox = (page: Page) => page.getByRole("searchbox", { name: "Search media" });

/** `POST /api/v1/media` requests only. */
const isUpload = (url: string, method: string) =>
  method === "POST" && new URL(url).pathname === "/api/v1/media";

async function openLibrary(page: Page, query = ""): Promise<void> {
  await page.goto(`/admin/media${query}`);
  await expect(heading(page)).toContainText(/Media|Trash/);
  await loaded(page);
}

/** The list has left its loading and placeholder states. */
async function loaded(page: Page): Promise<void> {
  await expect(page.locator("[aria-busy=true]")).toHaveCount(0);
  await expect(page.locator(".opacity-60[data-slot=media-grid]")).toHaveCount(0);
}

/** Drops `files` on the page's drop zone through a real `DataTransfer`. */
async function dropFiles(page: Page, files: FixtureFile[]): Promise<void> {
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
  const zone = page.locator("[data-slot=drop-zone]");
  await zone.dispatchEvent("dragenter", { dataTransfer: data });
  await zone.dispatchEvent("dragover", { dataTransfer: data });
  await expect(zone).toHaveAttribute("data-over", "true");
  await zone.dispatchEvent("drop", { dataTransfer: data });
}

/**
 * `snap` and `expectAccessible` wait for the network to go idle, which a held upload
 * never does: these take the screenshots (SQLite project only, as `snap`) and run axe in
 * both schemes without that wait.
 */
async function snapHeld(page: Page, name: string, backend: string): Promise<void> {
  if (backend !== "sqlite") return;
  for (const scheme of SCHEMES) {
    await page.emulateMedia({ colorScheme: scheme });
    for (const v of ["desktop", "mobile"] as const) {
      await page.setViewportSize(VIEWPORTS[v]);
      await page.waitForTimeout(300);
      await page.screenshot({
        path: screenFile(name, scheme, v),
        fullPage: true,
        animations: "disabled",
      });
    }
  }
  await page.emulateMedia({ colorScheme: "light" });
  await page.setViewportSize(VIEWPORTS.desktop);
}

async function axeHeld(page: Page): Promise<void> {
  for (const scheme of SCHEMES) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.waitForTimeout(300);
    const result = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();
    const violations = result.violations.map((v) => ({
      id: v.id,
      targets: v.nodes.map((n) => n.target.join(" ")),
    }));
    expect(violations, `axe violations in ${scheme} mode`).toEqual([]);
  }
  await page.emulateMedia({ colorScheme: "light" });
}

/** Media items of the user named `name` (the newest 200). */
async function byName(api: ContentApi, name: string): Promise<Media[]> {
  const items = await api.listMedia({ author: api.me.id, limit: "200" });
  return items.filter((m) => m.original_name === name);
}

test("upload_by_drop_shows_progress_and_variants", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const name = `${uid("drop")}.png`;
  try {
    const page = await as("author");
    await openLibrary(page);
    // Hold the upload a moment so its running state can be seen.
    let release: () => void = () => {};
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/api/v1/media", async (route) => {
      if (route.request().method() !== "POST") return route.fallback();
      await held;
      await route.continue();
    });
    await dropFiles(page, [png(name, 1200, 800)]);
    const row = trayRow(page, name);
    await expect(row).toBeVisible();
    await expect(row).toHaveAttribute("data-state", "uploading");
    await expect(row.getByRole("progressbar", { name: `Upload of ${name}` })).toBeVisible();
    await expect(tray(page).getByRole("heading")).toHaveText("Uploading: 0 of 1 done");
    release();
    await expect(row).toHaveAttribute("data-state", "done");
    await expect(tray(page).getByRole("heading")).toHaveText("1 file uploaded");
    // The new tile is first in the grid (titled after the file name, 07 §1).
    const stem = name.replace(/\.png$/, "");
    await expect(grid(page).locator("[data-tile]").first()).toContainText(stem);
    // The detail lists the variants once the job has made them (polled every 3 s).
    await tile(page, stem).click();
    await expect(page).toHaveURL(/[?&]id=/);
    await expect(detail(page)).toBeVisible();
    const variants = detail(page).getByRole("list", { name: "Variants" });
    await expect(variants).toBeVisible({ timeout: 45_000 });
    await expect(variants.getByRole("listitem")).not.toHaveCount(0);
    await expect(variants).toContainText("thumbnail");
    await expect(detail(page)).toContainText("1200 × 800");
  } finally {
    for (const m of await byName(api, name)) await api.purgeMedia(m.id);
    await api.dispose();
  }
});

test("upload_retry_after_network_drop_creates_one_item", async ({ as, site, backend, guard }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const name = `${uid("retry")}.png`;
  // The dropped response is a failed request in the browser's log.
  guard.allowFailedResource(/\/api\/v1\/media$/);
  guard.allow(/net::ERR_CONNECTION_RESET/);
  try {
    const page = await as("author");
    await openLibrary(page);
    const keys: string[] = [];
    let first = true;
    await page.route("**/api/v1/media", async (route) => {
      const req = route.request();
      if (req.method() !== "POST") return route.fallback();
      keys.push((await req.headerValue("Idempotency-Key")) ?? "");
      if (first) {
        // The server stores the upload, but its answer never reaches the page.
        first = false;
        const res = await route.fetch();
        expect(res.status()).toBe(201);
        await route.abort("connectionreset");
        return;
      }
      await route.continue();
    });
    await fileInput(page).setInputFiles({
      name,
      mimeType: "image/png",
      buffer: png(name, 64, 48).buffer,
    });
    const row = trayRow(page, name);
    // It retries by itself after 2 s with the same key, and the server answers with the
    // stored upload.
    await expect(row).toHaveAttribute("data-state", "done", { timeout: 15_000 });
    expect(keys).toHaveLength(2);
    expect(keys[0]).toMatch(/^[0-9a-f-]{36}$/);
    expect(keys[1]).toBe(keys[0]);
    expect(await byName(api, name)).toHaveLength(1);
  } finally {
    for (const m of await byName(api, name)) await api.purgeMedia(m.id);
    await api.dispose();
  }
});

test("rejected_type_shows_on_its_row", async ({ as, site, backend, guard, snap }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const bad = `${uid("bad")}.exe`;
  const good = `${uid("good")}.png`;
  guard.allowFailedResource(/\/api\/v1\/media$/);
  try {
    const page = await as("author");
    await openLibrary(page);
    const refused = page.waitForResponse(
      (r) => isUpload(r.url(), r.request().method()) && r.status() === 415,
    );
    await fileInput(page).setInputFiles([
      { name: bad, mimeType: "application/octet-stream", buffer: exe(bad).buffer },
      { name: good, mimeType: "image/png", buffer: png(good, 80, 60).buffer },
    ]);
    const problem = (await (await refused).json()) as { detail: string };
    const row = trayRow(page, bad);
    await expect(row).toHaveAttribute("data-state", "failed");
    // A plain sentence, the server's detail under it, and no Retry: it would fail again.
    await expect(row.locator("[data-slot=upload-error]")).toHaveText(
      "This file type isn't supported.",
    );
    await expect(row.locator("[data-slot=upload-error-detail]")).toHaveText(problem.detail);
    // The other file is not held up by it, and a 415 is never retried by itself.
    await expect(trayRow(page, good)).toHaveAttribute("data-state", "done");
    await expect(row.getByRole("button", { name: `Retry the upload of ${bad}` })).toHaveCount(0);
    await expect(row.getByRole("button", { name: `Dismiss ${bad}` })).toBeVisible();
    await expect(tray(page).getByRole("heading")).toHaveText("1 upload failed");
    await snap(page, "media-tray-failed");
    expect(await byName(api, bad)).toHaveLength(0);
    // Closing the tray dismisses both rows.
    await tray(page).getByRole("button", { name: "Close uploads" }).click();
    await expect(tray(page)).toHaveCount(0);
  } finally {
    for (const m of await byName(api, good)) await api.purgeMedia(m.id);
    await api.dispose();
  }
});

test("leaving_during_upload_asks", async ({ as, site, backend, guard }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const name = `${uid("leave")}.png`;
  guard.allowFailedResource(/\/api\/v1\/media$/);
  try {
    const page = await as("author");
    await openLibrary(page);
    // A slowed route: the upload stays running until the test lets it go.
    await page.route("**/api/v1/media", async (route) => {
      if (route.request().method() !== "POST") return route.fallback();
      await new Promise((r) => setTimeout(r, 60_000));
      await route.abort().catch(() => {});
    });
    await fileInput(page).setInputFiles({
      name,
      mimeType: "image/png",
      buffer: png(name, 1200, 800).buffer,
    });
    await expect(trayRow(page, name)).toHaveAttribute("data-state", "uploading");
    await snapHeld(page, "media-tray-uploading", backend);
    await axeHeld(page);

    const dialog = page.locator("[data-slot=upload-leave-dialog]");
    await page.getByRole("link", { name: "Dashboard" }).first().click();
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("Uploads are still running");
    await axeHeld(page);
    await snapHeld(page, "media-leave-dialog", backend);
    await dialog.getByRole("button", { name: "Stay" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(page).toHaveURL(/\/admin\/media$/);
    await expect(trayRow(page, name)).toHaveAttribute("data-state", "uploading");

    // The shortcut asks too; leaving cancels the upload.
    await page.locator("body").press("g");
    await page.locator("body").press("d");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Leave and cancel" }).click();
    await expect(page).toHaveURL(/\/admin\/?$/);
    expect(await byName(api, name)).toHaveLength(0);
  } finally {
    for (const m of await byName(api, name)) await api.purgeMedia(m.id);
    await api.dispose();
  }
});

test("edit_alt_text", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const media = await api.upload(png(`${uid("alt")}.png`, 120, 80), { title: `Alt ${uid()}` });
  try {
    const page = await as("author");
    await openLibrary(page, `?id=${media.id}`);
    await expect(detail(page)).toBeVisible();
    const alt = detail(page).getByRole("textbox", { name: "Alt text" });
    await alt.fill("A gradient from ember to night blue");
    const saved = page.waitForResponse(
      (r) => r.request().method() === "PATCH" && r.url().endsWith(`/api/v1/media/${media.id}`),
    );
    await alt.blur();
    const res = await saved;
    expect(res.status()).toBe(200);
    expect(await res.request().headerValue("If-Match")).toBe(`"${media.version}"`);
    expect((await api.getMedia(media.id)).alt).toBe("A gradient from ember to night blue");

    // `mod+s` saves the title.
    const title = detail(page).getByRole("textbox", { name: "Title", exact: true });
    await title.fill("Renamed by the keyboard");
    const savedTitle = page.waitForResponse(
      (r) => r.request().method() === "PATCH" && r.url().endsWith(`/api/v1/media/${media.id}`),
    );
    await page.keyboard.press(`${await mod(page)}+s`);
    expect((await savedTitle).status()).toBe(200);
    expect((await api.getMedia(media.id)).title).toBe("Renamed by the keyboard");

    // Reloaded, the sheet shows the saved values.
    await page.reload();
    await expect(detail(page).getByRole("textbox", { name: "Alt text" })).toHaveValue(
      "A gradient from ember to night blue",
    );
  } finally {
    await api.purgeMedia(media.id);
    await api.dispose();
  }
});

test("search_and_kind_filters", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const other = await ContentApi.as(backend, site.url, "author2");
  const w = word();
  const image = await api.upload(png(`${uid("k")}.png`, 90, 60), { title: `${w} harbour` });
  const doc = await api.upload(pdf(`${uid("k")}.pdf`), { title: `${w} report` });
  const theirs = await other.upload(png(`${uid("k")}.png`, 90, 60), { title: `${w} theirs` });
  try {
    const page = await as("author");
    await openLibrary(page);
    await searchBox(page).fill(w);
    await expect(page).toHaveURL(new RegExp(`[?&]q=${w}`));
    await loaded(page);
    await expect(grid(page).locator("[data-tile]")).toHaveCount(3);
    // A search covers every uploader: Mine is disabled.
    await expect(page.getByRole("button", { name: "Mine" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    // Kinds combine with the search.
    await page.getByRole("button", { name: "Documents" }).click();
    await expect(page).toHaveURL(/kind=document/);
    await loaded(page);
    await expect(grid(page).locator("[data-tile]")).toHaveCount(1);
    await expect(tile(page, `${w} report`)).toBeVisible();
    await page.getByRole("button", { name: "Images" }).click();
    await loaded(page);
    await expect(grid(page).locator("[data-tile]")).toHaveCount(2);
    await expect(tile(page, `${w} report`)).toHaveCount(0);

    // Without the search, Mine narrows to the author's own files.
    await page.getByRole("button", { name: "Clear search" }).click();
    await expect(page).not.toHaveURL(/q=/);
    await page.getByRole("button", { name: "Mine" }).click();
    await expect(page).toHaveURL(/mine=true/);
    await loaded(page);
    await expect(tile(page, `${w} harbour`)).toBeVisible();
    await expect(tile(page, `${w} theirs`)).toHaveCount(0);

    // The list view keeps the filters and is remembered for the next visit.
    await page.getByRole("radio", { name: "List view" }).click();
    await expect(page).toHaveURL(/view=list/);
    await expect(page.getByRole("table", { name: "Media files" })).toBeVisible();
    await openLibrary(page);
    await expect(page.getByRole("table", { name: "Media files" })).toBeVisible();
    await page.getByRole("radio", { name: "Grid view" }).click();
    await expect(grid(page)).toBeVisible();
  } finally {
    await api.purgeMedia(image.id);
    await api.purgeMedia(doc.id);
    await other.purgeMedia(theirs.id);
    await api.dispose();
    await other.dispose();
  }
});

test("trash_with_undo", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const w = word();
  const a = await api.upload(png(`${uid("t")}.png`, 90, 60), { title: `${w} one` });
  const b = await api.upload(png(`${uid("t")}.png`, 90, 60), { title: `${w} two` });
  try {
    const page = await as("author");
    await openLibrary(page, `?q=${w}`);
    await expect(grid(page).locator("[data-tile]")).toHaveCount(2);
    // By pointer, then by keyboard (Space on the focused option), as assistive tech sees it.
    await pick(page, `${w} one`);
    const two = tile(page, `${w} two`);
    await expect(two).toHaveAttribute("aria-selected", "false");
    await two.focus();
    await page.keyboard.press("Space");
    await expect(two).toHaveAttribute("aria-selected", "true");
    await expect(bulkBar(page)).toContainText("2 selected");
    await bulkBar(page).getByRole("button", { name: "Move to trash" }).click();
    const toast = page
      .locator("[data-sonner-toast]")
      .filter({ hasText: "2 files moved to the trash" });
    await expect(toast).toBeVisible();
    await expect(grid(page)).toHaveCount(0);
    expect((await api.getMedia(a.id)).status).toBe("trash");
    await toast.getByRole("button", { name: "Undo" }).click();
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "2 files restored" }),
    ).toBeVisible();
    await expect(grid(page).locator("[data-tile]")).toHaveCount(2);
    expect((await api.getMedia(a.id)).status).toBe("published");
    expect((await api.getMedia(b.id)).status).toBe("published");
  } finally {
    await api.purgeMedia(a.id);
    await api.purgeMedia(b.id);
    await api.dispose();
  }
});

test("trash_view_restore_and_delete", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const w = word();
  const keep = await api.upload(png(`${uid("tv")}.png`, 90, 60), { title: `${w} keep` });
  const drop = await api.upload(pdf(`${uid("tv")}.pdf`), { title: `${w} drop` });
  await api.trashMedia(keep);
  await api.trashMedia(drop);
  try {
    const page = await as("author");
    await openLibrary(page);
    await page.getByRole("button", { name: "Trash" }).click();
    await expect(page).toHaveURL(/trash=true/);
    await expect(heading(page)).toContainText("Trash");
    // The trash has no search.
    await expect(searchBox(page)).toHaveCount(0);
    await loaded(page);
    await expect(tile(page, `${w} keep`)).toBeVisible();

    await tile(page, `${w} keep`).click();
    await expect(detail(page)).toBeVisible();
    await expectAccessible(page);
    await detail(page).getByRole("button", { name: "Restore" }).click();
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "1 file restored" }),
    ).toBeVisible();
    await expect(tile(page, `${w} keep`)).toHaveCount(0);
    expect((await api.getMedia(keep.id)).status).toBe("published");

    await tile(page, `${w} drop`).click();
    await detail(page).getByRole("button", { name: "Delete permanently" }).click();
    const confirm = page.getByRole("alertdialog");
    await expect(confirm).toContainText("Delete this file permanently?");
    await snap(page, "media-delete-confirm");
    await confirm.getByRole("button", { name: "Delete permanently" }).click();
    await expect(tile(page, `${w} drop`)).toHaveCount(0);
    expect(await api.mediaStatus(drop.id)).toBe(404);
  } finally {
    await api.purgeMedia(keep.id);
    await api.purgeMedia(drop.id);
    await api.dispose();
  }
});

test("hostile_file_name_renders_as_text", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const hostileName = `<img src=x onerror=alert(1)>${uid("h")}.png`;
  const hostileTitle = `<script>alert("t")</script><b>${uid("h")}</b>`;
  const media = await api.upload(png(hostileName, 90, 60), { title: hostileTitle });
  try {
    const page = await as("author");
    page.on("dialog", (d) => {
      throw new Error(`a dialog opened: ${d.message()}`);
    });
    await openLibrary(page, `?view=list&id=${media.id}`);
    await expect(detail(page)).toBeVisible();
    // The title and the original name are text, not markup.
    await expect(detail(page).getByRole("heading", { name: hostileTitle })).toBeVisible();
    await expect(detail(page).getByText(media.original_name, { exact: true })).toBeVisible();
    await expect(page.locator("img[src=x], b:not([class])")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(detail(page)).toHaveCount(0);
    await expect(page.getByRole("table").getByText(hostileTitle, { exact: true })).toBeVisible();
    await expect(page.locator("script:not([src])")).toHaveCount(0);
  } finally {
    await api.purgeMedia(media.id);
    await api.dispose();
  }
});

test("contributor_sees_no_upload", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const w = word();
  const media = await api.upload(png(`${uid("c")}.png`, 90, 60), { title: `${w} public` });
  try {
    const page = await as("contributor");
    await page.goto("/admin/");
    await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
    // The nav shows Media (read-only for contributors).
    await page.getByRole("link", { name: "Media" }).first().click();
    await expect(heading(page)).toContainText("Media");
    await loaded(page);
    await expect(page.getByRole("button", { name: "Upload" })).toHaveCount(0);
    await expect(page.locator("[data-slot=drop-zone]")).toHaveCount(0);
    await expect(fileInput(page)).toHaveCount(0);
    // Their detail is read-only, without actions.
    await searchBox(page).fill(w);
    await loaded(page);
    await tile(page, `${w} public`).click();
    await expect(detail(page).getByRole("textbox", { name: "Alt text" })).toHaveAttribute(
      "readonly",
      "",
    );
    await expect(detail(page).getByRole("button", { name: "Move to trash" })).toHaveCount(0);
    await page.keyboard.press("Escape");
    // The palette offers no upload.
    await page.keyboard.press(`${await mod(page)}+k`);
    const palette = page.getByRole("dialog", { name: "Command palette" });
    await expect(palette).toBeVisible();
    await palette.getByRole("combobox").fill("upload");
    await expect(palette.getByRole("option", { name: "Upload media" })).toHaveCount(0);
  } finally {
    await api.purgeMedia(media.id);
    await api.dispose();
  }
});

test("palette_upload_opens_the_chooser", async ({ as }) => {
  const page = await as("author");
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
  await page.keyboard.press(`${await mod(page)}+k`);
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await palette.getByRole("combobox").fill("upload");
  const chooser = page.waitForEvent("filechooser");
  await palette.getByRole("option", { name: "Upload media" }).click();
  await chooser;
  await expect(page).toHaveURL(/\/admin\/media$/);
});

test("screens", async ({ as, site, backend, guard, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  // The stubbed failures are logged by the browser.
  guard.allowFailedResource(/\/api\/v1\/media/);
  const api = await ContentApi.as(backend, site.url, "author");
  const w = word();
  const created: Media[] = [];
  try {
    const colours: [number, number, number][][] = [
      [
        [232, 93, 45],
        [38, 70, 120],
      ],
      [
        [244, 162, 89],
        [42, 111, 151],
      ],
      [
        [96, 153, 102],
        [32, 48, 64],
      ],
    ];
    for (const [i, [from, to]] of colours.entries()) {
      created.push(
        await api.upload(png(`${uid("s")}.png`, 1200, 800, from, to), {
          title: `${w} landscape ${i + 1}`,
          alt: `Gradient ${i + 1}`,
        }),
      );
    }
    created.push(await api.upload(pdf(`${uid("s")}.pdf`), { title: `${w} annual report` }));
    created.push(await api.upload(svg(`${uid("s")}.svg`), { title: `${w} logo` }));

    const page = await as("author");
    // Wait for the variants so the tiles show their thumbnails.
    for (const m of created.slice(0, 3)) {
      await expect
        .poll(async () => (await api.getMedia(m.id)).variants.length, { timeout: 45_000 })
        .toBeGreaterThan(0);
    }
    await openLibrary(page, `?q=${w}`);
    await expect(grid(page).locator("[data-tile]")).toHaveCount(5);
    await expectAccessible(page);
    await snap(page, "media-grid");

    await page.getByRole("radio", { name: "List view" }).click();
    await loaded(page);
    await expect(page.getByRole("table", { name: "Media files" })).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-list");
    await page.getByRole("radio", { name: "Grid view" }).click();

    // A selection shows the bar.
    await pick(page, `${w} landscape 1`);
    await expect(bulkBar(page)).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-selection");
    await page.keyboard.press("Escape");

    await openLibrary(page, `?q=${word()}`);
    await expect(page.getByRole("heading", { name: /^No files match/ })).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-empty");

    const image = created[0];
    const doc = created[3];
    const logo = created[4];
    if (!image || !doc || !logo) throw new Error("fixtures missing");
    await openLibrary(page, `?id=${image.id}`);
    await expect(detail(page).getByRole("list", { name: "Variants" })).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-detail-image");
    await sheetToEnd(page);
    await expect(detail(page).getByRole("button", { name: "Move to trash" })).toBeInViewport();
    await snap(page, "media-detail-image-end");

    await openLibrary(page, `?id=${doc.id}`);
    await expect(detail(page).getByRole("link", { name: "Download" })).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-detail-pdf");

    await openLibrary(page, `?id=${logo.id}`);
    await expect(detail(page)).toContainText("Sanitised on upload");
    await expect(detail(page).locator("svg script")).toHaveCount(0);
    await expectAccessible(page);
    await snap(page, "media-detail-svg");

    // The Trash view with trashed files, a selection and its bulk bar, and a trashed
    // file's detail with Restore and Delete permanently.
    const binned = [
      await api.upload(png(`${uid("s")}.png`, 600, 400), { title: `${w} old banner` }),
      await api.upload(pdf(`${uid("s")}.pdf`), { title: `${w} old brochure` }),
    ];
    created.push(...binned);
    for (const m of binned) await api.trashMedia(m);
    await openLibrary(page, "?trash=true");
    await expect(tile(page, `${w} old banner`)).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-trash");
    await pick(page, `${w} old banner`);
    await expect(bulkBar(page).getByRole("button", { name: "Restore" })).toBeVisible();
    await expect(bulkBar(page).getByRole("button", { name: "Delete permanently" })).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-trash-selection");
    await page.keyboard.press("Escape");
    const trashed = binned[0];
    if (!trashed) throw new Error("fixtures missing");
    await openLibrary(page, `?trash=true&id=${trashed.id}`);
    await sheetToEnd(page);
    await expect(detail(page).getByRole("button", { name: "Restore" })).toBeInViewport();
    await expectAccessible(page);
    await snap(page, "media-detail-trashed");

    // The states the shared site never shows on its own, from a stubbed list.
    const list = (url: URL) => url.pathname === "/api/v1/media";
    const problem = (status: number, slug: string, detail: string) => ({
      status,
      contentType: "application/problem+json",
      body: JSON.stringify({
        type: `https://ferritecms.dev/problems/${slug}`,
        title: slug,
        status,
        detail,
      }),
    });
    await page.route(list, (route) =>
      route.request().method() === "GET"
        ? route.fulfill({ json: { items: [], next_cursor: null } })
        : route.fallback(),
    );
    await openLibrary(page);
    await expect(page.getByRole("heading", { name: "No media yet" })).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-none");

    await page.unroute(list);
    await page.route(list, (route) =>
      route.fulfill(problem(500, "internal", "The database is not answering.")),
    );
    await page.goto("/admin/media");
    await expect(page.getByRole("alert")).toContainText("The database is not answering.");
    await expectAccessible(page);
    await snap(page, "media-error");

    await page.unroute(list);
    await page.route(list, (route) => route.fulfill(problem(403, "forbidden", "Not allowed.")));
    await page.goto("/admin/media");
    await expect(page.getByRole("heading", { name: /permission|access|allowed/i })).toBeVisible();
    await expectAccessible(page);
    await snap(page, "media-forbidden");

    await page.unroute(list);
    await page.route(list, async (route) => {
      await new Promise((r) => setTimeout(r, 60_000));
      await route.continue().catch(() => undefined);
    });
    await page.goto("/admin/media");
    await expect(page.locator("[aria-busy=true]")).toHaveCount(1);
    await axeHeld(page);
    await snapHeld(page, "media-loading", backend);
    await page.unrouteAll({ behavior: "ignoreErrors" });
  } finally {
    for (const m of created) await api.purgeMedia(m.id);
    await api.dispose();
  }
});
