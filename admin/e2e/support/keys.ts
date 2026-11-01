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

/**
 * Presses a sequence (`g l`) once the page has loaded what it fetches and drawn it. WebKit
 * stamps a key event when it handles it, not when it was pressed, so on a page still
 * rendering its data the second key can be handled after the 1 s sequence timeout
 * (11 §6) although it was pressed at once. Not for a page that must stay busy (an upload
 * in flight): that one never goes idle.
 */
export async function chord(page: Page, ...keys: string[]): Promise<void> {
  await page.waitForLoadState("networkidle");
  // Two frames: what the loaded data renders has run once this resolves.
  await page.evaluate(
    () =>
      new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))),
  );
  for (const key of keys) await page.keyboard.press(key);
}
