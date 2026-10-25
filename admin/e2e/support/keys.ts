// Keyboard helpers shared by the specs.

import type { Page } from "@playwright/test";

/**
 * `mod` as the page sees it (11 §6). Not Playwright's `ControlOrMeta`, which follows the
 * host OS while the page's platform follows the emulated device.
 */
export async function mod(page: Page): Promise<"Meta" | "Control"> {
  const apple = await page.evaluate(() => {
    const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
    return /mac|iphone|ipad|ipod/i.test(nav.userAgentData?.platform ?? navigator.platform);
  });
  return apple ? "Meta" : "Control";
}
