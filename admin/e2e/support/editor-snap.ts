// Review screenshots of an editor section (M2b-2 Task 8): the editor's columns are sticky
// and viewport-tall, so it is captured by viewport, with the section in view. On a wide
// screen a panel section is scrolled to inside the panel (the window stays at the top) and
// the fields card by the window; on a phone the Settings sheet is opened and the section
// scrolled to inside it. Focus is dropped first, so no focused skip link or ring shifts it.

import { expect, type Page } from "@playwright/test";
import type { SnapOptions } from "./snap";

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
