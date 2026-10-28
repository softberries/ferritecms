// The Settings screen under the real server (M2b-3 Task 8; 07 §3 Settings): a title saved
// with `mod+s` shows on the public site; a page picked as the front page is what `/`
// shows; a permalink change says first that URLs move without redirects (Review Focus 4)
// and a post's URL then follows the new structure; two tabs saving the same group meet
// the stale question (P58); the Mail panel without mail; and the review screens.
//
// Settings are site-wide and other specs run beside this one, so each test changes as
// little as it can for as short as it can, and puts back what it changed in `finally`.
// The tests of one file run one after the other.

import type { Page, Request } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { mod } from "./support/keys";
import { mailbox } from "./support/smtp-sink";
import { uid } from "./support/uid";

const screen = (page: Page) => page.locator("[data-slot=settings-screen]");
const group = (page: Page) => page.locator("[data-slot=settings-group]");
const saveButton = (page: Page) => group(page).getByRole("button", { name: "Save changes" });
const toast = (page: Page, text: string) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });
const isPatch = (r: Request) =>
  r.method() === "PATCH" && new URL(r.url()).pathname === "/api/v1/settings";

/** Before a snap after typing or clicking: no focused control, the page at its top. */
async function atRest(page: Page): Promise<void> {
  await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
    window.scrollTo(0, 0);
  });
}

async function open(page: Page, name: string): Promise<void> {
  await page.goto(`/admin/settings/${name}`);
  await expect(screen(page)).toBeVisible();
  await expect(group(page)).toHaveAttribute("data-group", name);
  await expect(group(page).locator("[aria-busy=true]")).toHaveCount(0);
}

/** A published item of `type` titled `title`. */
async function published(api: ContentApi, type: string, title: string): Promise<Item> {
  return api.transition(await api.create(type, title), "publish");
}

test("save_general_and_see_title_on_site", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const before = await api.getSettings();
  const page = await as("admin");
  const title = uid("Site");
  try {
    await open(page, "general");
    const field = page.getByRole("textbox", { name: "Site title" });
    await field.fill(title);
    await expect(group(page).locator("[data-slot=settings-pending]")).toHaveText(
      "1 unsaved change",
    );
    // Saved from the keyboard, inside the field.
    const patch = page.waitForRequest(isPatch);
    await field.press(`${await mod(page)}+s`);
    expect((await patch).postDataJSON()).toEqual({ "site.title": title });
    await expect(toast(page, "Settings saved")).toBeVisible();
    await expect(saveButton(page)).toBeDisabled();

    // The public site names itself by it.
    await expect
      .poll(async () => (await (await page.request.get("/")).text()).includes(title), {
        timeout: 10_000,
      })
      .toBe(true);
    // A reload keeps it.
    await page.reload();
    await expect(page.getByRole("textbox", { name: "Site title" })).toHaveValue(title);
  } finally {
    await api.patchSettings({ "site.title": before["site.title"] ?? null });
    await api.dispose();
  }
});

test("front_page_choice_changes_home", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const before = await api.getSettings();
  const title = uid("Front");
  const home = await published(api, "page", title);
  const page = await as("admin");
  try {
    await open(page, "reading");
    await page.getByRole("radio", { name: /A page/ }).check();
    await page.getByRole("button", { name: /^Page/ }).click();
    await page.getByPlaceholder("Search published pages…").fill(title);
    await page.getByRole("option", { name: new RegExp(title) }).click();
    // Named by `GET /content/refs`, not by an id.
    await expect(page.locator("[data-slot=front-page-picker]")).toContainText(title);
    const patch = page.waitForRequest(isPatch);
    await saveButton(page).click();
    expect((await patch).postDataJSON()).toEqual({ "reading.front_page": home.id });
    await expect(toast(page, "Settings saved")).toBeVisible();

    await expect
      .poll(async () => (await (await page.request.get("/")).text()).includes(title), {
        timeout: 10_000,
      })
      .toBe(true);
    // A reload names the stored page again.
    await page.reload();
    await expect(page.locator("[data-slot=front-page-picker]")).toContainText(title);
  } finally {
    await api.patchSettings({ "reading.front_page": before["reading.front_page"] ?? null });
    await api.purge(home);
    await api.dispose();
  }
});

