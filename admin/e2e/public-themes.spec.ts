// The public starter themes under axe (M4-3b P64; 06 §8 sets WCAG 2.2 AA): each theme on
// a site of its own with the content it is checked on (`projects.ts`, ruling R-b16):
// `blog` on `seed demo`, `business` on `seed demo --profile business`, `docs` on the
// repository's `site/content` imported with `--front-page`. Each `describe` activates its
// theme through the API, waits until the node serves it, runs `expectAccessible` (the
// admin's tags, in both colour schemes) on each page, and puts `blog` back afterwards.
// Runs in the `public-themes-chromium` project only (`pnpm e2e --project
// public-themes-chromium`), its own CI step.
//
// Not covered: the contact form with the contact-form plugin installed (the theme's form
// markup is checked with its `contact_form` setting on, the plugin absent; the plugin's
// own pages are not), a comment thread with comments in it (the seed writes none; the
// post's empty thread and comment form are checked), the phone layout, and Firefox and
// WebKit.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { request as apiRequest, type Page } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { PUBLIC_THEMES, themeSiteKey, type PublicTheme } from "./support/projects";
import { siteInfo } from "./support/server";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
/** A path no theme or seed answers. */
const MISSING = "/no-such-page-e2e/";

interface Check {
  name: string;
  path: string;
  status: 200 | 404;
}

/** The pages P64 lists, per theme. */
const PAGES: Record<PublicTheme, Check[]> = {
  blog: [
    { name: "front_page", path: "/", status: 200 },
    { name: "single_post", path: "/welcome-to-ferritecms/", status: 200 },
    { name: "page", path: "/about/", status: 200 },
    { name: "category_archive", path: "/category/news/", status: 200 },
    { name: "search", path: "/?s=plugin", status: 200 },
    { name: "not_found", path: MISSING, status: 404 },
  ],
  business: [
    { name: "front_page", path: "/", status: 200 },
    { name: "child_page", path: "/home/garden-design/", status: 200 },
    { name: "contact", path: "/contact/", status: 200 },
    { name: "search", path: "/?s=garden", status: 200 },
    { name: "not_found", path: MISSING, status: 404 },
  ],
  docs: [
    { name: "front_page", path: "/", status: 200 },
    { name: "nested_page", path: "/getting-started/install/", status: 200 },
    { name: "search", path: "/?s=theme", status: 200 },
    { name: "not_found", path: MISSING, status: 404 },
  ],
};

/**
 * Activates `theme` on the site `siteKey` (unless it is active) and waits until its node
 * reports loading it and the public front page links the theme's own stylesheet.
 */
async function activate(siteKey: string, theme: PublicTheme): Promise<void> {
  const { url } = siteInfo(siteKey);
  const api = await ContentApi.as(siteKey, url, "admin");
  const anon = await apiRequest.newContext({ baseURL: url });
  try {
    if ((await api.activeTheme()) !== theme) await api.activateTheme(theme);
    await expect
      .poll(
        async () => {
          const entry = (await api.listThemes()).find((t) => t.name === theme);
          return (
            !!entry?.active &&
            entry.nodes.length > 0 &&
            entry.nodes.every((n) => n.name === theme && !n.fallback)
          );
        },
        { message: `the node loads ${theme}`, timeout: 25_000 },
      )
      .toBe(true);
    const css = readFileSync(resolve(REPO, "themes", theme, "assets", "style.css"), "utf8");
    await expect
      .poll(
        async () => {
          const html = await (await anon.get("/")).text();
          const href = /<link rel="stylesheet" href="([^"]+)"/.exec(html)?.[1];
          if (!href) return false;
          const res = await anon.get(href.replaceAll("&#x2f;", "/"));
          return res.ok() && (await res.text()) === css;
        },
        { message: `the front page is drawn by ${theme}`, timeout: 10_000 },
      )
      .toBe(true);
  } finally {
    await anon.dispose();
    await api.dispose();
  }
}

/** `page` at `path`, answered with `status`. */
async function visit(page: Page, path: string, status: number): Promise<void> {
  const res = await page.goto(path);
  expect(res?.status(), `${path} status`).toBe(status);
}

for (const { theme } of PUBLIC_THEMES) {
  const siteKey = themeSiteKey(theme);

  test.describe(theme, () => {
    test.use({ siteKey });
    test.beforeAll(async () => {
      await activate(siteKey, theme);
    });
    test.afterAll(async () => {
      await activate(siteKey, "blog");
    });

    for (const check of PAGES[theme]) {
      test(`${theme}_${check.name}_is_accessible`, async ({ page, guard, expectAccessible }) => {
        if (check.status === 404) {
          // The browser logs the document's 404; the theme's 404 page is what is checked.
          guard.allowFailedResource(new RegExp(`${check.path.replaceAll("/", "\\/")}$`));
        }
        await visit(page, check.path, check.status);
        await expectAccessible(page);
      });
    }

    if (theme === "business") {
      test("business_contact_form_is_accessible", async ({ page, site, expectAccessible }) => {
        // The theme's form markup (`partials/contact-form.html`), shown with the setting;
        // the plugin is not installed, so nothing is sent.
        const api = await ContentApi.as(siteKey, site.url, "admin");
        try {
          await api.patchThemeSettings("business", { contact_form: true });
          await expect
            .poll(async () =>
              (await (await page.request.get("/contact/")).text()).includes("cf-name"),
            )
            .toBe(true);
          await visit(page, "/contact/", 200);
          await expect(page.getByRole("textbox", { name: "Name" })).toBeVisible();
          await expectAccessible(page);
        } finally {
          await api.patchThemeSettings("business", { contact_form: null });
          await api.dispose();
        }
      });
    }

    if (theme === "docs") {
      test("docs_skip_link_is_first_and_focuses_main", async ({ page }) => {
        await visit(page, "/getting-started/install/", 200);
        await page.keyboard.press("Tab");
        const skip = page.getByRole("link", { name: "Skip to content" });
        await expect(skip).toBeFocused();
        await expect(skip).toBeInViewport();
        await page.keyboard.press("Enter");
        await expect(page.locator("main#content")).toBeFocused();
        await expect(page).toHaveURL(/#content$/);
      });
    }
  });
}
