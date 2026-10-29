// The Import screen under the real server (M3-2 P32, P34 check 1): export A of the test
// corpus with a zip of its uploads folder goes up through the start card, the run panel
// follows the `import.wordpress` job to `done`, the report opens, and an imported post
// answers at its old WordPress URL (the import keeps the old permalink structure).
//
// Site-wide (m2b R62; `SITE_WIDE_SPECS`): the import sets the site title, tagline and the
// post permalink structure and runs a job. The `finally` puts the three settings back and
// removes the `slug-change` redirects that putting the structure back makes (as the
// Settings spec does), so the specs after it see the site they expect.

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { ContentApi, type Redirect } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { zipFolder } from "./support/zip";

const CORPUS = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../crates/ferrite-import-wp/tests/corpus",
);
/** A published post of export A and its WordPress URL (`/%year%/%monthnum%/%postname%/`). */
const OLD_URL = "/2024/03/every-core-block/";
const RESTORED = ["site.title", "site.description", "permalink.post"] as const;

const screen = (page: Page) => page.locator("[data-slot=import-screen]");
const panel = (page: Page) => page.locator("[data-slot=import-run]");

async function open(page: Page): Promise<void> {
  await expect(screen(page).getByRole("heading", { name: "Import", level: 1 })).toBeVisible();
  await expect(screen(page).locator("[aria-busy=true]")).toHaveCount(0);
}

test("import_export_a_through_the_screen", async ({
  as,
  site,
  siteKey,
  snap,
  expectAccessible,
}) => {
  test.setTimeout(180_000);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const before = await api.getSettings();
  let started = false;
  const page = await as("admin");
  try {
    // The sidebar's Import item, between Plugins and Settings, and `g i`.
    await page.goto("/admin/");
    const nav = page.getByRole("navigation", { name: "Main" });
    const names = await nav.getByRole("link").allTextContents();
    const at = names.findIndex((n) => n.includes("Import"));
    expect(at).toBeGreaterThan(names.findIndex((n) => n.includes("Plugins")));
    expect(at).toBeLessThan(names.findIndex((n) => n.includes("Settings")));
    await page.keyboard.press("g");
    await page.keyboard.press("i");
    await expect(page).toHaveURL(/\/admin\/import$/);
    await open(page);
    const start = screen(page).getByRole("button", { name: "Start import" });
    await expect(start).toBeDisabled();
    await snap(page, "import-start");
    await expectAccessible(page);

    await screen(page)
      .locator("[data-slot=import-wxr] input[type=file]")
      .setInputFiles(join(CORPUS, "site-a.xml"));
    await screen(page)
      .locator("[data-slot=import-uploads] input[type=file]")
      .setInputFiles({
        name: "uploads.zip",
        mimeType: "application/zip",
        buffer: zipFolder(join(CORPUS, "site-a-uploads"), "uploads"),
      });
    await expect(screen(page).locator("[data-slot=import-file]")).toHaveCount(2);
    await screen(page).getByRole("button", { name: "Advanced" }).click();
    await expect(screen(page).getByRole("textbox", { name: "Category base" })).toHaveValue(
      "category",
    );
    await snap(page, "import-ready");
    await expectAccessible(page);

    const post = page.waitForRequest(
      (r) => r.method() === "POST" && new URL(r.url()).pathname === "/api/v1/import/wordpress",
    );
    await start.click();
    const request = await post;
    started = true;
    expect(request.headers()["idempotency-key"]).toMatch(/^[0-9a-f-]{36}$/);
    await expect(page).toHaveURL(/\/admin\/import\?job=[0-9a-f-]{36}$/);

    // The panel follows the job (a poll every 2 s) to the end of the run.
    await expect(panel(page).getByRole("heading", { name: "Import finished" })).toBeVisible({
      timeout: 150_000,
    });
    const steps = panel(page).getByRole("list", { name: "Steps" });
    await expect(steps.locator("[data-state=done]")).toHaveCount(7);
    const counts = panel(page).getByRole("table");
    await expect(counts.locator("tr[data-kind=post]")).toBeVisible();
    // The zip's files became media.
    await expect(counts.locator("tr[data-kind=media] td").first()).not.toHaveText("0");
    await snap(page, "import-done");
    await expectAccessible(page);

    // The report: a page in a new tab and a JSON download.
    const report = panel(page).getByRole("link", { name: /Open report/ });
    await expect(report).toHaveAttribute("target", "_blank");
    const href = (await report.getAttribute("href")) ?? "";
    expect(href).toMatch(/^\/api\/v1\/import\/runs\/[0-9a-f-]{36}\/report\.html$/);
    const [tab] = await Promise.all([page.context().waitForEvent("page"), report.click()]);
    await tab.waitForLoadState();
    expect(new URL(tab.url()).pathname).toBe(href);
    await expect(tab.locator("body")).toContainText("Counts");
    await tab.close();
    const json = await page.request.get(href.replace(/\.html$/, ".json"));
    expect(json.ok(), await json.text()).toBe(true);
    expect(((await json.json()) as { counts: Record<string, unknown> }).counts).toHaveProperty(
      "post",
    );

    // The runs table lists the run as done, with its report.
    const row = screen(page).locator("[data-slot=import-runs] tr[data-row-id]").first();
    await expect(row).toContainText("http://localhost:8099/");
    await expect(row.locator("[data-slot=run-state]").first()).toHaveText("Done");

    // The post at its old WordPress URL.
    const old = await page.request.get(OLD_URL, { maxRedirects: 0 });
    expect(old.status()).toBe(200);
    expect(await old.text()).toContain("Every core block");

    // "New import" brings the start card back.
    await panel(page).getByRole("button", { name: "New import" }).click();
    await expect(page).toHaveURL(/\/admin\/import$/);
    await expect(screen(page).getByRole("button", { name: "Start import" })).toBeVisible();
  } finally {
    await api.patchSettings(Object.fromEntries(RESTORED.map((k) => [k, before[k] ?? null])));
    if (started) {
      // Putting the structure back moves every dated post to `/%slug%/` and redirects the
      // dated paths (P30). Once the job has made this post's, remove every `slug-change`
      // redirect from or to a dated path, as the Settings spec does.
      await expect
        .poll(
          async () =>
            (await api.listRedirects("every-core-block")).some((r) => r.source === OLD_URL),
          { timeout: 15_000 },
        )
        .toBe(true);
      const shape = /^\/\d{4}\/\d{2}\/[^/]+\/$/;
      const leaked = (r: Redirect) =>
        r.origin === "slug-change" && (shape.test(r.source) || shape.test(r.target));
      for (const r of (await api.allRedirects()).filter(leaked)) await api.deleteRedirect(r.id);
    }
    await api.dispose();
  }
});

test("import_needs_the_import_capability", async ({ as, expectAccessible }) => {
  const page = await as("editor");
  await page.goto("/admin/");
  await expect(
    page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Posts" }),
  ).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Import" }),
  ).toHaveCount(0);
  await page.goto("/admin/import");
  await expect(page.getByText("You don't have access to this")).toBeVisible();
  await expectAccessible(page);
});
