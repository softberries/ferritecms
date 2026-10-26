// Review screenshots of an editor section (M2b-2 Task 8): the editor's columns are sticky
// and viewport-tall, so it is captured by viewport, with the section in view. On a wide
// screen a panel section is scrolled to inside the panel (the window stays at the top) and
// the fields card by the window; on a phone the Settings sheet is opened and the section
// scrolled to inside it. Focus is dropped first, so no focused skip link or ring shifts it.
// At every width the editor bar's content must fit inside its padding: the page-level
// overflow check alone misses a bar that overflows by less than its gutter, which wider
// font metrics (Linux CI) then push past the viewport.

import { expect, type Page } from "@playwright/test";
import type { SnapOptions } from "./snap";

/** Fails when the editor bar's children reach into (or past) its side padding. */
export async function expectEditorBarFits(page: Page): Promise<void> {
  const found = await page.locator("[data-slot=editor-bar]").evaluate((bar) => {
    const box = bar.getBoundingClientRect();
    const style = getComputedStyle(bar);
    const limit = box.right - parseFloat(style.paddingRight);
    let right = 0;
    for (const el of bar.querySelectorAll<HTMLElement>("*")) {
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.right > right) right = r.right;
    }
    return right > limit + 0.5 ? { right: Math.round(right), limit: Math.round(limit) } : null;
  });
  expect(found, "editor bar content fits inside its padding").toBeNull();
}

/** Snap options showing the section with `data-slot=<slot>` (`fields-card` on wide screens
 * for `card`, else its panel twin `panel-fields`). */
export function showSection(page: Page, slot: string, card = false): SnapOptions {
  return {
    fullPage: false,
    prepare: async (viewport) => {
      await page.evaluate(() => {
        const active = document.activeElement;
        if (active instanceof HTMLElement) active.blur();
        window.scrollTo(0, 0);
      });
      await expectEditorBarFits(page);
      const panel = page.locator("[data-slot=editor-panel]");
      if (viewport === "mobile") {
        if (!(await panel.isVisible())) {
          await page.locator("[data-slot=publish-bar] [data-slot=panel-toggle]").click();
          await expect(panel).toBeVisible();
        }
        await panel
          .locator(`[data-slot=${card ? "panel-fields" : slot}]`)
          .evaluate((el) => el.scrollIntoView({ block: "start" }));
        return;
      }
      if (card) {
        await page.locator("[data-slot=fields-card]").evaluate((el) => {
          window.scrollTo(0, el.getBoundingClientRect().top + window.scrollY - 112);
        });
        return;
      }
      await panel.locator(`[data-slot=${slot}]`).evaluate((el) => {
        const scroller = el.closest(".overflow-y-auto");
        if (!scroller) return;
        scroller.scrollTop +=
          el.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 16;
      });
    },
  };
}
