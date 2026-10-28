// Redirects under the real server (M2b-2 Task 13; 07 §3 Redirects): create, edit and
// delete with Undo; replacing an existing source; the slash-toggled and reserved-source
// refusals inline; search; CSV import and export; import errors as text (Review Focus 3);
// a redirect answering on the public site; an editor without the screen; and the review
// screens. Every redirect holds the test's own id, the screen is narrowed to it with
// `?q=` (the other worker writes redirects too), and the test deletes its rows afterwards.

import type { Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const screen = (page: Page) => page.locator("[data-slot=redirects-screen]");
const header = (page: Page) => page.locator("[data-slot=page-header]");
const table = (page: Page) => page.getByRole("table", { name: "Redirects" });
const rows = (page: Page) => table(page).locator("tr[data-row]");
const form = (page: Page) => page.locator("[data-slot=redirect-form]");
const importDialog = (page: Page) => page.locator("[data-slot=import-dialog]");

/** A short unique id that fits a path. It starts with a letter: the router JSON-decodes
 * search values, so a hand-written `?q=531038e6` would arrive as a number. */
const short = () =>
  `r${uid("")
    .replace(/[^a-z0-9]/g, "")
    .slice(-8)}`;

/** The screen narrowed to `q`. */
async function open(page: Page, q?: string): Promise<void> {
  await page.goto(`/admin/structure/redirects${q ? `?q=${encodeURIComponent(q)}` : ""}`);
  await expect(screen(page)).toBeVisible();
  await expect(table(page)).not.toHaveAttribute("aria-busy", "true");
}

/** The row whose source is `source`. */
const row = (page: Page, source: string) =>
  rows(page).filter({ has: page.locator("[data-slot=redirect-source]", { hasText: source }) });

/** Fills the open form and submits it with `submit`. */
async function fill(
  page: Page,
  values: { source?: string; target?: string; code?: string },
): Promise<void> {
  if (values.source !== undefined) {
    await form(page).getByRole("textbox", { name: "From" }).fill(values.source);
  }
  if (values.target !== undefined) {
    await form(page).getByRole("textbox", { name: "To" }).fill(values.target);
  }
  if (values.code !== undefined) {
    await form(page).getByRole("combobox", { name: "Response" }).click();
    await page.getByRole("option", { name: new RegExp(`^${values.code}`) }).click();
  }
}

/** Chooses `csv` as `name` in the open import dialog. */
async function chooseCsv(page: Page, csv: string, name = "redirects.csv"): Promise<void> {
  await importDialog(page)
    .locator("input[type=file]")
    .setInputFiles({ name, mimeType: "text/csv", buffer: Buffer.from(csv) });
}

test("create_edit_delete_with_undo", async ({ as, site, siteKey, guard }) => {
  // The slash-toggled source is refused with a 422 (shown on the field).
  guard.allowFailedResource(/\/api\/v1\/redirects$/);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const s = short();
  const page = await as("admin");
  try {
    await open(page, s);
    await expect(screen(page).getByText("No redirects match")).toBeVisible();

    // Create.
    await header(page).getByRole("button", { name: "New redirect" }).click();
    await expect(form(page)).toBeVisible();
    await fill(page, { source: `/old-${s}`, target: `/new-${s}` });
    await form(page).getByRole("button", { name: "Create redirect" }).click();
    await expect(form(page)).toHaveCount(0);
    await expect(page.getByText(`Redirect from /old-${s} created`)).toBeVisible();
    await expect(row(page, `/old-${s}`)).toContainText(`/new-${s}`);
    await expect(row(page, `/old-${s}`)).toContainText("301");
    await expect(row(page, `/old-${s}`)).toContainText("Manual");
    await expect(row(page, `/old-${s}`)).toContainText("Never");

    // The slash-toggled source is refused inline (R26).
    await header(page).getByRole("button", { name: "New redirect" }).click();
    await fill(page, { source: `/old-${s}/`, target: `/elsewhere-${s}` });
    await form(page).getByRole("button", { name: "Create redirect" }).click();
    await expect(form(page).getByRole("textbox", { name: "From" })).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    await expect(form(page)).toContainText("source taken by another redirect");

    // The same source again: the form says it replaces the stored one.
    await fill(page, { source: `/old-${s}` });
    await form(page).getByRole("button", { name: "Create redirect" }).click();
    await expect(form(page).locator("[data-slot=redirect-exists]")).toContainText(
      `A redirect from /old-${s} already exists`,
    );
    await form(page).getByRole("button", { name: "Replace redirect" }).click();
    await expect(form(page)).toHaveCount(0);
    await expect(row(page, `/old-${s}`)).toContainText(`/elsewhere-${s}`);
    await expect(rows(page)).toHaveCount(1);

    // Edit: target and code.
    await row(page, `/old-${s}`)
      .getByRole("button", { name: `Edit redirect from /old-${s}` })
      .click();
    await expect(form(page).getByRole("textbox", { name: "From" })).toHaveValue(`/old-${s}`);
    await fill(page, { target: `/newer-${s}`, code: "302" });
    await form(page).getByRole("button", { name: "Save" }).click();
    await expect(form(page)).toHaveCount(0);
    await expect(row(page, `/old-${s}`)).toContainText(`/newer-${s}`);
    await expect(row(page, `/old-${s}`)).toContainText("302");

    // Delete, then Undo: back with its code, hits from 0 (the toast says so).
    await row(page, `/old-${s}`)
      .getByRole("button", { name: `Delete redirect from /old-${s}` })
      .click();
    await expect(rows(page)).toHaveCount(0);
    const toast = page.locator("[data-sonner-toast]").filter({ hasText: "Redirect deleted" });
    await expect(toast).toContainText("hit count starts again at 0");
    await toast.getByRole("button", { name: "Undo" }).click();
    await expect(row(page, `/old-${s}`)).toContainText(`/newer-${s}`);
    await expect
      .poll(async () => (await api.listRedirects(s)).map((r) => [r.source, r.target, r.code]))
      .toEqual([[`/old-${s}`, `/newer-${s}`, 302]]);
  } finally {
    await page.context().close();
    await api.deleteRedirects(s);
    await api.dispose();
  }
});

test("edit_stale_guard", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const s = short();
  await api.createRedirect(`/stale-${s}`, `/a-${s}`);
  const page = await as("admin");
  try {
    await open(page, s);
    await row(page, `/stale-${s}`)
      .getByRole("button", { name: `Edit redirect from /stale-${s}` })
      .click();
    // Someone else changes it meanwhile.
    await api.createRedirect(`/stale-${s}`, `/theirs-${s}`, 308);
    await fill(page, { target: `/mine-${s}` });
    await form(page).getByRole("button", { name: "Save" }).click();
    const stale = page.locator("[data-slot=stale-dialog]");
    await expect(stale).toContainText("Changed by someone else");
    await stale.getByRole("button", { name: "Reload" }).click();
    await expect(form(page).getByRole("textbox", { name: "To" })).toHaveValue(`/theirs-${s}`);
    expect((await api.listRedirects(s))[0]?.target).toBe(`/theirs-${s}`);
  } finally {
    await page.context().close();
    await api.deleteRedirects(s);
    await api.dispose();
  }
});

