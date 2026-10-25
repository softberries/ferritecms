// The app shell under the real server (docs/design/11-admin-ui.md §4–§8): sidebar and
// its persisted state, the mobile nav sheet, shortcuts and the `?` sheet, theme and
// density, route transitions under reduced motion, and the not-found / forbidden /
// error states inside the shell. Every test uses a shared storage state it never ends.

import type { Page, Route } from "@playwright/test";
import { expect, test } from "./support/fixtures";
import { authFile } from "./support/server";
import { VIEWPORTS } from "./support/snap";

// The browser logs every non-2xx fetch as a console error; the state tests expect some.
const EXPECTED_FAILURES =
  /Failed to load resource: the server responded with a status of (403|500)/;
const PROBLEM = "https://ferritecms.dev/problems/";

const sidebar = (page: Page) => page.locator("[data-slot=sidebar][data-state]");
const nav = (page: Page) => page.getByRole("navigation", { name: "Main" });
// The top bar's toggle (the sidebar's rail is a second, mouse-only one).
const toggle = (page: Page) =>
  page.getByRole("banner").getByRole("button", { name: "Toggle sidebar" });

/**
 * `mod` as the page sees it (11 §6). Not Playwright's `ControlOrMeta`, which follows the
 * host OS while the page's platform follows the emulated device.
 */
async function mod(page: Page): Promise<"Meta" | "Control"> {
  const apple = await page.evaluate(() => {
    const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
    return /mac|iphone|ipad|ipod/i.test(nav.userAgentData?.platform ?? navigator.platform);
  });
  return apple ? "Meta" : "Control";
}

async function openDashboard(page: Page): Promise<void> {
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
}

/** Answers `GET /settings` (the dashboard placeholder's loader) with a problem. */
function failSettings(status: 403 | 500) {
  return (route: Route) =>
    route.fulfill({
      status,
      contentType: "application/problem+json",
      body: JSON.stringify(
        status === 403
          ? {
              type: `${PROBLEM}forbidden`,
              title: "Forbidden",
              status,
              detail: "needs settings:read",
            }
          : {
              type: `${PROBLEM}internal`,
              title: "Internal error",
              status,
              detail: "The server could not answer. It has been logged.",
            },
      ),
    });
}

test("sidebar_collapses_and_persists", async ({ as, context }) => {
  const page = await as("admin");
  await openDashboard(page);
  await expect(sidebar(page)).toHaveAttribute("data-state", "expanded");
  await expect(nav(page).getByRole("link", { name: "Dashboard" })).toHaveAttribute(
    "aria-current",
    "page",
  );

  await toggle(page).click();
  await expect(sidebar(page)).toHaveAttribute("data-state", "collapsed");
  await expect(sidebar(page)).toHaveAttribute("data-collapsible", "icon");
  expect(await page.evaluate(() => localStorage.getItem("ferrite.admin.sidebar"))).toBe(
    "collapsed",
  );
  // Per viewer in localStorage, never a cookie (11 §7).
  expect((await page.context().cookies()).map((c) => c.name)).not.toContain("sidebar_state");
  expect((await context.cookies()).map((c) => c.name)).not.toContain("sidebar_state");

  await page.reload();
  await expect(sidebar(page)).toHaveAttribute("data-state", "collapsed");

  // `mod+b` toggles it, except while typing (the editor's bold is `mod+b`).
  await page.keyboard.press(`${await mod(page)}+b`);
  await expect(sidebar(page)).toHaveAttribute("data-state", "expanded");
  await page.keyboard.press(`${await mod(page)}+k`);
  const input = page.getByRole("combobox");
  await expect(input).toBeFocused();
  await page.keyboard.press(`${await mod(page)}+b`);
  await expect(sidebar(page)).toHaveAttribute("data-state", "expanded");
  await page.keyboard.press("Escape");
});

test("mobile_nav_sheet", async ({ browser, backend, site, guard }) => {
  // A phone: touch targets stay at least 44 px (11 §8).
  const ctx = await browser.newContext({
    baseURL: site.url,
    storageState: authFile(backend, "author"),
    viewport: VIEWPORTS.mobile,
    hasTouch: true,
    isMobile: true,
  });
  await guard.watch(ctx);
  try {
    const page = await ctx.newPage();
    await openDashboard(page);
    // No desktop sidebar under 768 px; the trigger opens the sheet.
    await expect(nav(page)).toBeHidden();
    const trigger = toggle(page);
    expect(await trigger.evaluate((el) => el.getBoundingClientRect().height)).toBe(44);
    // Opened from the keyboard, so focus has a place to return to.
    await trigger.focus();
    await page.keyboard.press("Enter");
    const sheet = page.getByRole("dialog", { name: "Sidebar" });
    await expect(sheet).toBeVisible();
    const posts = sheet.getByRole("link", { name: "Posts" });
    await expect(posts).toBeVisible();
    expect(await posts.evaluate((el) => el.getBoundingClientRect().height)).toBe(44);

    // Focus stays inside the sheet.
    for (let i = 0; i < 12; i++) {
      await page.keyboard.press("Tab");
      expect(await sheet.evaluate((el) => el.contains(document.activeElement))).toBe(true);
    }
    await page.keyboard.press("Escape");
    await expect(sheet).toBeHidden();
    await expect(trigger).toBeFocused();

    // Following a link closes it.
    await trigger.tap();
    await sheet.getByRole("link", { name: "Posts" }).tap();
    await expect(sheet).toBeHidden();
    await expect(page).toHaveURL(`${site.url}/admin/content/post`);
  } finally {
    await ctx.close();
  }
});

