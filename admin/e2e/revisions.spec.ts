// The revisions drawer under the real server (plan Task 17; 07 §3, R26): every save is a
// revision, listed newest first with who saved it; the selected one is shown with what
// differs from the current item (or from the save before it) marked; Restore writes it
// back with `If-Match` and offers Undo, which goes back to what was current before; a
// `412` goes through the conflict flow; and a role that may not edit the item sees no
// revisions at all. Every item is created by the test itself (E2E data hygiene).

import type { Page } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { mod } from "./support/keys";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const body = (page: Page) => page.getByRole("textbox", { name: "Body" });
const drawer = (page: Page) => page.locator("[data-slot=revisions-drawer]");
const rows = (page: Page) => drawer(page).locator("[data-slot=revision-row]");
const detail = (page: Page) => drawer(page).locator("[data-slot=revision-detail]");
const entry = (page: Page) => page.locator("[data-slot=panel-revisions]");
const toast = (page: Page, text: string | RegExp) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });

/** A body of paragraphs (a heading first when `heading` is given). */
function doc(paragraphs: string[], heading?: string) {
  return {
    type: "doc",
    content: [
      ...(heading
        ? [{ type: "heading", attrs: { level: 2 }, content: [{ type: "text", text: heading }] }]
        : []),
      ...paragraphs.map((text) => ({ type: "paragraph", content: [{ type: "text", text }] })),
    ],
  };
}

async function openItem(page: Page, type: string, id: string): Promise<void> {
  await page.goto(`/admin/content/${type}/${id}`);
  await expect(body(page)).toBeVisible();
}

async function openDrawer(page: Page): Promise<void> {
  await body(page).click();
  await page.keyboard.press(`${await mod(page)}+Alt+r`);
  await expect(drawer(page)).toBeVisible();
}

type Stored = Item & { body: unknown };
const textOf = (item: Stored) => JSON.stringify(item.body);

async function revisionsOf(api: ContentApi, item: Item) {
  const res = await api.ctx.get(`/api/v1/content/${item.type}/${item.id}/revisions`);
  expect(res.status()).toBe(200);
  return ((await res.json()) as { items: { id: string; note?: string | null; title: string }[] })
    .items;
}

async function slotStatus(api: ContentApi, item: Item): Promise<number> {
  return (await api.ctx.get(`/api/v1/content/${item.type}/${item.id}/autosave`)).status();
}

