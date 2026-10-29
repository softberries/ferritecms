// The Import screen under the real server (M3-2 P32, P34 check 1): export A of the test
// corpus with a zip of its uploads folder goes up through the start card, the run panel
// follows the `import.wordpress` job to `done`, the report opens, and an imported post
// answers at its old WordPress URL (the import keeps the old permalink structure).
//
// Site-wide (m2b R62; `SITE_WIDE_SPECS`): the import sets the site title, tagline and the
// post permalink structure and runs a job. The `finally` puts the three settings back and
// removes the `slug-change` redirects that putting the structure back makes (as the
// Settings spec does), so the specs after it see the site they expect.
//
// It survives a CI retry against the same site (AD2): the settings to put back are the
// ones the first attempt saw (kept in a file per site), an import job left queued or
// running by an earlier attempt is waited for before starting, and a second import of the
// same export counts its media as unchanged and leaves the permalink structure alone (the
// old URL then redirects to the post). `--repeat-each 2` runs it twice on one site.

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { ContentApi, type Redirect } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { E2E_STATE_DIR } from "./support/server";
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

/**
 * The settings the import changes, as the first attempt on this site found them: saved
 * once per site (its URL names the run's server), read back by a retry.
 */
async function baseline(api: ContentApi, key: string, data: string): Promise<Settings> {
  // Keyed by the site's own data folder (a fresh temp folder per site), never its port,
  // which a later run may get again; global setup also empties the folder per run.
  const site = createHash("sha256").update(data).digest("hex").slice(0, 16);
  const file = join(E2E_STATE_DIR, `import-${key}-${site}.json`);
  if (existsSync(file)) return JSON.parse(readFileSync(file, "utf8")) as Settings;
  const now = await api.getSettings();
  const kept = Object.fromEntries(RESTORED.map((k) => [k, now[k] ?? null]));
  mkdirSync(E2E_STATE_DIR, { recursive: true });
  writeFileSync(file, JSON.stringify(kept));
  return kept;
}
type Settings = Record<string, unknown>;

/** Waits until no `import.wordpress` job is queued or running (an earlier attempt's). */
async function noImportJob(api: ContentApi): Promise<void> {
  const count = async (status: string) => {
    const res = await api.ctx.get(
      `/api/v1/system/jobs?kind=import.wordpress&status=${status}&limit=10`,
    );
    expect(res.ok(), await res.text()).toBe(true);
    return ((await res.json()) as { items: unknown[] }).items.length;
  };
  await expect
    .poll(async () => (await count("queued")) + (await count("running")), { timeout: 150_000 })
    .toBe(0);
}

/** Whether a run of export A finished before this attempt (a re-import). */
async function importedBefore(api: ContentApi): Promise<boolean> {
  const res = await api.ctx.get("/api/v1/import/runs?limit=50");
  expect(res.ok(), await res.text()).toBe(true);
  const runs = ((await res.json()) as { items: { source: string; state: string }[] }).items;
  return runs.some((r) => r.source.startsWith("http://localhost:8099") && r.state === "done");
}

async function open(page: Page): Promise<void> {
  await expect(screen(page).getByRole("heading", { name: "Import", level: 1 })).toBeVisible();
  await expect(screen(page).locator("[aria-busy=true]")).toHaveCount(0);
}