test("g_chords_navigate", async ({ as, site }) => {
  const page = await as("author");
  await page.goto("/admin/no/such/screen");
  await expect(page.getByRole("heading", { name: "Page not found" })).toBeVisible();
  await page.keyboard.press("g");
  await page.keyboard.press("d");
  await expect(page).toHaveURL(`${site.url}/admin/`);
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();

  // Typed into a field, the same keys are just text.
  await page.goto("/admin/no/such/screen");
  await expect(page.getByRole("heading", { name: "Page not found" })).toBeVisible();
  await page.keyboard.press(`${await mod(page)}+k`);
  await page.getByRole("combobox").pressSequentially("g d");
  await expect(page.getByRole("combobox")).toHaveValue("g d");
  await expect(page).toHaveURL(`${site.url}/admin/no/such/screen`);
});

test("shortcut_sheet_lists_bindings", async ({ as, snap, expectAccessible }) => {
  const page = await as("admin");
  await openDashboard(page);
  await page.keyboard.press("?");
  const sheet = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(sheet).toBeVisible();
  const rows = sheet.locator("dt");
  await expect(rows).toHaveText([
    "Open the command palette",
    "Show keyboard shortcuts",
    "Collapse or expand the sidebar",
    "Go to Dashboard",
  ]);
  await expect(sheet.getByRole("heading", { name: "General" })).toBeVisible();
  await expect(sheet.getByRole("heading", { name: "Go to" })).toBeVisible();
  await snap(page, "shell-shortcuts");
  await expectAccessible(page);
  await page.keyboard.press("Escape");
  await expect(sheet).toBeHidden();

  // Also from the account menu.
  await page.getByRole("button", { name: "Account" }).click();
  await page.getByRole("menuitem", { name: /Keyboard shortcuts/ }).click();
  await expect(sheet).toBeVisible();
});

test("theme_toggle_and_system", async ({ as }) => {
  const page = await as("admin");
  await openDashboard(page);
  const html = page.locator("html");
  const choose = async (name: "Light" | "Dark" | "System") => {
    await page.getByRole("button", { name: "Account" }).click();
    await page.getByRole("menuitemradio", { name }).click();
    await expect(page.getByRole("menuitemradio", { name })).toHaveAttribute("aria-checked", "true");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toBeHidden();
  };

  await choose("Dark");
  await expect(html).toHaveClass("dark");
  await choose("Light");
  await expect(html).toHaveClass("light");
  expect(await page.evaluate(() => localStorage.getItem("ferrite.admin.theme"))).toBe("light");

  await choose("System");
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(html).toHaveClass("dark");
  await page.emulateMedia({ colorScheme: "light" });
  await expect(html).toHaveClass("light");

  // Density sits in the same menu.
  await page.getByRole("button", { name: "Account" }).click();
  await page.getByRole("menuitemradio", { name: "Compact" }).click();
  await expect(html).toHaveAttribute("data-density", "compact");
});

