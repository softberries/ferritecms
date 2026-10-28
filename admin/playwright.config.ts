// E2E against a real `ferrite serve` (10 §1, §3). Global setup starts one site per
// selected project (`e2e/support/global-setup.ts`); build first:
//   pnpm -C admin build && cargo build --release -p ferrite && pnpm -C admin e2e
// The matrix (M2b P55, `e2e/support/projects.ts`): `chromium-sqlite`, `chromium-pg` (with
// FERRITE_TEST_PG_URL), `firefox-sqlite` and `webkit-sqlite`, every spec in each. Run it
// through `pnpm e2e` (Chromium) or `pnpm e2e:all` (all four), whose wrapper
// (`scripts/e2e.mjs`) picks the projects and keeps single-file runs to that file.

import { defineConfig, devices } from "@playwright/test";
import type { Options } from "./e2e/support/fixtures";
import {
  definedProjects,
  selectedProjects,
  SITE_WIDE_SUFFIX,
  SITE_WIDE_SPECS,
  type Engine,
} from "./e2e/support/projects";
import { VIEWPORTS } from "./e2e/support/snap";

const ci = !!process.env.CI;
const DEVICES: Record<Engine, (typeof devices)[string]> = {
  chromium: devices["Desktop Chrome"],
  firefox: devices["Desktop Firefox"],
  webkit: devices["Desktop Safari"],
};

export default defineConfig<Options>({
  testDir: "./e2e",
  // Playwright empties its output folder on every run; screenshots, the contact sheet
  // and server logs sit next to it in test-results/.
  outputDir: "./test-results/artifacts",
  globalSetup: "./e2e/support/global-setup.ts",
  // Two per selected project: each has its own site, so `e2e:all` runs its four side by
  // side at the load one project puts on one server.
  workers: 2 * Math.max(1, selectedProjects().length),
  retries: ci ? 1 : 0,
  forbidOnly: ci,
  reporter: ci
    ? [["list"], ["html", { open: "never", outputFolder: "./test-results/report" }]]
    : "list",
  use: {
    trace: "retain-on-failure",
    colorScheme: "light",
  },
  projects: definedProjects().flatMap((p) => {
    // Every project at the desktop viewport; `snap` changes it for its phone shots.
    const use = {
      ...DEVICES[p.engine],
      viewport: VIEWPORTS.desktop,
      backend: p.backend,
      siteKey: p.name,
    };
    // A teardown runs after its project even when tests failed there (a dependency
    // would skip it).
    return [
      {
        name: p.name,
        testIgnore: SITE_WIDE_SPECS,
        teardown: `${p.name}${SITE_WIDE_SUFFIX}`,
        use,
      },
      { name: `${p.name}${SITE_WIDE_SUFFIX}`, testMatch: SITE_WIDE_SPECS, workers: 1, use },
    ];
  }),
});
