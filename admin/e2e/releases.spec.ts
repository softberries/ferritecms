// Releases under the real server (M6-3, design 15 §4, §5, §11.2–11.3): from the editor, a
// published page's edit goes into a new release as a staged change (the editor then shows
// change mode) and a new draft page joins it to be published; Preview site shows both as
// they will be; Apply now puts both live in one step. A conflict after an edit elsewhere
// is rebased; a scheduled release locks its item's editor. axe on the list, the detail and
// the apply dialog. Every item and release is the test's own.

import type { Page } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

interface Release {
  id: string;
  title: string;
  status: string;
  version: number;
  entries: { item_id: string; conflict: boolean }[];
}

const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });
const panel = (page: Page) => page.locator("[data-slot=editor-panel]");
const addDialog = (page: Page) => page.locator("[data-slot=add-to-release-dialog]");
const detail = (page: Page) => page.locator("[data-slot=release-detail]");

async function openEditor(page: Page, item: Item) {
  await page.goto(`/admin/content/${item.type}/${item.id}`);
  await expect(title(page)).toBeVisible();
  await expect(panel(page)).toBeVisible();
}

test("a_release_from_the_editor_previews_and_applies", async ({
  as,
  site,
  siteKey,
  snap,
  expectAccessible,
}) => {
  test.setTimeout(120_000);
  const api = await ContentApi.as(siteKey, site.url, "editor");
  const tag = uid();
  const about = await api.createIn("page", `About ${tag}`, "published");
  const launch = await api.create("page", `Launch ${tag}`);
  const releaseTitle = `Spring ${tag}`;
  const page = await as("editor");
  await page.setViewportSize({ width: 1280, height: 900 });
  try {
    // A published page's edit, staged in a new release.
    await openEditor(page, about);
    await title(page).fill(`About ${tag} (new)`);
    await panel(page).getByRole("button", { name: "Add to release…" }).click();
    await expect(addDialog(page)).toBeVisible();
    await addDialog(page).getByRole("radio", { name: "New release" }).check();
    await addDialog(page).getByLabel("Title of the new release").fill(releaseTitle);
    await expect(
      addDialog(page).getByRole("checkbox", { name: "Include my changes" }),
    ).toBeChecked();
    await expectAccessible(page);
    await addDialog(page).getByRole("button", { name: "Add to release" }).click();
    // Change mode: the staged change, edited through the release.
    await expect(page.locator("[data-slot=change-mode-banner]")).toContainText(releaseTitle);
    await expect(page).toHaveURL(/\?release=/);
    await expect(title(page)).toHaveValue(`About ${tag} (new)`);
    // The live page is unchanged.
    expect((await api.get("page", about.id)).title).toBe(`About ${tag}`);

    // The new page joins the release, to be published.
    await openEditor(page, launch);
    await panel(page).getByRole("button", { name: "Add to release…" }).click();
    await addDialog(page).getByRole("radio", { name: releaseTitle }).check();
    await addDialog(page).getByRole("radio", { name: "Publish it" }).check();
    await addDialog(page).getByRole("button", { name: "Add to release" }).click();
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: releaseTitle }),
    ).toBeVisible();

    // The release lists both.
    await page.goto("/admin/releases");
    await expect(page.locator("[data-slot=releases-screen]")).toBeVisible();
    await expectAccessible(page);
    await page.getByRole("table", { name: "Releases" }).getByText(releaseTitle).click();
    await expect(detail(page)).toBeVisible();
    const entries = page.getByRole("table", { name: "Entries" });
    await expect(entries.getByRole("row")).toHaveCount(3);
    await expect(entries.getByRole("row", { name: new RegExp(`About ${tag}`) })).toContainText(
      "Changes",
    );
    await expect(entries.getByRole("row", { name: new RegExp(`Launch ${tag}`) })).toContainText(
      "Publish",
    );
    await snap(page, "release-detail");
    await expectAccessible(page);

    // Preview site: the whole site as the release leaves it, in a new tab.
    const [preview] = await Promise.all([
      page.waitForEvent("popup"),
      detail(page).getByRole("button", { name: "Preview site" }).click(),
    ]);
    await preview.waitForURL((u) => u.pathname === "/");
    await preview.goto(`/${about.slug}/`);
    await expect(preview.getByText(`About ${tag} (new)`).first()).toBeVisible();
    await preview.goto(`/${launch.slug}/`);
    await expect(preview.getByText(`Launch ${tag}`).first()).toBeVisible();
    await preview.close();

    // Apply now: both live in one step.
    await detail(page).getByRole("button", { name: "Apply now…" }).click();
    const apply = page.locator("[data-slot=apply-dialog]");
    await expect(apply.getByRole("list", { name: "What goes live" })).toContainText(
      `Launch ${tag}`,
    );
    await expectAccessible(page);
    await apply.getByRole("button", { name: "Apply now" }).click();
    await expect(detail(page).locator("[data-slot=release-badge]")).toHaveText("Applied");
    const nowAbout = await api.get("page", about.id);
    const nowLaunch = await api.get("page", launch.id);
    expect(nowAbout.title).toBe(`About ${tag} (new)`);
    expect(nowLaunch.status).toBe("published");
    const live = await page.request.get(`/${launch.slug}/`);
    expect(live.status()).toBe(200);
  } finally {
    await api.purge(about);
    await api.purge(launch);
    await api.dispose();
  }
});

test("a_conflict_rebases_and_a_schedule_locks_the_editor", async ({ as, site, siteKey }) => {
  test.setTimeout(90_000);
  const api = await ContentApi.as(siteKey, site.url, "editor");
  const tag = uid();
  const item = await api.createIn("page", `Locked ${tag}`, "published");
  let release = await api.call<Release>("POST", "/releases", {
    data: { title: `Conflict ${tag}` },
  });
  const page = await as("editor");
  try {
    release = await api.call<Release>("POST", `/releases/${release.id}/items`, {
      version: release.version,
      data: {
        item_id: item.id,
        transition: "none",
        changes: { title: `Locked ${tag} staged`, body: { type: "doc", content: [] } },
      },
    });
    // Someone saves the item meanwhile: the entry conflicts.
    await api.patch(await api.get("page", item.id), { title: `Locked ${tag} edited` });
    await page.goto(`/admin/releases/${release.id}`);
    const row = page.getByRole("table", { name: "Entries" }).getByRole("row").nth(1);
    await expect(row.locator("[data-slot=entry-conflict]")).toBeVisible();
    await row.getByRole("button", { name: "Rebase" }).click();
    await expect(row.locator("[data-slot=entry-conflict]")).toHaveCount(0);

    // Scheduled: the item's editor says it is locked.
    const fresh = await api.call<Release>("GET", `/releases/${release.id}`);
    const at = new Date(Date.now() + 86_400_000).toISOString();
    await api.call("POST", `/releases/${release.id}/schedule`, {
      version: fresh.version,
      data: { at },
    });
    await page.goto(`/admin/content/page/${item.id}`);
    await expect(page.locator("[data-slot=read-only-banner]")).toContainText(
      `Locked by the scheduled release “Conflict ${tag}”`,
    );
    await expect(page.getByRole("textbox", { name: "Body" })).toHaveAttribute(
      "contenteditable",
      "false",
    );
  } finally {
    const now = await api.call<Release>("GET", `/releases/${release.id}`);
    let version = now.version;
    if (now.status === "scheduled") {
      version = (await api.call<Release>("POST", `/releases/${release.id}/unschedule`, { version }))
        .version;
    }
    await api.call("DELETE", `/releases/${release.id}`, { version });
    await api.purge(item);
    await api.dispose();
  }
});