test("three_saves_three_revisions_with_diff", async ({
  as,
  site,
  backend,
  snap,
  expectAccessible,
}) => {
  const api = await ContentApi.as(backend, site.url, "editor");
  const tag = uid();
  const t1 = `Planting notes ${tag}`;
  const t2 = `Spring planting notes ${tag}`;
  const t3 = `Spring planting guide ${tag}`;
  let item = await api.create("post", t1, {
    body: doc(
      [
        "Start the tomatoes indoors six weeks before the last frost.",
        "Beans go straight into the ground once the soil is warm.",
      ],
      "Seeds",
    ),
  });
  item = await api.patch(item, {
    title: t2,
    body: doc(
      [
        "Start the tomatoes and peppers indoors six weeks before the last frost.",
        "Beans go straight into the ground once the soil is warm.",
        "Mulch the beds after the first watering.",
      ],
      "Seeds",
    ),
  });
  item = await api.patch(item, {
    title: t3,
    body: doc(
      [
        "Start the peppers indoors eight weeks before the last frost.",
        "Beans and peas go straight into the ground once the soil is warm.",
        "Mulch the beds after the first watering.",
      ],
      "Seeds and seedlings",
    ),
  });

  const page = await as("editor");
  await openItem(page, "post", item.id);
  // The panel's entry counts them.
  await expect(entry(page)).toContainText("Revisions (3)");
  await openDrawer(page);

  // Three saves, newest first, with their titles and who saved them.
  await expect(rows(page)).toHaveCount(3);
  await expect(rows(page).nth(0)).toContainText(t3);
  await expect(rows(page).nth(1)).toContainText(t2);
  await expect(rows(page).nth(2)).toContainText(t1);
  await expect(rows(page).nth(0)).toContainText("Current");
  // Who saved it (the harness accounts have no display name: the login).
  await expect(rows(page).first()).toContainText(api.me.login);
  // The one before the current is selected and compared with the current item.
  await expect(rows(page).nth(1)).toHaveAttribute("aria-current", "true");
  const text = detail(page).locator("[data-slot=revision-text]");
  // Restoring it brings "tomatoes and" back and removes "and peas".
  await expect(text.locator("ins[data-diff=added]", { hasText: "tomatoes" })).toBeVisible();
  await expect(text.locator("del[data-diff=removed]", { hasText: "peas" })).toBeVisible();
  await expect(text.locator("ins", { hasText: "tomatoes" })).toHaveAttribute(
    "aria-label",
    /^Added: /,
  );
  await expect(text.locator("del", { hasText: "peas" })).toHaveAttribute(
    "aria-label",
    /^Removed: /,
  );
  await expect(detail(page).locator("[data-slot=revision-stats]")).toBeVisible();
  await snap(page, "revisions-drawer");
  await expectAccessible(page);

  // The same revision against the save before it: what that save changed.
  await detail(page).getByRole("radio", { name: "Previous save" }).click();
  await expect(text.locator("ins", { hasText: "Mulch the beds" })).toBeVisible();
  await expect(text.locator("ins", { hasText: "peppers" })).toBeVisible();
  // That save only added to the text ("Planting" → "Spring planting" in the title).
  await expect(text.locator('section[aria-label="Text"] del')).toHaveCount(0);
  await expect(
    text.locator('section[aria-label="Title"] ins', { hasText: "Spring" }),
  ).toBeVisible();
  // Selecting a row shows it (below 768 px the list gives way to it: the mobile shots).
  await rows(page).nth(1).click();
  await snap(page, "revisions-diff-previous");

  // The current revision is the current item: nothing to restore.
  await detail(page).getByRole("radio", { name: "Current" }).click();
  await rows(page).nth(0).click();
  await expect(text).toContainText("same as the current one");
  await expect(detail(page).getByRole("button", { name: "Restore this version" })).toBeDisabled();
  // The oldest has no previous save to compare with.
  await rows(page).nth(2).click();
  await expect(detail(page).getByRole("radio", { name: "Previous save" })).toBeDisabled();
  await snap(page, "revisions-diff-current");

  // ↑/↓ move through the list.
  await rows(page).nth(2).focus();
  await page.keyboard.press("ArrowUp");
  await expect(rows(page).nth(1)).toHaveAttribute("aria-current", "true");

  // The shortcut closes it again; nothing was written.
  await page.keyboard.press(`${await mod(page)}+Alt+r`);
  await expect(drawer(page)).toHaveCount(0);
  // The focus goes back where it was (the body).
  await expect(body(page)).toBeFocused();
  expect((await api.get("post", item.id)).version).toBe(item.version);
  await api.dispose();
});

test("restore_and_undo", async ({ as, site, backend, snap }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const older = `Older words ${uid()}`;
  const newer = `Newer words ${uid()}`;
  const created = await api.create("post", `Restore ${uid()}`, { body: doc([older]) });
  const item = await api.patch(created, { body: doc([newer]) });
  const [current, first] = await revisionsOf(api, item);
  expect(current && first).toBeTruthy();

  const page = await as("author");
  await openItem(page, "post", item.id);
  await expect(body(page)).toContainText(newer);

  // An unsaved edit: Restore asks first, Cancel keeps it.
  const typed = `Typed and not saved ${uid()}`;
  await body(page).click();
  await page.keyboard.press(`${await mod(page)}+End`);
  await page.keyboard.type(` ${typed}`);
  await entry(page).click();
  await expect(drawer(page)).toBeVisible();
  await expect(rows(page)).toHaveCount(2);
  await expect(rows(page).nth(1)).toHaveAttribute("aria-current", "true");
  const restore = detail(page).getByRole("button", { name: "Restore this version" });
  await restore.click();
  const confirm = page.locator("[data-slot=revision-restore-confirm]");
  await expect(confirm).toBeVisible();
  await expect(confirm).toContainText("Replace your unsaved edits?");
  await snap(page, "revisions-restore-confirm");
  await confirm.getByRole("button", { name: "Cancel" }).click();
  await expect(confirm).toHaveCount(0);
  // Still in the drawer (the editor behind it is inert), the edit still there.
  await expect(drawer(page)).toBeVisible();
  await expect(page.locator(".editor-body")).toContainText(typed);

  // Replace my edits: the older revision is written back with `If-Match`.
  const posts: { url: string; ifMatch: string | undefined }[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/revisions/")) {
      posts.push({ url: r.url(), ifMatch: r.headers()["if-match"] });
    }
  });
  await restore.click();
  await confirm.getByRole("button", { name: "Replace my edits" }).click();
  await expect(toast(page, "Version restored")).toBeVisible();
  await expect(drawer(page)).toHaveCount(0);
  await expect(body(page)).toContainText(older);
  await expect(body(page)).not.toContainText(typed);
  expect(posts).toEqual([
    {
      url: expect.stringContaining(`/revisions/${first?.id}/restore`) as unknown as string,
      ifMatch: `"${item.version}"`,
    },
  ]);
  const restored = (await api.get("post", item.id)) as Stored;
  expect(textOf(restored)).toContain(older);
  expect(restored.version).toBe(item.version + 1);
  const afterRestore = await revisionsOf(api, item);
  expect(afterRestore).toHaveLength(3);
  expect(afterRestore[0]?.note).toBe(`restored from ${first?.id}`);
  // The slot is gone and nothing of the replaced edits went back into it.
  await page.waitForLoadState("networkidle");
  expect(await slotStatus(api, item)).toBe(204);
  await expect(page.locator("[data-slot=save-state]").first()).toHaveAttribute(
    "data-state",
    "saved",
  );
  await snap(page, "revisions-restored-toast");

  // Undo: back to what was current before (the revision after the restore's snapshot).
  await toast(page, "Version restored").getByRole("button", { name: "Undo" }).click();
  await expect(toast(page, "Restore undone")).toBeVisible();
  await expect(body(page)).toContainText(newer);
  await expect(body(page)).not.toContainText(older);
  expect(posts[1]?.url).toContain(`/revisions/${current?.id}/restore`);
  expect(posts[1]?.ifMatch).toBe(`"${restored.version}"`);
  const undone = (await api.get("post", item.id)) as Stored;
  expect(textOf(undone)).toContain(newer);
  expect(undone.version).toBe(restored.version + 1);
  expect((await revisionsOf(api, item))[0]?.note).toBe(`restored from ${current?.id}`);

  // A reload shows the stored item: no restore banner, no old draft.
  await page.waitForLoadState("networkidle");
  expect(await slotStatus(api, item)).toBe(204);
  await page.reload();
  await expect(body(page)).toContainText(newer);
  await expect(page.locator("[data-slot=restore-banner]")).toHaveCount(0);
  await expect(body(page)).not.toContainText(typed);

  // The drawer lists the restores with their note.
  await openDrawer(page);
  await expect(rows(page)).toHaveCount(4);
  await expect(rows(page).nth(0).locator("[data-slot=revision-note]")).toContainText(
    "Restored from",
  );
  await api.dispose();
});