/** Every computed `transform` of the route container over ~300 ms after `act`. */
async function transformsDuring(page: Page, act: () => Promise<void>): Promise<string[]> {
  await page.evaluate(() => {
    const w = window as unknown as { __samples: string[]; __sampling: boolean };
    w.__samples = [];
    w.__sampling = true;
    const tick = () => {
      const el = document.querySelector("[data-slot=route-transition]");
      if (el) w.__samples.push(getComputedStyle(el).transform);
      if (w.__sampling) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  await act();
  await page.waitForTimeout(350);
  return page.evaluate(() => {
    const w = window as unknown as { __samples: string[]; __sampling: boolean };
    w.__sampling = false;
    return w.__samples;
  });
}

test("reduced_motion_disables_transforms", async ({ as }) => {
  const page = await as("admin");
  // Control: with motion allowed, the entering screen rises (a transform is seen).
  await openDashboard(page);
  const moving = await transformsDuring(page, () =>
    nav(page).getByRole("link", { name: "Posts" }).click(),
  );
  expect(moving.some((t) => t !== "none")).toBe(true);

  await page.emulateMedia({ reducedMotion: "reduce" });
  await openDashboard(page);
  const still = await transformsDuring(page, () =>
    nav(page).getByRole("link", { name: "Pages" }).click(),
  );
  await expect(page.getByRole("heading", { name: "Page not found" })).toBeVisible();
  expect(still.length).toBeGreaterThan(0);
  expect(still.filter((t) => t !== "none")).toEqual([]);
});

test("not_found_inside_shell", async ({ as, site }) => {
  const page = await as("editor");
  await page.goto("/admin/definitely/not/here");
  await expect(page.getByRole("heading", { name: "Page not found" })).toBeVisible();
  // The shell is around it: nav, top bar crumb, the address unchanged.
  await expect(nav(page)).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Breadcrumb" })).toContainText("Not found");
  await expect(page).toHaveURL(`${site.url}/admin/definitely/not/here`);
  await expect(page).toHaveTitle("Not found · FerriteCMS");
  await page.getByRole("link", { name: "Go to dashboard" }).click();
  await expect(page).toHaveURL(`${site.url}/admin/`);
  await expect(page).toHaveTitle("Dashboard · FerriteCMS");
});

test("unauthenticated_not_found_goes_to_login", async ({ browser, site, guard }) => {
  // Behind the guard: an anonymous visitor learns nothing about which screens exist.
  guard.allow(/status of 401/);
  const ctx = await browser.newContext({ baseURL: site.url });
  await guard.watch(ctx);
  try {
    const page = await ctx.newPage();
    await page.goto("/admin/definitely/not/here");
    await expect(page).toHaveURL(/\/admin\/login\?next=%2Fadmin%2Fdefinitely%2Fnot%2Fhere$/);
  } finally {
    await ctx.close();
  }
});

test("screens", async ({ as, guard, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  guard.allow(EXPECTED_FAILURES);
  const page = await as("admin");

  await openDashboard(page);
  await snap(page, "shell-dashboard");
  await expectAccessible(page);

  // The search-engines pill (the setting itself is site-wide, so it is answered here).
  await page.route("**/api/v1/settings", async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    await route.fulfill({ response, json: { ...body, "reading.discourage_indexing": true } });
  });
  await openDashboard(page);
  await expect(page.locator("[data-slot=discouraged-pill]")).toBeVisible();
  await snap(page, "shell-dashboard-discouraged");
  await expectAccessible(page);
  await page.unroute("**/api/v1/settings");

  // Compact density.
  await page.evaluate(() => localStorage.setItem("ferrite.admin.density", "compact"));
  await openDashboard(page);
  await expect(page.locator("html")).toHaveAttribute("data-density", "compact");
  await snap(page, "shell-dashboard-compact");
  await expectAccessible(page);
  await page.evaluate(() => localStorage.removeItem("ferrite.admin.density"));

  // Collapsed to the icon rail.
  await openDashboard(page);
  await toggle(page).click();
  await expect(sidebar(page)).toHaveAttribute("data-state", "collapsed");
  await snap(page, "shell-collapsed");
  await expectAccessible(page);
  await toggle(page).click();

  // Account menu and the New menu, open.
  await page.getByRole("button", { name: "Account" }).click();
  await expect(page.getByRole("menu")).toBeVisible();
  await snap(page, "shell-user-menu");
  await expectAccessible(page);
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "More to create" }).click();
  await expect(page.getByRole("menuitem", { name: "New page" })).toBeVisible();
  await snap(page, "shell-new-menu");
  await expectAccessible(page);
  await page.keyboard.press("Escape");

  // The ⌘K palette (minimal until Task 11).
  await page.keyboard.press(`${await mod(page)}+k`);
  await expect(page.getByRole("dialog", { name: "Jump to" })).toBeVisible();
  await snap(page, "shell-palette");
  await expectAccessible(page);
  await page.keyboard.press("Escape");

  // Not found, forbidden and a failed load, all inside the shell.
  await page.goto("/admin/definitely/not/here");
  await expect(page.getByRole("heading", { name: "Page not found" })).toBeVisible();
  await snap(page, "shell-not-found");
  await expectAccessible(page);

  await page.route("**/api/v1/settings", failSettings(403));
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { name: "You don't have access to this" })).toBeVisible();
  await expect(nav(page)).toBeVisible();
  await snap(page, "shell-forbidden");
  await expectAccessible(page);
  await page.unroute("**/api/v1/settings");

  await page.route("**/api/v1/settings", failSettings(500));
  await page.goto("/admin/");
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("The server could not answer.");
  await expect(nav(page)).toBeVisible();
  await snap(page, "shell-error");
  await expectAccessible(page);
  await page.unroute("**/api/v1/settings");
  // Retry recovers once the server answers.
  await alert.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();

  // The mobile nav sheet, open (the desktop shots of this state show the plain shell).
  await page.setViewportSize(VIEWPORTS.mobile);
  await openDashboard(page);
  await toggle(page).click();
  await expect(page.getByRole("dialog", { name: "Sidebar" })).toBeVisible();
  await expectAccessible(page);
  await snap(page, "shell-mobile-nav");
});