test("search_narrows", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const s = short();
  await api.createRedirect(`/alpha-${s}`, "/somewhere");
  await api.createRedirect(`/beta-${s}`, `/target-alpha-${s}`);
  await api.createRedirect(`/gamma-${s}`, "/elsewhere");
  const page = await as("admin");
  try {
    await open(page, s);
    await expect(rows(page)).toHaveCount(3);
    // `/` focuses the search; it matches sources and targets.
    await page.locator("body").click({ position: { x: 1, y: 1 } });
    await page.keyboard.press("/");
    const search = page.getByRole("searchbox", { name: "Search redirects" });
    await expect(search).toBeFocused();
    await search.fill(`alpha-${s}`);
    await expect(page).toHaveURL(new RegExp(`[?&]q=alpha-${s}`));
    await expect(rows(page)).toHaveCount(2);
    await expect(row(page, `/gamma-${s}`)).toHaveCount(0);
    await search.fill(`gamma-${s}`);
    await expect(rows(page)).toHaveCount(1);
    await expect(row(page, `/gamma-${s}`)).toBeVisible();
    await search.fill(`nothing-${s}`);
    await expect(screen(page).getByText("No redirects match")).toBeVisible();
  } finally {
    await page.context().close();
    await api.deleteRedirects(s);
    await api.dispose();
  }
});