test("permalink_change_warns", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const before = await api.getSettings();
  const post = await published(api, "post", uid("Moved"));
  const page = await as("admin");
  try {
    await open(page, "permalinks");
    await page.getByRole("radio", { name: /Month and name/ }).check();
    const year = String(new Date().getUTCFullYear());
    await expect(page.locator("[data-slot=permalink-post-preview]")).toContainText(
      new RegExp(`/${year}/\\d{2}/sample-post/$`),
    );
    // Every settings request from here: the stale re-read (`GET`) and the `PATCH` come
    // only after the confirm.
    const calls: string[] = [];
    page.on("request", (r) => {
      if (new URL(r.url()).pathname === "/api/v1/settings") calls.push(r.method());
    });
    await saveButton(page).click();
    const confirm = page.getByRole("alertdialog", { name: "Change the permalinks?" });
    await expect(confirm).toContainText(
      "Every post or page whose address follows the changed structure moves to a new URL. " +
        "No redirects are created from the old URLs, so links to them elsewhere stop working.",
    );
    // Cancel sends nothing (checked again below, after a save that does send).
    await confirm.getByRole("button", { name: "Cancel" }).click();
    await expect(confirm).toBeHidden();
    expect(calls).toEqual([]);

    await saveButton(page).click();
    const patch = page.waitForRequest(isPatch);
    await confirm.getByRole("button", { name: "Change permalinks" }).click();
    expect((await patch).postDataJSON()).toEqual({ "permalink.post": "/%year%/%month%/%slug%/" });
    await expect(toast(page, "Settings saved")).toBeVisible();
    // One re-read and one write, both from the confirmed save: a late request from the
    // cancelled one would show here.
    expect(calls).toEqual(["GET", "PATCH"]);

    // The post's URL follows the new structure (UTC year and month of publication).
    const at = new Date(post.published_at ?? "");
    const expected = `/${at.getUTCFullYear()}/${String(at.getUTCMonth() + 1).padStart(2, "0")}/${post.slug}/`;
    await expect
      .poll(async () => (await api.refs([post.id]))[0]?.permalink, { timeout: 10_000 })
      .toBe(expected);
    await expect
      .poll(async () => (await page.request.get(expected)).status(), { timeout: 10_000 })
      .toBe(200);
  } finally {
    await api.patchSettings({ "permalink.post": before["permalink.post"] ?? null });
    await api.purge(post);
    await api.dispose();
  }
});

test("settings_stale_guard", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const before = await api.getSettings();
  const a = await as("admin");
  const b = await as("admin");
  try {
    await open(a, "comments");
    await open(b, "comments");
    // A saves first.
    await a.getByRole("textbox", { name: "Comment threads per page" }).fill("37");
    await saveButton(a).click();
    await expect(toast(a, "Settings saved")).toBeVisible();

    // B started from the old copy: its save asks, and nothing is written yet.
    const patches: unknown[] = [];
    b.on("request", (r) => {
      if (isPatch(r)) patches.push(r.postDataJSON());
    });
    await b.getByRole("textbox", { name: "Comment threads per page" }).fill("38");
    await saveButton(b).click();
    const dialog = b.getByRole("alertdialog", { name: "Changed by someone else" });
    await expect(dialog).toContainText("The Comments settings was changed since you opened it");
    await expect(dialog.getByRole("button", { name: "Reload" })).toBeVisible();
    expect(patches).toEqual([]);
    await dialog.getByRole("button", { name: "Overwrite" }).click();
    await expect(toast(b, "Settings saved")).toBeVisible();
    expect(patches).toEqual([{ "comments.per_page": 38 }]);
    expect((await api.getSettings())["comments.per_page"]).toBe(38);

    // Reload takes theirs and drops the edit.
    await a.getByRole("textbox", { name: "Comment threads per page" }).fill("39");
    await saveButton(a).click();
    await a
      .getByRole("alertdialog", { name: "Changed by someone else" })
      .getByRole("button", { name: "Reload" })
      .click();
    await expect(a.getByRole("textbox", { name: "Comment threads per page" })).toHaveValue("38");
    await expect(saveButton(a)).toBeDisabled();
  } finally {
    await api.patchSettings({ "comments.per_page": before["comments.per_page"] ?? null });
    await api.dispose();
  }
});