test("restore_conflict_goes_through_the_conflict_flow", async ({ as, site, backend, guard }) => {
  // The refused restore is a 412 by design: the browser logs that failed load.
  guard.allowFailedResource(/\/revisions\/[0-9a-f-]+\/restore$/);
  const api = await ContentApi.as(backend, site.url, "editor");
  const created = await api.create("post", `Conflict restore ${uid()}`, { body: doc(["One."]) });
  const item = await api.patch(created, { body: doc(["Two."]) });
  const page = await as("editor");
  await openItem(page, "post", item.id);
  await openDrawer(page);
  await expect(rows(page)).toHaveCount(2);

  // Someone saves after this screen loaded the item.
  const theirs = `Saved elsewhere ${uid()}`;
  await api.patch(item, { body: doc([theirs]) });
  await detail(page).getByRole("button", { name: "Restore this version" }).click();
  // Nothing unsaved here: the screen moves to their version and says the restore did not run.
  await expect(toast(page, "so the version was not restored")).toBeVisible();
  await expect(drawer(page)).toHaveCount(0);
  await expect(body(page)).toContainText(theirs);
  expect(textOf((await api.get("post", item.id)) as Stored)).toContain(theirs);
  await api.dispose();
});

test("contributor_sees_no_revisions_on_published", async ({ as, site, backend }) => {
  const contributor = await ContentApi.as(backend, site.url, "contributor");
  const editor = await ContentApi.as(backend, site.url, "editor");
  const draft = await contributor.create("post", `Contrib revisions ${uid()}`);
  const page = await as("contributor");

  // Their own draft: they may edit it, so its revisions are theirs to see.
  await openItem(page, "post", draft.id);
  await expect(entry(page)).toContainText("Revisions (1)");

  // Published by an editor: no longer editable for them (R26), so no revisions.
  const submitted = await contributor.transition(draft, "submit");
  await editor.transition(submitted, "publish");
  await openItem(page, "post", draft.id);
  await expect(page.getByRole("note")).toBeVisible();
  await expect(entry(page)).toHaveCount(0);
  await body(page).click();
  await page.keyboard.press(`${await mod(page)}+Alt+r`);
  await expect(drawer(page)).toHaveCount(0);
  const res = await contributor.ctx.get(`/api/v1/content/post/${draft.id}/revisions`);
  expect(res.status()).toBe(404);
  await contributor.dispose();
  await editor.dispose();
});
