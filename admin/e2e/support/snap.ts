// Review screenshots (M2b P11): every screen in light and dark, at desktop and mobile
// width, under `test-results/screens/`. They are review artefacts for the contact sheet
// (`contact-sheet.ts`), not pixel-diff baselines.

import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { RESULTS_DIR } from "./server";

export const VIEWPORTS = {
  desktop: { width: 1440, height: 900 },
  mobile: { width: 390, height: 844 },
} as const;
export type Viewport = keyof typeof VIEWPORTS;
export const SCHEMES = ["light", "dark"] as const;
export type Scheme = (typeof SCHEMES)[number];

export const SCREENS_DIR = join(RESULTS_DIR, "screens");
const NAME = /^[a-z0-9][a-z0-9_-]*$/;

/** `<name>.<scheme>.<viewport>.png`, the names the contact sheet groups by. */
export function screenFile(name: string, scheme: Scheme, viewport: Viewport): string {
  return join(SCREENS_DIR, `${name}.${scheme}.${viewport}.png`);
}

/** The colour scheme the page currently sees. */
export async function currentScheme(page: Page): Promise<Scheme> {
  const dark = await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches);
  return dark ? "dark" : "light";
}

/** Waits for network idle, loaded fonts and the end of every finite animation. */
export async function settle(page: Page): Promise<void> {
  await page.waitForLoadState("networkidle");
  await page.evaluate(async () => {
    await document.fonts.ready;
    // A colour-scheme change reaches <html> through a React effect, and the colour
    // transitions it starts exist only after that; let two frames pass first.
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    const finite = document
      .getAnimations()
      .filter((a) => a.effect?.getComputedTiming().iterations !== Infinity);
    const timeout = new Promise((r) => setTimeout(r, 5_000));
    await Promise.race([
      Promise.all(finite.map((a) => a.finished.catch(() => undefined))),
      timeout,
    ]);
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  });
}

/** Four full-page screenshots of the page as it is now; restores viewport and scheme. */
export async function snap(page: Page, name: string): Promise<void> {
  if (!NAME.test(name)) throw new Error(`snap name must match ${String(NAME)}: ${name}`);
  mkdirSync(SCREENS_DIR, { recursive: true });
  const size = page.viewportSize();
  const scheme = await currentScheme(page);
  try {
    for (const s of SCHEMES) {
      await page.emulateMedia({ colorScheme: s });
      for (const v of Object.keys(VIEWPORTS) as Viewport[]) {
        await page.setViewportSize(VIEWPORTS[v]);
        await settle(page);
        await page.screenshot({
          path: screenFile(name, s, v),
          fullPage: true,
          animations: "disabled",
        });
      }
    }
  } finally {
    await page.emulateMedia({ colorScheme: scheme });
    if (size) await page.setViewportSize(size);
  }
}