test("mail_panel_without_mail", async ({ as, snap, expectAccessible }) => {
  const page = await as("admin");
  // The E2E site has mail; this page sees a server without it.
  await page.route("**/api/v1/system/status", async (route) => {
    const res = await route.fetch();
    const body = (await res.json()) as Record<string, unknown>;
    await route.fulfill({
      response: res,
      json: { ...body, mail: { configured: false, from: null } },
    });
  });
  await open(page, "mail");
  const panel = page.locator("[data-slot=mail-panel]");
  await expect(panel).toContainText("Mail is not set up");
  await expect(panel.locator("[data-slot=mail-setup] pre")).toContainText("[mail]");
  await expect(panel).toContainText("Then restart ferrite serve");
  await expect(page.getByRole("button", { name: "Send test e-mail" })).toHaveCount(0);
  await snap(page, "settings-mail-none");
  await expectAccessible(page);
});

test("settings_screens", async ({ as, snap, expectAccessible, site }) => {
  test.setTimeout(120_000);
  const page = await as("admin");
  // `/admin/settings` opens General; `g s` reaches it from anywhere.
  await page.goto("/admin/");
  await expect(
    page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Settings" }),
  ).toBeVisible();
  await page.keyboard.press("g");
  await page.keyboard.press("s");
  await expect(page).toHaveURL(/\/admin\/settings\/general$/);
  await page.goto("/admin/settings");
  await expect(page).toHaveURL(/\/admin\/settings\/general$/);
  await expect(group(page)).toHaveAttribute("data-group", "general");
  await expect(group(page).getByRole("heading", { name: "General" })).toBeVisible();
  await snap(page, "settings-general");
  await expectAccessible(page);

  // Leaving with unsaved changes asks; Stay keeps them.
  await page.getByRole("textbox", { name: "Tagline" }).fill(uid("Unsaved"));
  await page
    .getByRole("navigation", { name: "Settings groups" })
    .getByRole("link", { name: "Reading" })
    .click();
  const leave = page.getByRole("alertdialog", { name: "Leave without saving?" });
  await expect(leave).toBeVisible();
  await leave.getByRole("button", { name: "Stay" }).click();
  await expect(page).toHaveURL(/\/settings\/general$/);
  await leave.waitFor({ state: "hidden" });
  // Back to what is stored, then on to Reading.
  await page.reload();
  await page
    .getByRole("navigation", { name: "Settings groups" })
    .getByRole("link", { name: "Reading" })
    .click();
  await expect(group(page)).toHaveAttribute("data-group", "reading");
  await snap(page, "settings-reading");
  await expectAccessible(page);

  await open(page, "permalinks");
  await page.getByRole("radio", { name: /Custom/ }).check();
  await expect(page.locator("[data-slot=permalink-post-custom]")).toBeVisible();
  await snap(page, "settings-permalinks", { prepare: () => atRest(page) });
  await expectAccessible(page);
  await page.getByRole("textbox", { name: "Custom structure" }).fill("/blog/%year%/%slug%/");
  await saveButton(page).click();
  await expect(page.getByRole("alertdialog", { name: "Change the permalinks?" })).toBeVisible();
  await snap(page, "settings-permalinks-confirm", { fullPage: false });
  await expectAccessible(page);
  await page.getByRole("button", { name: "Cancel" }).click();

  await open(page, "comments");
  await snap(page, "settings-comments");
  await expectAccessible(page);

  await open(page, "media-sizes");
  const sizes = page.locator("[data-slot=media-sizes]");
  await expect(sizes).toContainText("Set by the theme blog");
  await expect(sizes.locator("tr[data-size=card]")).toContainText("640 × 360");
  await snap(page, "settings-media-sizes");
  await expectAccessible(page);

  await open(page, "mail");
  const panel = page.locator("[data-slot=mail-panel]");
  await expect(panel).toContainText("Mail is set up");
  await snap(page, "settings-mail");
  await expectAccessible(page);
  // The test mail goes to the signed-in user's own address (rate limited: once per run).
  const after = Date.now();
  const sent = page.waitForResponse(
    (r) => r.request().method() === "POST" && r.url().endsWith("/api/v1/system/mail/test"),
  );
  await page.getByRole("button", { name: "Send test e-mail" }).click();
  const { to } = (await (await sent).json()) as { to: string };
  await expect(toast(page, `Test e-mail queued for ${to}`)).toBeVisible();
  await mailbox(site.mailDir).waitFor(to, { after });

  // An unknown group is not found, like any unknown admin URL.
  await page.goto("/admin/settings/nope");
  await expect(page.getByRole("heading", { name: /not found/i })).toBeVisible();
});
