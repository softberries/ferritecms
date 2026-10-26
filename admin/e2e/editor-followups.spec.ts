// Editor follow-ups from M2b-1 (M2b-2 Task 14) under the real server: edits made over a
// kept copy (after Take theirs) are backed up in the browser, so closing the page without
// saving does not lose them (ruling R13, plan P40); and `mod+shift+p` pressed on `/new`
// while the first save moves the item to `/$id` opens the publish dialog there (the M2b-1
// exit finding). Every item is created by the test itself (E2E data hygiene).

import type { Page } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { mod } from "./support/keys";
import { uid } from "./support/uid";

const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });
const body = (page: Page) => page.getByRole("textbox", { name: "Body" });
const indicator = (page: Page) => page.locator("[data-slot=editor-bar] [data-slot=save-state]");
const banner = (page: Page, mode: string) =>
  page.locator(`[data-slot=restore-banner][data-mode=${mode}]`);
const conflict = (page: Page) => page.locator("[data-slot=conflict-dialog]");
const publishDialog = (page: Page) => page.locator("[data-slot=publish-dialog]");

async function openItem(page: Page, type: string, id: string): Promise<void> {
  await page.goto(`/admin/content/${type}/${id}`);
  await expect(body(page)).toHaveAttribute("contenteditable", "true");
}

/** Types `text` as a new paragraph at the end of the body. */
async function typeAtEnd(page: Page, text: string): Promise<void> {
  await body(page).click();
  await page.keyboard.press(`${await mod(page)}+End`);
  await page.keyboard.press("Enter");
  await page.keyboard.type(text);
}

/** The local backups of item `id` in this page's storage, as text. */
const backupsOf = (page: Page, id: string) =>
  page.evaluate((item) => {
    const out: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key?.startsWith(`ferrite:backup:${item}:`)) out.push(localStorage.getItem(key) ?? "");
    }
    return out.join("\n");
  }, id);

test("kept_copy_edits_survive_a_crash", async ({
  as,
  site,
  backend,
  guard,
  snap,
  expectAccessible,
}) => {
  // The refused save is a 412 by design: the browser logs that failed load.
  guard.allowFailedResource(/\/api\/v1\/content\/post\/[0-9a-f-]+$/);
  const editorApi = await ContentApi.as(backend, site.url, "editor");
  const item = await editorApi.create("post", `Crash ${uid()}`);
  const a = await as("editor");
  const b = await as("admin");
  await openItem(a, "post", item.id);
  await openItem(b, "post", item.id);
  // A saves a new title; B saves over the old version and takes theirs (mine is kept).
  const theirs = `Their crash title ${uid()}`;
  await title(a).fill(theirs);
  await a.keyboard.press(`${await mod(a)}+s`);
  await expect(indicator(a)).toHaveAttribute("data-state", "saved");
  const mine = `My kept paragraph ${uid()}`;
  await typeAtEnd(b, mine);
  await b.keyboard.press(`${await mod(b)}+s`);
  await conflict(b).getByRole("button", { name: "Take theirs" }).click();
  await expect(banner(b, "kept")).toBeVisible();

  // New edits over their version: not autosaved (the slot holds mine), backed up here.
  const newer = `Edit that survives a crash ${uid()}`;
  await typeAtEnd(b, newer);
  await expect.poll(() => backupsOf(b, item.id)).toContain(newer);

  // The tab goes away without a word (no save, no beforeunload).
  const context = b.context();
  await b.close({ runBeforeUnload: false });
  const c = await context.newPage();
  await openItem(c, "post", item.id);
  // The slot (mine) opens as before; this browser's backup is offered beside it.
  await expect(banner(c, "restored")).toBeVisible();
  await expect(body(c)).toContainText(mine);
  const offer = banner(c, "backup");
  await expect(offer).toContainText("Unsaved edits from this browser");
  await snap(c, "editor-backup-offer");
  await expectAccessible(c);

  await offer.getByRole("button", { name: "Restore edits" }).click();
  await expect(body(c)).toContainText(newer);
  await expect(body(c)).not.toContainText(mine);
  await expect(title(c)).toHaveValue(theirs);
  // As before the crash: mine is kept in the slot, the edits are the working draft.
  await expect(banner(c, "kept")).toBeVisible();
  await expect(banner(c, "backup")).toHaveCount(0);

  // Saving stores the edits and ends the backup; the kept copy stays in the slot.
  await c.keyboard.press(`${await mod(c)}+s`);
  await expect(indicator(c)).toHaveAttribute("data-state", "saved");
  expect(JSON.stringify((await editorApi.get("post", item.id)).body)).toContain(newer);
  await expect.poll(() => backupsOf(c, item.id)).toBe("");
  await editorApi.dispose();
});

test("publish_dialog_survives_first_save", async ({ as, snap, expectAccessible }) => {
  const page = await as("editor");
  await page.goto("/admin/content/post/new");
  await title(page).fill(`Publish at once ${uid()}`);
  // At once: the first save is still to run (or running) when the key is pressed.
  await page.keyboard.press(`${await mod(page)}+Shift+p`);
  await expect(page).toHaveURL(/\/admin\/content\/post\/[0-9a-f-]{36}$/);
  await expect(publishDialog(page)).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Ready to publish?" })).toBeVisible();
  await snap(page, "editor-publish-after-first-save");
  await expectAccessible(page);
  await publishDialog(page).getByRole("button", { name: "Cancel" }).click();
  await expect(publishDialog(page)).toHaveCount(0);
});
