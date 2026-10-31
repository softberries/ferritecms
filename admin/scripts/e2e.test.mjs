import { expect, test } from "vitest";
import { playwrightArgs } from "./e2e.mjs";

const sqlite = { all: false, pg: false };

// The site-wide teardown of each named project is not in the arguments: Playwright runs a
// project's teardown by itself.
test("default_names_the_chromium_projects", () => {
  expect(playwrightArgs([], sqlite)).toEqual(["test", "--project=chromium-sqlite"]);
  expect(playwrightArgs([], { all: false, pg: true })).toEqual([
    "test",
    "--project=chromium-sqlite",
    "--project=chromium-pg",
  ]);
});

test("all_names_no_project", () => {
  expect(playwrightArgs(["--workers", "4"], { all: true, pg: true })).toEqual([
    "test",
    "--workers",
    "4",
  ]);
});

test("a_file_names_the_twin_and_drops_deps", () => {
  expect(playwrightArgs(["e2e/settings.spec.ts"], sqlite)).toEqual([
    "test",
    "--project=chromium-sqlite",
    "--project=chromium-sqlite-site-wide",
    "--no-deps",
    "e2e/settings.spec.ts",
  ]);
});

test("project_values_and_option_values_are_not_files", () => {
  expect(playwrightArgs(["--project", "firefox-sqlite", "--retries", "2"], sqlite)).toEqual([
    "test",
    "--project=firefox-sqlite",
    "--retries",
    "2",
  ]);
  expect(playwrightArgs(["-g", "screens", "--project=webkit-sqlite"], sqlite)).toEqual([
    "test",
    "--project=webkit-sqlite",
    "--project=webkit-sqlite-site-wide",
    "--no-deps",
    "-g",
    "screens",
  ]);
});

test("all_with_a_filter_names_every_project", () => {
  expect(playwrightArgs(["e2e/menus.spec.ts"], { all: true, pg: false })).toEqual([
    "test",
    "--project=chromium-sqlite",
    "--project=firefox-sqlite",
    "--project=webkit-sqlite",
    "--project=chromium-sqlite-site-wide",
    "--project=firefox-sqlite-site-wide",
    "--project=webkit-sqlite-site-wide",
    "--no-deps",
    "e2e/menus.spec.ts",
  ]);
});

test("a_file_after_project_is_a_file", () => {
  expect(playwrightArgs(["--project", "firefox-sqlite", "e2e/menus.spec.ts:12"], sqlite)).toEqual([
    "test",
    "--project=firefox-sqlite",
    "--project=firefox-sqlite-site-wide",
    "--no-deps",
    "e2e/menus.spec.ts:12",
  ]);
});

test("public_themes_has_no_twin", () => {
  // `public-themes-chromium` (the starter themes' axe checks) is one project, without a
  // `-site-wide` teardown; naming one would fail ("Project(s) not found").
  expect(
    playwrightArgs(["--project", "public-themes-chromium", "e2e/public-themes.spec.ts"], sqlite),
  ).toEqual(["test", "--project=public-themes-chromium", "--no-deps", "e2e/public-themes.spec.ts"]);
  expect(playwrightArgs(["--project=public-themes-chromium"], sqlite)).toEqual([
    "test",
    "--project=public-themes-chromium",
  ]);
});
