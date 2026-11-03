// The content health screen under the real server (M6-4, design 16 §11.2): a site with a
// broken internal link, an orphan page, a redirect chain and an image without alt text;
// "Run now" queues the job, the findings appear in their tabs, a row opens the editor at
// the finding, `g h` navigates, axe is clean in light and dark. The items and redirects
// are the test's own; the run covers the whole shared site, so rows are found by title.

import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

test("run_now_lists_findings_by_tab", async ({ as, site, siteKey, expectAccessible }) => {
  const admin = await ContentApi.as(siteKey, site.url, "admin");
  const tag = uid("h");
  const broken = await admin.createIn("post", `Broken ${tag}`, "published");
  const withLink = await admin.patch(broken, {
    body: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            {
              type: "text",
              text: "The missing guide",
              marks: [{ type: "link", attrs: { href: `/no-such-${tag}/` } }],
            },
          ],
        },
        { type: "image", attrs: { src: "/favicon.ico" } },
      ],
    },
  });
  const orphan = await admin.createIn("page", `Orphan ${tag}`, "published");
  const ctx = admin.ctx;
  const me = (await (await ctx.get("/api/v1/auth/me")).json()) as { csrf_token: string };
  for (const [source, target] of [
    [`/${tag}-a/`, `/${tag}-b/`],
    [`/${tag}-b/`, `/${tag}-c/`],
  ]) {
    const r = await ctx.post("/api/v1/redirects", {
      headers: { "X-CSRF-Token": me.csrf_token },
      data: { source_path: source, target, code: 301 },
    });
    expect(r.ok(), await r.text()).toBe(true);
  }

  const page = await as("admin");
  await page.goto("/admin/");
  await page.keyboard.press("g");
  await page.keyboard.press("h");
  await expect(page).toHaveURL(/\/admin\/health/);
  await expect(page.getByRole("heading", { name: "Content health", level: 1 })).toBeVisible();

  const before = (await (await ctx.get("/api/v1/health/summary")).json()) as {
    last: { id: string } | null;
  };
  const run = page.locator("[data-slot=health-run]");
  await run.click();
  // Wait for a newer done run than the one before.
  await expect
    .poll(
      async () => {
        const s = (await (await ctx.get("/api/v1/health/summary")).json()) as {
          last: { id: string } | null;
          running: unknown;
        };
        return s.last !== null && s.last.id !== before.last?.id && s.running === null;
      },
      { timeout: 60_000 },
    )
    .toBe(true);
  await page.reload();

  const table = page.locator("[data-slot=health-findings]");
  await page.getByRole("tab", { name: "Accessibility" }).click();
  await expect(
    table.locator("tr", { hasText: `Broken ${tag}` }).filter({ hasText: "Image without alt text" }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "Links" }).click();
  await expect(
    table.locator("tr", { hasText: `Broken ${tag}` }).filter({ hasText: "Broken link" }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "Site" }).click();
  await expect(table.locator("tr", { hasText: `Orphan ${tag}` })).toBeVisible();
  await expect(table.locator("tr", { hasText: `/${tag}-a/` })).toBeVisible();
  await expectAccessible(page);
  await page.emulateMedia({ colorScheme: "dark" });
  await expectAccessible(page);
  await page.emulateMedia({ colorScheme: "light" });

  // A row opens the editor at the finding.
  await page.getByRole("tab", { name: "Accessibility" }).click();
  await table
    .locator("tr", { hasText: `Broken ${tag}` })
    .filter({ hasText: "Image without alt text" })
    .getByRole("link", { name: `Broken ${tag}` })
    .click();
  await expect(page).toHaveURL(
    new RegExp(`/content/post/${withLink.id}\\?check=image-alt-missing`),
  );
  await expect(
    page.locator("[data-slot=quality-finding][data-rule=image-alt-missing][data-focused]"),
  ).toBeVisible();
  void orphan;
});