test("import_and_export_roundtrip", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const s = short();
  const csv = [
    "source,target,code",
    `/one-${s},/uno-${s},301`,
    `/two-${s},"https://example.com/${s}?a=1,2",302`,
    `/three-${s},/tres-${s},410`,
  ].join("\r\n");
  const page = await as("admin");
  try {
    await open(page, s);
    await header(page).getByRole("button", { name: "Import" }).click();
    await chooseCsv(page, csv);
    const preview = importDialog(page).locator("[data-slot=import-preview]");
    await expect(preview).toContainText("3 rows");
    await expect(preview).toContainText("The header row is skipped");
    await expect(preview).toContainText(`https://example.com/${s}?a=1,2`);
    await importDialog(page).getByRole("button", { name: "Import 3 redirects" }).click();
    await expect(importDialog(page)).toHaveCount(0);
    await expect(page.getByText("Imported 3 redirects")).toBeVisible();
    await expect(rows(page)).toHaveCount(3);
    await expect(row(page, `/two-${s}`)).toContainText("Import");
    await expect(row(page, `/three-${s}`)).toContainText("410");

    // Export: a plain same-origin link with the session cookie.
    const link = header(page).getByRole("link", { name: "Export" });
    await expect(link).toHaveAttribute("download", "redirects.csv");
    const download = page.waitForEvent("download");
    await link.click();
    const file = await download;
    expect(file.suggestedFilename()).toBe("redirects.csv");
    const text = await readFile(await file.path(), "utf8");
    expect(text.startsWith("source,target,code\r\n")).toBe(true);
    expect(text).toContain(`/one-${s},/uno-${s},301\r\n`);
    expect(text).toContain(`/two-${s},"https://example.com/${s}?a=1,2",302\r\n`);
    expect(text).toContain(`/three-${s},/tres-${s},410\r\n`);
  } finally {
    await page.context().close();
    await api.deleteRedirects(s);
    await api.dispose();
  }
});

test("import_errors_are_text", async ({ as, site, siteKey, guard }) => {
  guard.allowFailedResource(/\/api\/v1\/redirects\/import$/);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const s = short();
  const hostile = `/<b>x</b>-${s}`;
  // Line 3 repeats line 2's source (its error quotes the HTML); line 4's source is
  // router-owned. Nothing is imported.
  const csv = [
    `/fine-${s},/t-${s},301`,
    `${hostile},/t-${s},301`,
    `${hostile},/u-${s},302`,
    `/api/${s},/t-${s},301`,
  ].join("\n");
  const page = await as("admin");
  const dialogs: string[] = [];
  page.on("dialog", (d) => {
    dialogs.push(d.message());
    void d.dismiss();
  });
  try {
    await open(page, s);
    await header(page).getByRole("button", { name: "Import" }).click();
    await chooseCsv(page, csv);
    await expect(importDialog(page).locator("[data-slot=import-preview]")).toContainText(hostile);
    await importDialog(page).getByRole("button", { name: "Import 4 redirects" }).click();
    const errors = importDialog(page).locator("[data-slot=import-errors]");
    await expect(errors).toContainText("Nothing was imported");
    const lines = errors.locator("li");
    await expect(lines).toHaveCount(2);
    await expect(lines.nth(0)).toContainText("Line 3");
    await expect(lines.nth(0)).toContainText(`duplicate source "${hostile}"`);
    await expect(lines.nth(1)).toContainText("Line 4");
    await expect(lines.nth(1)).toContainText("a path the site serves itself");
    // Nothing of it became markup.
    await expect(importDialog(page).locator("b, script, img")).toHaveCount(0);
    expect(dialogs).toEqual([]);
    expect(await api.listRedirects(s)).toEqual([]);
  } finally {
    await page.context().close();
    await api.deleteRedirects(s);
    await api.dispose();
  }
});

test("redirect_works_on_the_site", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const s = short();
  const page = await as("admin");
  try {
    await open(page, s);
    await header(page).getByRole("button", { name: "New redirect" }).click();
    await fill(page, { source: `/go-${s}`, target: `/landing-${s}/`, code: "307" });
    await form(page).getByRole("button", { name: "Create redirect" }).click();
    await expect(row(page, `/go-${s}`)).toBeVisible();
    for (const path of [`/go-${s}`, `/go-${s}/`]) {
      const res = await page.request.get(path, { maxRedirects: 0 });
      expect(res.status(), path).toBe(307);
      expect(res.headers().location, path).toBe(`/landing-${s}/`);
    }
  } finally {
    await page.context().close();
    await api.deleteRedirects(s);
    await api.dispose();
  }
});

