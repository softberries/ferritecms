// The editor's side panel and the publishing flow under the real server (plan Task 15;
// 07 §3, 11 §1, Review Focus 4): what each role is offered comes from `allowed`; the
// publish dialog saves unsaved changes first and transitions with the version the save
// returned (F40); scheduling; the slug and its permalink; terms (an existing category and
// a tag created inline); trash from the editor with Undo (F36); and the review screens.
// Every item is created by the test itself (E2E data hygiene).

import type { Page, Request } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { mod } from "./support/keys";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

type Stored = Item & {
  terms: string[];
  permalink: string;
  scheduled_at?: string | null;
  allowed: { edit: boolean };
};

const panel = (page: Page) => page.locator("[data-slot=editor-panel]");
const body = (page: Page) => page.getByRole("textbox", { name: "Body" });
const title = (page: Page) => page.getByRole("textbox", { name: "Title" });
const dialog = (page: Page) => page.locator("[data-slot=publish-dialog]");
const toast = (page: Page, text: string | RegExp) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });

/** Opens the editor; on a wide screen the panel is open (the default). */
async function openItem(page: Page, type: string, id: string, wide = true): Promise<void> {
  await page.goto(`/admin/content/${type}/${id}`);
  await expect(title(page)).toBeVisible();
  if (wide) await expect(panel(page)).toBeVisible();
}

const isWrite = (r: Request, id: string) =>
  ["PATCH", "POST", "DELETE"].includes(r.method()) && r.url().includes(`/content/post/${id}`);

test("contributor_can_submit_not_publish", async ({
  as,
  site,
  backend,
  snap,
  expectAccessible,
}) => {
  const contributor = await ContentApi.as(backend, site.url, "contributor");
  const item = await contributor.create("post", `Contrib ${uid()}`);
  const page = await as("contributor");
  await openItem(page, "post", item.id);

  const actions = panel(page).locator("[data-slot=panel-actions]");
  await expect(actions.getByRole("button", { name: "Submit for review" })).toBeVisible();
  await expect(actions.getByRole("button", { name: "Save draft" })).toBeVisible();
  await expect(page.getByRole("button", { name: /^Publish/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Schedule/ })).toHaveCount(0);
  // The publish shortcut does nothing without `allowed.publish`.
  await body(page).click();
  await page.keyboard.press(`${await mod(page)}+Shift+p`);
  await expect(dialog(page)).toHaveCount(0);
  await snap(page, "editor-panel-contributor");
  await expectAccessible(page);

  // An unsaved edit is saved first, then the item is submitted with the new version.
  await page.keyboard.type("A first paragraph.");
  const writes: Request[] = [];
  page.on("request", (r) => {
    if (isWrite(r, item.id)) writes.push(r);
  });
  await actions.getByRole("button", { name: "Submit for review" }).click();
  await expect(toast(page, "Submitted for review")).toBeVisible();
  expect(writes.map((r) => `${r.method()} ${new URL(r.url()).pathname.split("/").at(-1)}`)).toEqual(
    ["PATCH " + item.id, "POST submit"],
  );
  const stored = (await contributor.get("post", item.id)) as Stored;
  expect(stored.status).toBe("pending");
  // The submit carries the version the PATCH returned.
  expect(writes[1]?.headers()["if-match"]).toBe((await writes[0]?.response())?.headers().etag);
  await expect(panel(page).locator("[data-slot=status-badge]")).toHaveText("Pending review");
  // Still no way to publish. The server keeps `edit` for a contributor's own pending item
  // (07 §1), so the editor stays editable and says it waits for review; the read-only
  // branch (`edit` false after a transition) is the unit test
  // `read_only_after_a_transition_takes_edit_away`.
  await expect(page.getByRole("button", { name: /^Publish/ })).toHaveCount(0);
  expect(stored.allowed.edit).toBe(true);
  await expect(body(page)).toHaveAttribute("contenteditable", "true");
  await expect(panel(page).getByText("Waiting for review")).toBeVisible();
  await expect(page.getByRole("note")).toHaveCount(0);
  await contributor.dispose();
});

