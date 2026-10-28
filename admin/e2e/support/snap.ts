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

/**
 * Waits for network idle, loaded fonts and the end of every finite animation, including
 * one that starts when another ends (a crossfade that exits, then enters), for at most 5 s.
 */
export async function settle(page: Page): Promise<void> {
  await page.waitForLoadState("networkidle");
  await page.evaluate(async () => {
    await document.fonts.ready;
    const frames = () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    // A colour-scheme change reaches <html> through a React effect, and the colour
    // transitions it starts exist only after that; let two frames pass first.
    await frames();
    const deadline = Date.now() + 5_000;
    for (;;) {
      const running = document
        .getAnimations()
        .filter(
          (a) => a.playState === "running" && a.effect?.getComputedTiming().iterations !== Infinity,
        );
      if (running.length === 0 || Date.now() >= deadline) break;
      const left = new Promise((r) => setTimeout(r, Math.max(0, deadline - Date.now())));
      await Promise.race([
        Promise.all(running.map((a) => a.finished.catch(() => undefined))),
        left,
      ]);
      await frames();
    }
  });
}

/**
 * Fails when the page is wider than the viewport (11 §8: no horizontal scroll at any
 * width), naming the width found and the widest element sticking out.
 */
export async function expectNoHorizontalOverflow(page: Page, where: string): Promise<void> {
  const found = await page.evaluate(() => {
    const doc = document.documentElement;
    if (doc.scrollWidth <= window.innerWidth) return null;
    let widest = "";
    let right = 0;
    for (const el of document.querySelectorAll<HTMLElement>("body *")) {
      const r = el.getBoundingClientRect().right;
      if (r > right) {
        right = r;
        widest = `${el.tagName.toLowerCase()}${el.className ? `.${String(el.className).split(" ").slice(0, 3).join(".")}` : ""}`;
      }
    }
    return { scrollWidth: doc.scrollWidth, innerWidth: window.innerWidth, widest, right };
  });
  if (found) {
    throw new Error(
      `horizontal overflow at ${where}: scrollWidth ${found.scrollWidth} > innerWidth ` +
        `${found.innerWidth} (widest: ${found.widest}, right edge ${Math.round(found.right)})`,
    );
  }
}

export interface SnapOptions {
  /**
   * Runs after each viewport and scheme change, before the capture: put the screen in
   * the state to show at that width (open a sheet, scroll a section into view).
   */
  prepare?: (viewport: Viewport) => Promise<void>;
  /** `false`: the viewport only (screens built on sticky, viewport-tall columns). */
  fullPage?: boolean;
}

/** Four full-page screenshots of the page as it is now; restores viewport and scheme. */
export async function snap(page: Page, name: string, options: SnapOptions = {}): Promise<void> {
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
        if (options.prepare) {
          await options.prepare(v);
          await settle(page);
        }
        await expectNoHorizontalOverflow(page, `${name} ${s} ${v}`);
        await page.screenshot({
          path: screenFile(name, s, v),
          fullPage: options.fullPage ?? true,
          animations: "disabled",
        });
      }
    }
  } finally {
    await page.emulateMedia({ colorScheme: scheme });
    if (size) await page.setViewportSize(size);
  }
}
