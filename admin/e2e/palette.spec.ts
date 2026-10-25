// The ⌘K command palette under the real server (plan Task 11; 11 §7): keyboard-only
// navigation, live content search against `seed demo`, creating, preferences, Recent,
// and the review screens (open, recent, searching, results, empty). The editor routes
// arrive with Tasks 14–15, so opening a hit or "New post" is checked by its URL.

import type { Page, Route } from "@playwright/test";
import { expect, test } from "./support/fixtures";
import { mod } from "./support/keys";

const palette = (page: Page) => page.getByRole("dialog", { name: "Command palette" });
const input = (page: Page) => palette(page).getByRole("combobox");
const option = (page: Page, name: string | RegExp) =>
  palette(page).getByRole("option", { name, exact: typeof name === "string" });
const nav = (page: Page) => page.getByRole("navigation", { name: "Main" });

/** The seeded post the search tests look for (`crates/ferrite/src/seed/demo.rs`). */
const SEEDED = "Why WebAssembly sandboxes";
/** Requests of the palette's search (`GET /content/{type}?q=…`). */
const SEARCH = /\/api\/v1\/content\/[^/?]+\?(?:.*&)?q=/;
/** `palette/sources.ts` `SEARCH_DEBOUNCE_MS` (the spec does not import app code). */
const SEARCH_DEBOUNCE_MS = 150;

async function openDashboard(page: Page): Promise<void> {
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
}

async function openPalette(page: Page): Promise<void> {
  await page.keyboard.press(`${await mod(page)}+k`);
  await expect(palette(page)).toBeVisible();
  await expect(input(page)).toBeFocused();
}

/** The id of the seeded post, from the same API the palette uses. */
async function seededId(page: Page): Promise<string> {
  const res = await page.request.get(`/api/v1/content/post?q=WebAssembly&limit=50`);
  expect(res.ok()).toBe(true);
  const body = (await res.json()) as { items: { id: string; title: string }[] };
  const item = body.items.find((i) => i.title === SEEDED);
  if (!item) throw new Error(`${SEEDED} is not in the seeded site`);
  return item.id;
}