test("import_export_a_through_the_screen", async ({
  as,
  guard,
  site,
  siteKey,
  snap,
  expectAccessible,
}) => {
  test.setTimeout(180_000);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const before = await baseline(api, siteKey, site.data);
  let started = false;
  let thrown: { error: unknown } | null = null;
  const page = await as("admin");
  try {
    // A retry: an earlier attempt's import may still be queued or running.
    await noImportJob(api);
    const reimport = await importedBefore(api);
    // The sidebar's Import item, between Plugins and Settings, and `g i`.
    await page.goto("/admin/");
    const nav = page.getByRole("navigation", { name: "Main" });
    await expect(nav.getByRole("link", { name: "Import" })).toBeVisible();
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
    // Focus moved from the gone start card to the panel's heading.
    await expect(panel(page).locator("header h2")).toBeFocused();

    // The panel follows the job (a poll every 2 s) to the end of the run.
    await expect(panel(page).getByRole("heading", { name: "Import finished" })).toBeVisible({
      timeout: 150_000,
    });
    const steps = panel(page).getByRole("list", { name: "Steps" });
    await expect(steps.locator("[data-state=done]")).toHaveCount(7);
    const counts = panel(page).getByRole("table");
    await expect(counts.locator("tr[data-kind=post]")).toBeVisible();
    // The zip's files became media: created by a first import, unchanged by a re-import.
    const media = counts.locator("tr[data-kind=media] td");
    const cell = async (i: number) => Number((await media.nth(i).textContent()) ?? "0");
    expect((await cell(0)) + (await cell(2))).toBeGreaterThan(0);
    await snap(page, "import-done");
    await expectAccessible(page);

    // The report: a page in a new tab and a JSON download.
    const report = panel(page).getByRole("link", { name: /Open report/ });
    await expect(report).toHaveAttribute("target", "_blank");
    const href = (await report.getAttribute("href")) ?? "";
    expect(href).toMatch(/^\/api\/v1\/import\/runs\/[0-9a-f-]{36}\/report\.html$/);
    // The report is served under `sandbox` (no scripts): the browser refuses the guard's
    // own init script there and logs it.
    const escaped = new URL(href, site.url).href.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    guard.allow(
      new RegExp(
        `^Blocked script execution in '${escaped}' because the document's frame is sandboxed`,
      ),
    );
    // Firefox requests `/favicon.ico` for the page itself, which its `default-src 'none'`
    // refuses (and logs); that load is the browser's, not the report's.
    const favicon = new URL("/favicon.ico", site.url).href.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    guard.allow(new RegExp(`^img-src blocked ${favicon} at resource:`));
    guard.allow(
      new RegExp(
        `^\\[JavaScript Error: "Content-Security-Policy: The page’s settings blocked the loading of a resource \\(img-src\\) at ${favicon} `,
      ),
    );
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
    await expect(row).toContainText("http://localhost:8099");
    await expect(row.locator("[data-slot=run-state]").first()).toHaveText("Done");

    // The post at its old WordPress URL: served there by a first import (which takes the
    // old structure); a re-import keeps the site's structure and redirects the old URL.
    const old = await page.request.get(OLD_URL, { maxRedirects: reimport ? 1 : 0 });
    expect(old.status()).toBe(200);
    expect(await old.text()).toContain("Every core block");

    // "New import" brings the start card back.
    await panel(page).getByRole("button", { name: "New import" }).click();
    await expect(page).toHaveURL(/\/admin\/import$/);
    await expect(
      screen(page).locator("[data-slot=import-start]").getByRole("heading", { name: "New import" }),
    ).toBeFocused();
    await expect(screen(page).getByRole("button", { name: "Start import" })).toBeVisible();
  } catch (error) {
    thrown = { error };
  }
  // The cleanup: every step runs whatever the one before did; a cleanup failure is
  // reported only when the test itself passed, so it never hides the real error.
  {
    const problems: unknown[] = [];
    const step = async (run: () => Promise<unknown>) => {
      try {
        await run();
      } catch (error) {
        problems.push(error);
      }
    };
    // Only a structure the import changed moves posts back (a re-import keeps it).
    let moved = false;
    await step(async () => {
      const now = await api.getSettings();
      moved = (now["permalink.post"] ?? null) !== (before["permalink.post"] ?? null);
    });
    await step(() =>
      api.patchSettings(Object.fromEntries(RESTORED.map((k) => [k, before[k] ?? null]))),
    );
    if (started && moved) {
      // Putting the structure back moves every dated post to `/%slug%/` and redirects the
      // dated paths (P30). Once the job has made this post's, remove every `slug-change`
      // redirect from or to a dated path, as the Settings spec does.
      await step(() =>
        expect
          .poll(
            async () =>
              (await api.listRedirects("every-core-block")).some((r) => r.source === OLD_URL),
            { timeout: 15_000 },
          )
          .toBe(true),
      );
      const shape = /^\/\d{4}\/\d{2}\/[^/]+\/$/;
      const leaked = (r: Redirect) =>
        r.origin === "slug-change" && (shape.test(r.source) || shape.test(r.target));
      await step(async () => {
        for (const r of (await api.allRedirects()).filter(leaked)) await api.deleteRedirect(r.id);
      });
    }
    await step(() => api.dispose());
    if (thrown) throw thrown.error;
    if (problems.length > 0) throw problems[0];
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
