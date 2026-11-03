// The live preview beside the editor under the real server (M6-3, design 15 §3, §11.1;
// ruling P7 D16): typing shows in the preview within a few seconds without saving (the
// 2 s autosave debounce, then a reload of the sandboxed frame on a signed link); the split
// view from 1280 px with its divider, the Write | Preview control below, also on a phone;
// focus mode hides the pane; axe in each layout. Scroll is not kept across reloads (the
// frame is an opaque origin, P7). Every item is created by the test itself.

import type { Page } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { mod } from "./support/keys";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });
const pane = (page: Page) => page.locator("[data-slot=preview-pane]");
const frame = (page: Page) => page.frameLocator("[data-slot=preview-frame]");

test("typing_shows_in_the_split_preview_without_saving", async ({
  as,
  site,
  siteKey,
  snap,
  expectAccessible,
}) => {
  const author = await ContentApi.as(siteKey, site.url, "author");
  const name = `Live ${uid()}`;
  const item = await author.create("post", name, {
    body: {
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "First words." }] }],
    },
  });
  const page = await as("author");
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/admin/content/post/${item.id}`);
  await expect(title(page)).toBeVisible();

  // The bar's toggle (and `mod+alt+p`) opens the pane beside the text.
  await page.getByRole("button", { name: "Live preview" }).click();
  await expect(pane(page)).toBeVisible();
  const iframe = page.locator("[data-slot=preview-frame]");
  await expect(iframe).toHaveAttribute("sandbox", "allow-scripts allow-popups allow-forms");
  await expect(iframe).toHaveAttribute("title", `Preview of ${name}`);
  await expect(iframe).toHaveAttribute(
    "src",
    /^\/_preview\/link\/ferp_[A-Za-z0-9_-]{43}\?working=1/,
  );
  await expect(frame(page).getByText(name).first()).toBeVisible();
  // The side panel is a sheet in the split view.
  await expect(page.locator("aside[data-slot=editor-panel]")).toHaveCount(0);
  await expect(page.getByRole("separator", { name: "Resize the preview" })).toBeVisible();

  // Typing: the preview shows the new title within about 4 s, without a save.
  const saves: string[] = [];
  page.on("request", (r) => {
    if (r.method() === "PATCH" && r.url().includes(`/content/post/${item.id}`)) saves.push(r.url());
  });
  const next = `${name} edited`;
  await title(page).fill(next);
  await expect(frame(page).getByText(next).first()).toBeVisible({ timeout: 6_000 });
  await expect(page.locator("[data-slot=preview-status]")).toHaveText(/^Updated \d\d:\d\d:\d\d$/);
  expect(saves).toEqual([]);
  const stored = await author.get("post", item.id);
  expect(stored.title).toBe(name);

  // The divider moves by keyboard and is remembered.
  const divider = page.getByRole("separator", { name: "Resize the preview" });
  await divider.focus();
  await page.keyboard.press("Home");
  await expect(divider).toHaveAttribute("aria-valuenow", "35");
  await page.keyboard.press("ArrowRight");
  await expect(divider).toHaveAttribute("aria-valuenow", "40");
  await snap(page, "editor-live-preview-split");
  await expectAccessible(page);

  // Device widths.
  await pane(page).getByRole("radio", { name: "Phone" }).click();
  await expect(page.locator("[data-slot=preview-frame-box]")).toHaveCSS("width", "390px");
  await pane(page).getByRole("radio", { name: "Desktop" }).click();

  // Focus mode hides the pane; the shortcut closes it.
  await title(page).click();
  await page.keyboard.press(`${await mod(page)}+Shift+f`);
  await expect(pane(page)).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(pane(page)).toBeVisible();
  await page.keyboard.press(`${await mod(page)}+Alt+p`);
  await expect(pane(page)).toHaveCount(0);
  await author.dispose();
});

test("below_1280_px_write_and_preview_swap", async ({ as, site, siteKey, expectAccessible }) => {
  const author = await ContentApi.as(siteKey, site.url, "author");
  const name = `Swap ${uid()}`;
  const item = await author.create("post", name);
  const page = await as("author");
  await page.setViewportSize({ width: 1100, height: 800 });
  await page.goto(`/admin/content/post/${item.id}`);
  await expect(title(page)).toBeVisible();
  const view = page.getByRole("radiogroup", { name: "Writing or preview" });
  await view.getByRole("radio", { name: "Preview" }).click();
  await expect(pane(page)).toBeVisible();
  await expect(title(page)).toBeHidden();
  await expect(frame(page).getByText(name).first()).toBeVisible();
  await expectAccessible(page);
  await view.getByRole("radio", { name: "Write" }).click();
  await expect(pane(page)).toHaveCount(0);
  await expect(title(page)).toBeVisible();
  await author.dispose();
});

test("a_phone_uses_the_write_preview_control", async ({
  as,
  site,
  siteKey,
  snap,
  expectAccessible,
}) => {
  const author = await ContentApi.as(siteKey, site.url, "author");
  const name = `Phone ${uid()}`;
  const item = await author.create("post", name);
  const page = await as("author");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`/admin/content/post/${item.id}`);
  await expect(title(page)).toBeVisible();
  await expect(page.getByRole("button", { name: "Live preview" })).toHaveCount(0);
  const view = page.getByRole("radiogroup", { name: "Writing or preview" });
  await view.getByRole("radio", { name: "Preview" }).click();
  await expect(pane(page)).toBeVisible();
  await expect(frame(page).getByText(name).first()).toBeVisible();
  await snap(page, "editor-live-preview-phone");
  await expectAccessible(page);
  await author.dispose();
});

test("a_new_item_asks_for_a_save_first", async ({ as }) => {
  const page = await as("author");
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/admin/content/post/new");
  await expect(title(page)).toBeVisible();
  await page.getByRole("button", { name: "Live preview" }).click();
  await expect(pane(page).getByText("Save once to preview")).toBeVisible();
  await expect(page.locator("[data-slot=preview-frame]")).toHaveCount(0);
});
