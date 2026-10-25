// E2E against a real `ferrite serve` (10 §1, §3). Global setup starts the sites
// (`e2e/support/global-setup.ts`); build first:
//   pnpm -C admin build && cargo build --release -p ferrite && pnpm -C admin e2e
// With FERRITE_TEST_PG_URL set, `chromium-pg` runs the `*.smoke.spec.ts` files against a
// Postgres site as well (M2b P22).

import { defineConfig, devices } from "@playwright/test";
import type { Options } from "./e2e/support/fixtures";
import { VIEWPORTS } from "./e2e/support/snap";

const ci = !!process.env.CI;
const chromium = { ...devices["Desktop Chrome"], viewport: VIEWPORTS.desktop };

export default defineConfig<Options>({
  testDir: "./e2e",
  // Playwright empties its output folder on every run; screenshots, the contact sheet
  // and server logs sit next to it in test-results/.
  outputDir: "./test-results/artifacts",
  globalSetup: "./e2e/support/global-setup.ts",
  workers: 2,
  retries: ci ? 1 : 0,
  forbidOnly: ci,
  reporter: ci
    ? [["list"], ["html", { open: "never", outputFolder: "./test-results/report" }]]
    : "list",
  use: {
    trace: "retain-on-failure",
    colorScheme: "light",
  },
  projects: [
    { name: "chromium-sqlite", use: { ...chromium, backend: "sqlite" } },
    ...(process.env.FERRITE_TEST_PG_URL
      ? [
          {
            name: "chromium-pg",
            testMatch: /\.smoke\.spec\.ts$/,
            use: { ...chromium, backend: "pg" as const },
          },
        ]
      : []),
  ],
});