test("editor_has_no_redirects", async ({ as }) => {
  const page = await as("editor");
  await page.goto("/admin/");
  const nav = page.getByRole("navigation", { name: "Main" });
  await expect(nav.getByRole("link", { name: "Menus" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Redirects" })).toHaveCount(0);
  await page.goto("/admin/structure/redirects");
  await expect(page.getByText("You don't have access to this")).toBeVisible();
  await expect(screen(page)).toHaveCount(0);
});

test("screens", async ({ as, site, siteKey, guard, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  // The form's and the import's refusals are 422s, shown in the dialogs.
  guard.allowFailedResource(/\/api\/v1\/redirects(\/import)?$/);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const s = short();
  await api.createRedirect(`/old-blog/${s}/`, `/blog/${s}/`);
  await api.createRedirect(
    `/spring-sale-${s}`,
    `https://shop.example.com/collections/spring-${s}`,
    302,
  );
  await api.createRedirect(
    `/archive/2019/a-rather-long-old-address-that-keeps-going-${s}/`,
    `/writing/a-rather-long-new-address-that-also-keeps-going-${s}/`,
    308,
  );
  await api.importRedirects(
    `/discontinued-${s},/products-${s}/,410\n/team-${s},/about/team-${s}/,301\n`,
  );
  // A slug change records its own redirect.
  const post = await api.createIn("post", `Moved ${s}`, "published");
  await api.patch(post, { slug: `moved-for-good-${s}` });
  // Hits reach the table within the flush interval (10 s).
  await api.ctx.get(`/old-blog/${s}/`, { maxRedirects: 0 });
  await api.ctx.get(`/spring-sale-${s}`, { maxRedirects: 0 });
  await expect
    .poll(async () => (await api.listRedirects(s)).filter((r) => r.hits > 0).length, {
      timeout: 25_000,
    })
    .toBe(2);
  const page = await as("admin");
  try {
    await open(page, s);
    await expect(rows(page)).toHaveCount(6);
    await snap(page, "redirects-table");
    await expectAccessible(page);

    // Nothing stored at all (the list answered empty).
    await page.route(/\/api\/v1\/redirects(\?|$)/, (route) =>
      route.fulfill({ json: { items: [], next_cursor: null } }),
    );
    await open(page);
    await expect(screen(page).getByText("No redirects yet")).toBeVisible();
    await snap(page, "redirects-empty");
    await expectAccessible(page);
    await page.unroute(/\/api\/v1\/redirects(\?|$)/);

    // The form with the server's refusal inline.
    await open(page, s);
    await header(page).getByRole("button", { name: "New redirect" }).click();
    await fill(page, { source: `/api/${s}`, target: `/blog/${s}/` });
    await form(page).getByRole("button", { name: "Create redirect" }).click();
    await expect(form(page)).toContainText("a path the site serves itself");
    await snap(page, "redirects-form-errors", { fullPage: false });
    await expectAccessible(page);
    await page.keyboard.press("Escape");
    await expect(form(page)).toHaveCount(0);

    // The import preview, then the server's errors.
    await header(page).getByRole("button", { name: "Import" }).click();
    await chooseCsv(
      page,
      [
        "source,target,code",
        `/winter-${s},/collections/winter-${s},301`,
        `/feed/${s},/news-${s},301`,
        `/loop-${s},/loop-${s},302`,
        `/gone-${s},/x,999`,
        `/ok-${s},https://example.com/${s},308`,
        `/more-${s},/more-new-${s},301`,
      ].join("\r\n"),
      "winter-campaign.csv",
    );
    await expect(importDialog(page).locator("[data-slot=import-preview]")).toContainText("6 rows");
    await snap(page, "redirects-import-preview", { fullPage: false });
    await expectAccessible(page);
    await importDialog(page).getByRole("button", { name: "Import 6 redirects" }).click();
    await expect(importDialog(page).locator("[data-slot=import-errors] li")).toHaveCount(3);
    await snap(page, "redirects-import-errors", { fullPage: false });
    await expectAccessible(page);
  } finally {
    await page.context().close();
    await api.purge(post);
    await api.deleteRedirects(s);
    await api.dispose();
  }
});