test("author_publishes_and_unpublishes", async ({ as, site, backend }) => {
  const author = await ContentApi.as(backend, site.url, "author");
  const item = await author.create("post", `Author ${uid()}`);
  const page = await as("author");
  await openItem(page, "post", item.id);

  // Unsaved: a new title. The dialog says it will be saved first.
  const name = `Published ${uid()}`;
  await title(page).fill(name);
  await panel(page).getByRole("button", { name: "Publish…" }).click();
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page).locator("[data-slot=publish-summary]")).toContainText(name);
  await expect(dialog(page)).toContainText("Your unsaved changes are saved first.");
  const writes: Request[] = [];
  page.on("request", (r) => {
    if (isWrite(r, item.id)) writes.push(r);
  });
  await dialog(page).getByRole("button", { name: "Publish now" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(toast(page, "Published")).toBeVisible();
  // F40: the PATCH, then the publish with the version the PATCH returned.
  expect(writes.map((r) => r.method())).toEqual(["PATCH", "POST"]);
  const patched = (await writes[0]?.response())?.headers().etag;
  expect(writes[1]?.headers()["if-match"]).toBe(patched);
  expect(JSON.parse(writes[0]?.postData() ?? "{}")).toEqual({ title: name });
  let stored = (await author.get("post", item.id)) as Stored;
  expect(stored.status).toBe("published");
  expect(stored.title).toBe(name);
  await expect(panel(page).locator("[data-slot=status-badge]")).toHaveText("Published");
  await expect(panel(page).getByRole("button", { name: "Update" })).toBeDisabled();

  await panel(page).getByRole("button", { name: "Unpublish" }).click();
  await expect(toast(page, "Unpublished")).toBeVisible();
  await expect(panel(page).locator("[data-slot=status-badge]")).toHaveText("Draft");
  stored = (await author.get("post", item.id)) as Stored;
  expect(stored.status).toBe("draft");
  await author.dispose();
});

test("editor_schedules", async ({ as, site, backend }) => {
  const editor = await ContentApi.as(backend, site.url, "editor");
  const unique = `w${uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-10)}`;
  const item = await editor.create("post", `Scheduled ${unique}`);
  const page = await as("editor");

  const tile = async () => {
    const text = await page
      .getByRole("region", { name: "Posts at a glance" })
      .getByRole("link", { name: /^Scheduled/ })
      .locator("[data-slot=metric-value]")
      .textContent();
    return Number((text ?? "").replace(/\D/g, ""));
  };
  await openItem(page, "post", item.id);
  await page.keyboard.press(`${await mod(page)}+Shift+p`);
  await expect(dialog(page)).toBeVisible();
  await dialog(page).getByText("Later", { exact: true }).click();
  // Tomorrow (the default day) at 10:15 on this browser's clock.
  const time = dialog(page).getByLabel("Time", { exact: true });
  await time.fill("10:15");
  await expect(dialog(page).locator("[data-slot=schedule-zone]")).toContainText("UTC");
  const expected = await page.evaluate(() => {
    const d = new Date();
    d.setDate(d.getDate() + 1);
    d.setHours(10, 15, 0, 0);
    return d.toISOString().replace(/\.\d{3}Z$/, "Z");
  });
  await expect(dialog(page).locator("[data-slot=schedule-zone]")).toContainText(
    `${expected.slice(0, 16).replace("T", " ")} UTC`,
  );
  await dialog(page)
    .getByRole("button", { name: /^Schedule for/ })
    .click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(toast(page, "Scheduled for")).toBeVisible();
  const stored = (await editor.get("post", item.id)) as Stored;
  expect(stored.status).toBe("scheduled");
  expect(Date.parse(stored.scheduled_at ?? "")).toBe(Date.parse(expected));
  await expect(panel(page).locator("[data-slot=panel-scheduled-at]")).toBeVisible();
  await expect(panel(page).getByRole("button", { name: "Unschedule" })).toBeVisible();

  // The list shows when.
  await page.goto(`/admin/content/post?q=${unique}`);
  const row = page.getByRole("table", { name: "Posts" }).locator("tbody tr[data-row]");
  await expect(row).toHaveCount(1);
  const when = row.locator(`time[datetime="${stored.scheduled_at ?? ""}"]`);
  await expect(when).toBeVisible();
  await expect(when).toHaveText(/^for /);

  // The dashboard's Scheduled tile counts it (polled: the other worker writes posts too).
  await expect
    .poll(
      async () => {
        await page.goto("/admin/");
        await expect(page.locator("[data-slot=tile-skeleton]")).toHaveCount(0);
        const shown = await tile();
        const res = await page.request.get("/api/v1/content/post/counts");
        const counts = (await res.json()) as { scheduled: number };
        return shown === counts.scheduled ? "equal" : { shown, counts };
      },
      { timeout: 30_000 },
    )
    .toBe("equal");
  // …and the count includes this post.
  const listed = await page.request.get(`/api/v1/content/post?status=scheduled&q=${unique}`);
  expect(((await listed.json()) as { items: { id: string }[] }).items.map((i) => i.id)).toEqual([
    item.id,
  ]);
  await editor.purge(item);
  await editor.dispose();
});

