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

  // After a reload, a stored choice wins over the system scheme.
  await page.evaluate(() => localStorage.setItem("ferrite.admin.theme", "light"));
  await page.reload();
  await expect(html).toHaveClass("light");
  const lightBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(lightBg).not.toBe(darkBg);
});

test("dark_os_first_paint_is_dark_before_js", async ({ browser, site, guard }) => {
  // The frame before the module script runs: no JS at all, so <html> has no class and
  // only tokens.css decides (11 §4). No storage state needed: index.html is public.
  const background = async (colorScheme: "light" | "dark") => {
    const ctx = await browser.newContext({
      baseURL: site.url,
      javaScriptEnabled: false,
      colorScheme,
    });
    try {
      const page = await ctx.newPage();
      await page.goto("/admin/");
      expect(await page.locator("html").getAttribute("class")).toBeNull();
      return await page.evaluate(() => {
        const s = getComputedStyle(document.body);
        return {
          bg: s.backgroundColor,
          scheme: getComputedStyle(document.documentElement).colorScheme,
        };
      });
    } finally {
      await ctx.close();
    }
  };
  const dark = await background("dark");
  const light = await background("light");
  expect(dark.scheme).toBe("dark");
  expect(light.scheme).toBe("light");
  expect(dark.bg).not.toBe(light.bg);

  // Same colour as the JS-applied `.dark` background (on the sign-in screen: this
  // context has no session, and `/admin/` would answer the guard's `me` with a 401).
  const ctx = await browser.newContext({ baseURL: site.url, colorScheme: "dark" });
  await guard.watch(ctx);
  try {
    const page = await ctx.newPage();
    await page.goto("/admin/login");
    await expect(page.locator("html")).toHaveClass("dark");
    expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(
      dark.bg,
    );
  } finally {
    await ctx.close();
  }
});

test("explicit_dark_against_light_os_has_no_light_frame", async ({ browser, site, as }) => {
  // P57: an explicit choice is a cookie, and `index.html` arrives with its class, so the
  // frame before any JS already has the chosen theme, even against the OS scheme.
  const page = await as("admin");
  await page.goto("/admin/");
  await page.getByRole("button", { name: "Account" }).click();
  await page.getByRole("menuitemradio", { name: "Dark" }).click();
  await expect(page.locator("html")).toHaveClass("dark");
  const cookieOf = async () =>
    (await page.context().cookies(`${site.url}/admin/`)).find(
      (c) => c.name === "ferrite_admin_theme",
    );
  const cookie = await cookieOf();
  expect(cookie).toMatchObject({ value: "dark", path: "/admin", sameSite: "Lax", httpOnly: false });
  // About a year.
  expect(cookie!.expires * 1000 - Date.now()).toBeGreaterThan(360 * 86_400_000);
  const darkBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  await page.getByRole("menuitemradio", { name: "Light" }).click();
  await expect(page.locator("html")).toHaveClass("light");
  expect((await cookieOf())?.value).toBe("light");
  const lightBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  // `system` removes it.
  await page.getByRole("menuitemradio", { name: "System" }).click();
  await expect.poll(cookieOf).toBeUndefined();
  await page.keyboard.press("Escape");

  // The first frame: no JS, the OS against the choice.
  const firstFrame = async (
    colorScheme: "light" | "dark",
    value: string,
  ): Promise<{ cls: string | null; bg: string }> => {
    const ctx = await browser.newContext({
      baseURL: site.url,
      javaScriptEnabled: false,
      colorScheme,
    });
    try {
      await ctx.addCookies([{ name: "ferrite_admin_theme", value, url: `${site.url}/admin/` }]);
      const p = await ctx.newPage();
      await p.goto("/admin/login");
      return {
        cls: await p.locator("html").getAttribute("class"),
        bg: await p.evaluate(() => getComputedStyle(document.body).backgroundColor),
      };
    } finally {
      await ctx.close();
    }
  };
  expect(await firstFrame("light", "dark")).toEqual({ cls: "dark", bg: darkBg });
  expect(await firstFrame("dark", "light")).toEqual({ cls: "light", bg: lightBg });
  // Any other value is ignored: the OS decides, and nothing of it reaches the page.
  const hostile = await firstFrame("light", "x%22%3E%3Cscript%3E");
  expect(hostile).toEqual({ cls: null, bg: lightBg });
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
