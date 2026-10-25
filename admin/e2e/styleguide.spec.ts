// The living styleguide (/admin/_styleguide, M2b P12): one review screen per section,
// axe in both schemes, the overlays opened, and density measured in the browser.

import { expect, test } from "./support/fixtures";
import { authFile } from "./support/server";
import { VIEWPORTS } from "./support/snap";

// Mirrors src/routes/_app/-styleguide/sections.ts (the nav is checked against it below).
const SECTIONS = [
  "colour",
  "type",
  "layout",
  "motion",
  "controls",
  "display",
  "navigation",
  "overlays",
  "patterns",
  "data",
  "charts",
] as const;

test("styleguide_screens", async ({ as, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  const page = await as("admin");

  await page.goto("/admin/_styleguide");
  const nav = page.getByRole("navigation", { name: "Styleguide sections" });
  // "All sections" plus one link per section, in order.
  await expect(nav.getByRole("link")).toHaveCount(SECTIONS.length + 1);
  for (const id of SECTIONS) {
    await expect(page.locator(`section[data-section=${id}]`)).toBeVisible();
  }

  for (const id of SECTIONS) {
    await page.goto(`/admin/_styleguide?section=${id}`);
    await expect(page.locator("section[data-section]")).toHaveCount(1);
    await expect(page.locator(`section[data-section=${id}]`)).toBeVisible();
    // No horizontal page scroll at phone width (11 §8); charts re-measure on a
    // ResizeObserver tick, so poll.
    await page.setViewportSize(VIEWPORTS.mobile);
    await expect
      .poll(
        () =>
          page.evaluate(
            () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
          ),
        { message: `${id}: horizontal overflow at 390 px` },
      )
      .toBe(0);
    await page.setViewportSize(VIEWPORTS.desktop);
    if (id === "data") {
      // Hover a row so its actions show.
      await page.getByRole("table", { name: "Posts" }).getByRole("row").nth(1).hover();
    }
    await snap(page, `styleguide-${id}`);
    await expectAccessible(page);
  }

  // Overlays, open.
  await page.goto("/admin/_styleguide?section=overlays");
  await page.getByRole("button", { name: "Actions" }).click();
  await expect(page.getByRole("menu")).toBeVisible();
  await snap(page, "styleguide-overlays-menu");
  await expectAccessible(page);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toBeHidden();

  await page.getByRole("button", { name: "Schedule…" }).click();
  const dialog = page.getByRole("dialog", { name: "Schedule post" });
  await expect(dialog).toBeVisible();
  await snap(page, "styleguide-overlays-dialog");
  await expectAccessible(page);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  // Focus returns to the trigger, which shows the standard ring.
  const trigger = page.getByRole("button", { name: "Schedule…" });
  await expect(trigger).toBeFocused();
  // (`transition-all` on buttons eases the ring in, so poll until it settles.)
  await expect
    .poll(() =>
      trigger.evaluate((el) => {
        const s = getComputedStyle(el);
        return `${s.outlineStyle} ${s.outlineWidth} ${s.outlineOffset}`;
      }),
    )
    .toBe("solid 2px 2px");
});

test("styleguide_compact_density", async ({ as }) => {
  const page = await as("admin");
  await page.goto("/admin/_styleguide?section=data");
  const table = page.getByRole("table", { name: "Posts" });
  const firstRow = table.getByRole("row").nth(1);
  const height = () => firstRow.evaluate((el) => el.getBoundingClientRect().height);
  await expect(page.locator("html")).toHaveAttribute("data-density", "comfortable");
  expect(await height()).toBe(44);

  await page.getByRole("radio", { name: "Compact" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-density", "compact");
  expect(await height()).toBe(34);
  // Controls follow --control-h (30 px compact).
  await page.goto("/admin/_styleguide?section=layout");
  const button = page.getByRole("button", { name: "Save", exact: true });
  expect(await button.evaluate((el) => el.getBoundingClientRect().height)).toBe(30);
});

test("styleguide_touch_targets", async ({ browser, backend, site, guard }) => {
  // On a touch screen controls and rows stay at least 44 px whatever the density (11 §4).
  const ctx = await browser.newContext({
    baseURL: site.url,
    storageState: authFile(backend, "admin"),
    viewport: { width: 390, height: 844 },
    hasTouch: true,
    isMobile: true,
  });
  await guard.watch(ctx);
  try {
    const page = await ctx.newPage();
    await page.goto("/admin/_styleguide?section=layout");
    expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
    await page.getByRole("radio", { name: "Compact" }).tap();
    await expect(page.locator("html")).toHaveAttribute("data-density", "compact");
    const button = page.getByRole("button", { name: "Save", exact: true });
    expect(await button.evaluate((el) => el.getBoundingClientRect().height)).toBe(44);
  } finally {
    await ctx.close();
  }
});

test("styleguide_table_frame_and_mixed_checkbox", async ({ as }) => {
  const page = await as("admin");
  await page.goto("/admin/_styleguide?section=data");
  // Every table scrolls inside its frame and never spills past its parent, at desktop
  // width too (review fix round 1); the header sticks to the frame.
  const frames = page.locator("[data-slot=data-table]");
  await expect(frames).toHaveCount(3);
  for (const frame of await frames.all()) {
    const box = await frame.evaluate((el) => {
      const s = getComputedStyle(el);
      const parent = el.parentElement?.getBoundingClientRect();
      return {
        overflowX: s.overflowX,
        spill: parent ? el.getBoundingClientRect().right - parent.right : 0,
        // The empty-state specimen has no header row (Task 13 fix round 1).
        sticky: (() => {
          const head = el.querySelector("thead");
          return head ? getComputedStyle(head).position : "sticky";
        })(),
      };
    });
    expect(box).toEqual({ overflowX: "auto", spill: 0, sticky: "sticky" });
  }

  // "Select all" with one row selected is mixed, and filled with primary in both schemes.
  const all = page.getByRole("table", { name: "Posts" }).getByRole("checkbox", {
    name: "Select all rows",
  });
  await expect(all).toHaveAttribute("data-state", "indeterminate");
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await expect(page.locator("html")).toHaveClass(scheme);
    await expect
      .poll(() =>
        all.evaluate((el) => {
          // A probe painted with the primary token, resolved like the checkbox fill.
          const probe = document.createElement("span");
          probe.style.backgroundColor = "var(--primary)";
          document.body.append(probe);
          const primary = getComputedStyle(probe).backgroundColor;
          probe.remove();
          return getComputedStyle(el).backgroundColor === primary;
        }),
      )
      .toBe(true);
  }
});
