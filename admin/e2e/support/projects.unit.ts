// Unit tests (Vitest, the `scripts` project); not `*.test.ts`, which Playwright would
// collect as a spec.

import { expect, test } from "vitest";
import {
  PUBLIC_THEMES,
  PUBLIC_THEMES_PROJECT,
  PUBLIC_THEMES_SPEC,
  SITE_WIDE_SPECS,
  publicThemesSelected,
  selectedProjects,
  themeSiteKey,
} from "./projects";

const argv = (...args: string[]) => ["node", "playwright", "test", ...args];

test("public_themes_run_without_a_project_and_when_named", () => {
  expect(publicThemesSelected(argv())).toBe(true);
  expect(publicThemesSelected(argv("--project", PUBLIC_THEMES_PROJECT))).toBe(true);
  expect(publicThemesSelected(argv(`--project=${PUBLIC_THEMES_PROJECT}`))).toBe(true);
  expect(publicThemesSelected(argv("--project=public-*"))).toBe(true);
});

test("public_themes_stay_out_of_the_admin_projects", () => {
  expect(publicThemesSelected(argv("--project=chromium-sqlite"))).toBe(false);
  expect(publicThemesSelected(argv("--project", "chromium-*", "firefox-sqlite"))).toBe(false);
  // And the public-themes project starts none of the admin projects' sites.
  expect(selectedProjects(argv(`--project=${PUBLIC_THEMES_PROJECT}`))).toEqual([]);
});

test("each_theme_has_a_site_of_its_own", () => {
  expect(PUBLIC_THEMES.map((t) => [t.theme, t.content])).toEqual([
    ["blog", "demo"],
    ["business", "business"],
    ["docs", "docs"],
  ]);
  const keys = PUBLIC_THEMES.map((t) => themeSiteKey(t.theme));
  expect(keys).toEqual([
    "public-themes-chromium-blog",
    "public-themes-chromium-business",
    "public-themes-chromium-docs",
  ]);
});

test("the_spec_is_matched_by_its_project_only", () => {
  expect(PUBLIC_THEMES_SPEC.test("e2e/public-themes.spec.ts")).toBe(true);
  expect(PUBLIC_THEMES_SPEC.test("/x/admin/e2e/public-themes.spec.ts")).toBe(true);
  expect(PUBLIC_THEMES_SPEC.test("e2e/themes.spec.ts")).toBe(false);
  expect(SITE_WIDE_SPECS.test("e2e/public-themes.spec.ts")).toBe(false);
});