test("slug_change_updates_permalink", async ({ as, site, backend }) => {
  const admin = await ContentApi.as(backend, site.url, "admin");
  const item = (await admin.create("post", `Slug ${uid()}`)) as Stored;
  const page = await as("admin");
  await openItem(page, "post", item.id);
  const permalink = panel(page).locator("[data-slot=permalink]");
  await expect(permalink).toContainText(item.permalink);

  const slug = `renamed-${uid("").slice(-8)}`;
  await panel(page).getByLabel("Slug").fill(slug);
  await expect(permalink).toContainText(`${slug}/`);
  await expect(permalink).toContainText("after you save");
  const patch = page.waitForRequest((r) => r.method() === "PATCH" && isWrite(r, item.id));
  await page.keyboard.press(`${await mod(page)}+s`);
  // Only the changed member goes out.
  expect(JSON.parse((await patch).postData() ?? "{}")).toEqual({ slug });
  await expect(page.locator("[data-slot=save-state]")).toHaveAttribute("data-state", "saved");
  const stored = (await admin.get("post", item.id)) as Stored;
  expect(stored.slug).toBe(slug);
  await expect(permalink).toContainText(stored.permalink);
  await expect(permalink).not.toContainText("after you save");
  await admin.dispose();
});

test("assign_category_and_new_tag", async ({ as, site, backend }) => {
  const editor = await ContentApi.as(backend, site.url, "editor");
  const category = await editor.createTerm("category", `Cat ${uid("").slice(-8)}`);
  const tagName = `tag-${uid("").slice(-8)}`;
  const item = await editor.create("post", `Terms ${uid()}`);
  const page = await as("editor");
  let tagId: string | undefined;
  try {
    await openItem(page, "post", item.id);
    const categories = panel(page).locator("[data-slot=panel-terms][data-taxonomy=category]");
    await categories.getByRole("checkbox", { name: category.name }).check();
    const tags = panel(page).locator("[data-slot=panel-terms][data-taxonomy=tag]");
    await tags.getByRole("button", { name: /^Add / }).click();
    await page.keyboard.type(tagName);
    await page.getByRole("option", { name: `Create “${tagName}”` }).click();
    await expect(tags.getByRole("button", { name: `Remove ${tagName}` })).toBeVisible();
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(page.locator("[data-slot=save-state]")).toHaveAttribute("data-state", "saved");

    const res = await editor.ctx.get("/api/v1/taxonomies/tag/terms");
    const all = ((await res.json()) as { items: { id: string; name: string }[] }).items;
    tagId = all.find((t) => t.name === tagName)?.id;
    expect(tagId).toBeTruthy();
    const stored = (await editor.get("post", item.id)) as Stored;
    expect(stored.terms).toEqual([category.id, tagId]);
    // After a reload both are still chosen.
    await page.reload();
    await expect(categories.getByRole("checkbox", { name: category.name })).toBeChecked();
    await expect(tags.getByRole("button", { name: `Remove ${tagName}` })).toBeVisible();
  } finally {
    await editor.deleteTerm(category.id);
    if (tagId) await editor.deleteTerm(tagId);
    await editor.dispose();
  }
});