test("open_with_mod_k_and_navigate", async ({ as, site }) => {
  const page = await as("author");
  await openDashboard(page);

  // Keyboard only: open, type, Enter.
  await openPalette(page);
  await page.keyboard.type("pages");
  await expect(option(page, "Pages")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(`${site.url}/admin/content/page`);
  await expect(palette(page)).toBeHidden();

  // `mod+k` toggles it, also from inside its own field.
  await openPalette(page);
  await page.keyboard.press(`${await mod(page)}+k`);
  await expect(palette(page)).toBeHidden();

  // Without a query the arrow keys walk the groups; Recent comes first and leaves out
  // the screen already open.
  await openPalette(page);
  const recent = palette(page).getByRole("group", { name: "Recent" });
  await expect(recent.getByRole("option")).toHaveText([/Dashboard/]);
  await expect(option(page, /Dashboard/).first()).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowDown");
  // The Go to group's first row (Dashboard again, as a nav item).
  await expect(
    palette(page).getByRole("group", { name: "Go to" }).getByRole("option").first(),
  ).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(`${site.url}/admin/`);
});

test("search_finds_seeded_post", async ({ as, site }) => {
  const page = await as("editor");
  await openDashboard(page);
  const id = await seededId(page);

  const searches: string[] = [];
  page.on("request", (r) => {
    if (SEARCH.test(r.url())) searches.push(r.url());
  });
  await openPalette(page);
  // One character searches nothing.
  await page.keyboard.type("W");
  // Well past the 150 ms debounce (`SEARCH_DEBOUNCE_MS`) plus a round trip: still nothing.
  await page.waitForTimeout(SEARCH_DEBOUNCE_MS + 250);
  expect(searches).toEqual([]);
  await page.keyboard.type("ebAssembly");

  const hit = option(page, new RegExp(SEEDED));
  await expect(hit).toBeVisible();
  await expect(hit).toContainText("/why-webassembly-sandboxes");
  await expect(hit).toContainText("Published");
  await expect(palette(page).getByRole("status")).toHaveText(/\d+ content results?/);
  // One round per type, each for the whole word (the keystrokes before it were debounced
  // away or aborted), with at most 8 hits.
  for (const url of searches) {
    expect(new URL(url).searchParams.get("limit")).toBe("8");
  }
  const lastRound = searches.filter((u) => new URL(u).searchParams.get("q") === "WebAssembly");
  expect(lastRound.map((u) => new URL(u).pathname).sort()).toEqual([
    "/api/v1/content/page",
    "/api/v1/content/post",
  ]);

  await hit.click();
  await expect(page).toHaveURL(`${site.url}/admin/content/post/${id}`);
  await expect(palette(page)).toBeHidden();

  // The opened item is now in Recent.
  await openDashboard(page);
  await openPalette(page);
  await expect(
    palette(page).getByRole("group", { name: "Recent" }).getByRole("option").first(),
  ).toContainText(SEEDED);
});

test("create_new_post_from_palette", async ({ as, site }) => {
  const page = await as("author");
  await openDashboard(page);
  await openPalette(page);
  await page.keyboard.type("new post");
  await expect(option(page, "New post")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(`${site.url}/admin/content/post/new`);
  // Reopened at once (while the closing animation runs), it starts empty.
  await openPalette(page);
  await expect(input(page)).toHaveValue("");
  await page.keyboard.type("new page");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(`${site.url}/admin/content/page/new`);
});

test("switch_theme_from_palette", async ({ as }) => {
  const page = await as("author");
  await openDashboard(page);
  const html = page.locator("html");
  await expect(html).toHaveClass("light");

  await openPalette(page);
  await page.keyboard.type("dark");
  await expect(option(page, /Dark theme/)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  await expect(palette(page)).toBeHidden();
  await expect(html).toHaveClass("dark");
  expect(await page.evaluate(() => localStorage.getItem("ferrite.admin.theme"))).toBe("dark");

  // The current choice is marked.
  await openPalette(page);
  await expect(option(page, /Dark theme/)).toContainText("Current");
  await expect(option(page, /Light theme/)).not.toContainText("Current");

  // Density sits in the same group.
  await page.keyboard.type("compact");
  await page.keyboard.press("Enter");
  await expect(html).toHaveAttribute("data-density", "compact");
});

test("screens", async ({ as, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  const page = await as("admin");
  await openDashboard(page);

  // Open, nothing typed (Recent holds only the open screen, so it is left out).
  await openPalette(page);
  await snap(page, "palette-open");
  await expectAccessible(page);
  await page.keyboard.press("Escape");

  // Recent, populated: two screens and an item.
  await nav(page).getByRole("link", { name: "Posts" }).click();
  await nav(page).getByRole("link", { name: "Pages" }).click();
  await openPalette(page);
  await page.keyboard.type("WebAssembly");
  await option(page, new RegExp(SEEDED)).click();
  await openDashboard(page);
  await openPalette(page);
  await expect(
    palette(page).getByRole("group", { name: "Recent" }).getByRole("option"),
  ).toHaveCount(3);
  await snap(page, "palette-recent");
  await expectAccessible(page);

  // Searching: the server holds its answer until the shots are taken.
  let release = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const hold = async (route: Route) => {
    await held;
    // A round the palette aborted meanwhile is already gone.
    await route.continue().catch(() => undefined);
  };
  await page.route(SEARCH, hold);
  await page.keyboard.type("sandbox");
  await expect(palette(page).locator("[data-slot=palette-searching]")).toBeVisible();
  await expect(palette(page).getByRole("status")).toHaveText("Searching content…");
  await snap(page, "palette-searching");
  await expectAccessible(page);
  release();
  await page.unroute(SEARCH, hold);

  // Results.
  await expect(option(page, new RegExp(SEEDED))).toBeVisible();
  await expect(palette(page).locator("[data-slot=palette-searching]")).toBeHidden();
  await snap(page, "palette-results");
  await expectAccessible(page);

  // A query nothing matches.
  await input(page).fill("qqzzxv");
  await expect(palette(page).getByText("No results for “qqzzxv”")).toBeVisible();
  await snap(page, "palette-empty");
  await expectAccessible(page);

  // Preferences, with the current choices checked.
  await input(page).fill("theme");
  await expect(option(page, /System theme/)).toContainText("Current");
  await snap(page, "palette-preferences");
  await expectAccessible(page);
});
