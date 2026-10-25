// The design language in the real browser (docs/design/11-admin-ui.md §2–§4): the
// self-hosted fonts load under the CSP, the tokens reach the page, and the theme and
// density preferences apply before the first render.

import type { Page } from "@playwright/test";
import { expect, test } from "./support/fixtures";

const cssVar = (page: Page, name: string) =>
  page.evaluate((n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim(), name);

test("fonts_and_tokens_apply", async ({ as }) => {
  const page = await as("admin");
  const fonts: string[] = [];
  page.on("response", (r) => {
    if (r.url().endsWith(".woff2")) fonts.push(`${r.status()} ${new URL(r.url()).pathname}`);
  });
  await page.goto("/admin/");
  const heading = page.getByRole("heading", { level: 1 });
  await expect(heading).toBeVisible();

  const loaded = await page.evaluate(async () => {
    await document.fonts.ready;
    const mono = await document.fonts.load('14px "Geist Mono Variable"');
    return {
      inter: document.fonts.check('600 28px "Inter Variable"'),
      mono: mono.length > 0,
      family: getComputedStyle(document.querySelector("h1") as Element).fontFamily,
    };
  });
  expect(loaded.inter).toBe(true);
  expect(loaded.mono).toBe(true);
  expect(loaded.family).toMatch(/^"Inter Variable"/);
  // Hashed, same-origin files only (font-src 'self'; principle 1: no CDN).
  expect(fonts.length).toBeGreaterThan(0);
  for (const f of fonts) expect(f).toMatch(/^200 \/admin\/assets\/[\w-]+\.woff2$/);

  // The unlayered token wins over Tailwind's same-name theme variable (the minifier
  // drops leading zeros).
  expect(await cssVar(page, "--ease-out")).toBe("cubic-bezier(.22, 1, .36, 1)");
  expect(await cssVar(page, "--shadow-1")).toContain("oklch(");
  // Body text is the 14/20 UI default.
  const body = await page.evaluate(() => {
    const s = getComputedStyle(document.body);
    return `${s.fontSize}/${s.lineHeight}`;
  });
  expect(body).toBe("14px/20px");
});

test("theme_follows_the_system_unless_chosen", async ({ as }) => {
  const page = await as("admin");
  await page.goto("/admin/");
  const html = page.locator("html");
  await expect(html).not.toHaveClass(/\bdark\b/);
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(html).toHaveClass(/\bdark\b/);
  const darkBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);

  // A stored choice wins over the system scheme from the first frame.
  await page.evaluate(() => localStorage.setItem("ferrite.admin.theme", "light"));
  await page.reload();
  await expect(html).not.toHaveClass(/\bdark\b/);
  const lightBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(lightBg).not.toBe(darkBg);
});

test("compact_density_switches_the_variables", async ({ as }) => {
  const page = await as("admin");
  await page.goto("/admin/");
  await expect(page.locator("html")).toHaveAttribute("data-density", "comfortable");
  expect(await cssVar(page, "--row-h")).toBe("44px");
  await page.evaluate(() => localStorage.setItem("ferrite.admin.density", "compact"));
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-density", "compact");
  expect(await cssVar(page, "--control-h")).toBe("30px");
  expect(await cssVar(page, "--row-h")).toBe("34px");
  expect(await cssVar(page, "--page-pad")).toBe("16px");
});

test("reduced_motion_zeroes_the_duration_tokens", async ({ as }) => {
  const page = await as("admin");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/admin/");
  for (const name of ["--dur-instant", "--dur-fast", "--dur-base", "--dur-slow"]) {
    // The minifier writes `0ms` as `0s`.
    expect(await cssVar(page, name), name).toMatch(/^0m?s$/);
  }
});

test("root_screens", async ({ as, snap, expectAccessible }) => {
  const page = await as("admin");
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await snap(page, "root");
  await expectAccessible(page);
});