test("trash_from_editor_with_undo", async ({ as, site, backend }) => {
  const editor = await ContentApi.as(backend, site.url, "editor");
  const item = await editor.createIn("post", `Trash ${uid()}`, "published");
  const page = await as("editor");
  await openItem(page, "post", item.id);
  await panel(page).getByRole("button", { name: "Move to trash" }).click();
  await expect(page).toHaveURL(/\/admin\/content\/post$/);
  const trashed = toast(page, "1 item moved to trash");
  await expect(trashed).toBeVisible();
  expect((await editor.get("post", item.id)).status).toBe("trash");
  await trashed.getByRole("button", { name: "Undo" }).click();
  await expect(toast(page, "1 item restored")).toBeVisible();
  // F36: back published, not a draft.
  await expect.poll(async () => (await editor.get("post", item.id)).status).toBe("published");
  await editor.dispose();
});

test("unreadable_parent_settles", async ({ as, site, backend, guard, snap, expectAccessible }) => {
  // A state the server allows: children (one published, one the author's own draft)
  // under someone else's draft parent, which the author may not read (404). On its own
  // type, removed afterwards, so no other spec's list meets it.
  const admin = await ContentApi.as(backend, site.url, "admin");
  const author = await ContentApi.as(backend, site.url, "author");
  const type = uid("t").slice(0, 32);
  const label = `Docs ${type.slice(-6)}`;
  await admin.createType(type, label, true);
  try {
    const parent = await admin.create(type, `Hidden parent ${uid("").slice(-6)}`);
    const published = await admin.createIn(type, `Public child ${uid("").slice(-6)}`, "published", {
      parent_id: parent.id,
    });
    const mine = await author.create(type, `My child ${uid("").slice(-6)}`, {
      parent_id: parent.id,
    });
    // The browser logs its own line for exactly this lookup; nothing else may fail.
    const lookup = new RegExp(`/api/v1/content/${type}/${parent.id}$`);
    guard.allowFailedResource(lookup);
    const page = await as("author");
    const lookups: string[] = [];
    page.on("request", (r) => {
      if (lookup.test(r.url())) lookups.push(r.url());
    });

    // The list: a settled, muted "Parent not available" for both children, asked once.
    await page.goto(`/admin/content/${type}`);
    const rows = page.getByRole("table", { name: label }).locator("tbody tr[data-row]");
    await expect(rows).toHaveCount(2);
    for (const title of [published.title, mine.title]) {
      await expect(
        rows.filter({ hasText: title }).locator("[data-slot=parent-unavailable]"),
      ).toHaveText(/^Parent not available/);
    }
    await expect(rows.getByText("…")).toHaveCount(0);
    expect(lookups).toHaveLength(1);
    await snap(page, "content-list-parent-unavailable");
    await expectAccessible(page);

    // The editor: the field says so, and a save keeps the parent.
    await openItem(page, type, mine.id);
    const trigger = panel(page).locator("[data-slot=parent-trigger]");
    await expect(trigger).toContainText("Parent not available");
    await expect(panel(page).locator("p[data-slot=parent-unavailable]")).toBeVisible();
    await snap(page, "editor-panel-parent-unavailable");
    await expectAccessible(page);
    const name = `Renamed ${uid("").slice(-6)}`;
    await title(page).fill(name);
    const patch = page.waitForRequest((r) => r.method() === "PATCH" && r.url().includes(mine.id));
    await page.keyboard.press(`${await mod(page)}+s`);
    expect(JSON.parse((await patch).postData() ?? "{}")).toEqual({ title: name });
    await expect(page.locator("[data-slot=save-state]")).toHaveAttribute("data-state", "saved");
    expect((await admin.get(type, mine.id)).parent_id).toBe(parent.id);

    // …and another parent can still be chosen (here: none).
    await trigger.click();
    await page.getByRole("option", { name: "No parent (top level)" }).click();
    await expect(trigger).toContainText("No parent (top level)");
    await page.keyboard.press(`${await mod(page)}+s`);
    await expect(page.locator("[data-slot=save-state]")).toHaveAttribute("data-state", "saved");
    expect((await admin.get(type, mine.id)).parent_id ?? null).toBeNull();
  } finally {
    await admin.removeType(type);
    await Promise.all([admin.dispose(), author.dispose()]);
  }
});

test("phone_publish_bar", async ({ as, site, backend }) => {
  // Below 1024 px the panel is a sheet, so the status and the primary action live in a
  // bar at the foot of the editor (same `allowed` rules, same F40 flow).
  const author = await ContentApi.as(backend, site.url, "author");
  const contributor = await ContentApi.as(backend, site.url, "contributor");
  const post = await author.create("post", `Phone ${uid()}`);
  const draft = await contributor.create("post", `Phone contrib ${uid()}`);
  try {
    const page = await as("author");
    await page.setViewportSize({ width: 390, height: 844 });
    await openItem(page, "post", post.id, false);
    const bar = page.getByRole("region", { name: "Publishing" });
    await expect(bar).toContainText("Draft");
    await bar.getByRole("button", { name: "Publish…" }).click();
    await dialog(page).getByRole("button", { name: "Publish now" }).click();
    await expect(toast(page, "Published")).toBeVisible();
    await expect(bar).toContainText("Published");
    await expect(bar.getByRole("button", { name: "Update" })).toBeDisabled();
    expect((await author.get("post", post.id)).status).toBe("published");
    await bar.getByRole("button", { name: "Settings" }).click();
    await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();

    const other = await as("contributor");
    await other.setViewportSize({ width: 390, height: 844 });
    await openItem(other, "post", draft.id, false);
    const cbar = other.getByRole("region", { name: "Publishing" });
    await expect(cbar.getByRole("button", { name: "Submit for review" })).toBeVisible();
    await expect(cbar.getByRole("button", { name: /^Publish/ })).toHaveCount(0);
  } finally {
    await Promise.all([author.dispose(), contributor.dispose()]);
  }
});

test("screens", async ({ as, site, backend, snap, expectAccessible }) => {
  test.setTimeout(300_000);
  for (const role of ["admin", "editor", "author"] as const) {
    const api = await ContentApi.as(backend, site.url, role);
    const item = await api.create("post", `Notes from the ${role} desk`, {
      body: {
        type: "doc",
        content: [
          {
            type: "paragraph",
            content: [{ type: "text", text: "A short draft to show the side panel with." }],
          },
        ],
      },
    });
    const page = await as(role);
    await openItem(page, "post", item.id);
    await snap(page, `editor-panel-${role}`);
    await expectAccessible(page);

    if (role === "admin") {
      // The publish dialog, now and later.
      await panel(page).getByRole("button", { name: "Publish…" }).click();
      await expect(dialog(page)).toBeVisible();
      await snap(page, "editor-publish-dialog");
      await expectAccessible(page);
      await dialog(page).getByText("Later", { exact: true }).click();
      await expect(dialog(page).locator("[data-slot=schedule-picker]")).toBeVisible();
      await snap(page, "editor-schedule-picker");
      await expectAccessible(page);
      await page.keyboard.press("Escape");
      await expect(dialog(page)).toHaveCount(0);

      // A published page with a parent (the hierarchical fields).
      // Drafts, so no other account's page list meets them; removed afterwards.
      const parent = await api.create("page", `Parent ${uid("").slice(-6)}`);
      const child = await api.create("page", `Child ${uid("").slice(-6)}`, {
        parent_id: parent.id,
      });
      try {
        await openItem(page, "page", child.id);
        await expect(panel(page).locator("[data-slot=parent-trigger]")).toContainText(parent.title);
        await snap(page, "editor-panel-page");
        await expectAccessible(page);
      } finally {
        await api.purge(child);
        await api.purge(parent);
      }

      // The phone sheet.
      await page.setViewportSize({ width: 390, height: 844 });
      await openItem(page, "post", item.id, false);
      await expect(panel(page)).toHaveCount(0);
      await page
        .getByRole("region", { name: "Publishing" })
        .getByRole("button", { name: "Settings" })
        .click();
      await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
      await snap(page, "editor-panel-sheet");
      await expectAccessible(page);
      await page.keyboard.press("Escape");
      await page.setViewportSize({ width: 1440, height: 900 });
    }
    await api.dispose();
  }
});
