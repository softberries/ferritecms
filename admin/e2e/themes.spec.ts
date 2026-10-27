// The Themes screen under the real server (M2b-3 Task 9; 07 §3 Themes, P42): a folder
// picked in the browser is packed into exactly the bytes `ferrite theme pack` writes,
// installs and activates (the nodes report loading it); uploading a theme that is
// installed says it replaces it (Review Focus 4); a theme whose templates do not compile
// lists the server's problems as text; the settings sheet saves; and the review screens.
//
// The active theme is site-wide and other specs run beside this one, so a test that
// activates a theme puts back the one it found in `finally`, straight after its checks.
// The tests of one file run one after the other.

import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page, Request } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const FIXTURES = resolve(dirname(fileURLToPath(import.meta.url)), "../src/appearance/__fixtures__");
const FOLDER = join(FIXTURES, "blog-min");
const TAR = join(FIXTURES, "blog-min.tar");

const screen = (page: Page) => page.locator("[data-slot=themes-screen]");
const card = (page: Page, name: string) =>
  page.locator(`[data-slot=theme-card][data-theme-name="${name}"]`);
const dialog = (page: Page) => page.getByRole("dialog", { name: "Upload a theme" });
const toast = (page: Page, text: string) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });
const isUpload = (r: Request) =>
  r.method() === "POST" && new URL(r.url()).pathname === "/api/v1/themes";

async function open(page: Page): Promise<void> {
  await page.goto("/admin/appearance/themes");
  await expect(screen(page)).toBeVisible();
  await expect(page.locator("[data-slot=theme-card]").first()).toBeVisible();
}

/**
 * Picks `folder` in the open upload dialog: the folder input on engines whose
 * Playwright supports directory upload (Chromium, Firefox); on WebKit the same theme's
 * `.tar` (only the blog-min fixture has one).
 */
async function pickFolder(page: Page, browserName: string, folder: string): Promise<void> {
  if (browserName === "webkit") {
    if (folder !== FOLDER) test.skip(true, "WebKit has no directory upload in Playwright");
    await dialog(page).locator("[data-slot=theme-tar-input]").setInputFiles(TAR);
    return;
  }
  await dialog(page).locator("[data-slot=theme-folder-input]").setInputFiles(folder);
}

/** A theme folder whose index template does not compile. */
function brokenTheme(name: string): string {
  const root = mkdtempSync(join(tmpdir(), "ferrite-e2e-theme-"));
  const dir = join(root, name);
  mkdirSync(join(dir, "templates"), { recursive: true });
  writeFileSync(
    join(dir, "theme.toml"),
    `[theme]\nname = "${name}"\nversion = "0.1.0"\ndescription = "Broken on purpose."\n`,
  );
  writeFileSync(join(dir, "templates", "index.html"), "<h1>{{ site.title </h1>\n");
  return dir;
}

test("upload_folder_installs_and_activates", async ({ as, site, backend, browserName }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const before = await api.activeTheme();
  const page = await as("admin");
  try {
    await open(page);
    await page.getByRole("button", { name: "Upload theme" }).click();
    await pickFolder(page, browserName, FOLDER);
    const picked = dialog(page).locator("[data-slot=theme-picked]");
    await expect(picked.locator("[data-slot=theme-picked-name]")).toHaveText("blog-min");
    // Five files in the folder; the hidden one is left out.
    if (browserName !== "webkit") await expect(picked).toContainText("4 files");

    const upload = page.waitForRequest(isUpload);
    await dialog(page)
      .getByRole("button", { name: /^(Upload|Replace theme)$/ })
      .click();
    expect((await upload).headers()["content-type"]).toBe("application/x-tar");
    await expect(toast(page, "Theme blog-min installed")).toBeVisible();
    await expect(dialog(page)).toBeHidden();
    await expect(card(page, "blog-min")).toContainText("Minimal theme fixture");
    // The bytes sent were the Rust packer's, byte for byte: the stored artifact is the
    // SHA-256 of the committed `blog-min.tar` (Playwright does not expose a Blob body).
    const stored = (await api.listThemes()).find((t) => t.name === "blog-min");
    expect(stored?.hash).toBe(createHash("sha256").update(readFileSync(TAR)).digest("hex"));

    // Activate: confirm, then the card follows the nodes until they load it.
    await card(page, "blog-min").getByRole("button", { name: "Activate blog-min" }).click();
    const confirm = page.getByRole("alertdialog", { name: "Activate blog-min?" });
    await expect(confirm).toContainText(`${before} stays installed`);
    await confirm.getByRole("button", { name: "Activate" }).click();
    await expect(toast(page, "blog-min is the active theme")).toBeVisible();
    await expect(card(page, "blog-min")).toHaveAttribute("data-active", "true");
    await expect(
      card(page, "blog-min").locator("[data-slot=theme-node][data-state=loaded]"),
    ).toHaveCount(1, { timeout: 25_000 });
    await expect(card(page, "blog-min").locator("[data-slot=theme-watch]")).toHaveCount(0);
    // The public site is drawn by it (blog-min's index is one heading).
    await expect
      .poll(async () => (await (await page.request.get("/")).text()).includes("<body><h1>"), {
        timeout: 10_000,
      })
      .toBe(true);
  } finally {
    await api.activateTheme(before);
    await api.dispose();
  }
});

test("replacing_a_theme_warns", async ({
  as,
  site,
  backend,
  browserName,
  snap,
  expectAccessible,
}) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  await api.installTheme(readFileSync(TAR));
  const page = await as("admin");
  try {
    await open(page);
    const uploads: string[] = [];
    page.on("request", (r) => {
      if (isUpload(r)) uploads.push(r.url());
    });
    await page.getByRole("button", { name: "Upload theme" }).click();
    await pickFolder(page, browserName, FOLDER);
    const warning = dialog(page).locator("[data-slot=theme-replace]");
    await expect(warning).toHaveText(
      "A theme named blog-min is installed (version 0.1.0). Uploading replaces it.",
    );
    await expect(dialog(page).getByRole("button", { name: "Replace theme" })).toBeEnabled();
    await snap(page, "themes-upload-replace", { fullPage: false });
    await expectAccessible(page);
    // Nothing is sent until Replace theme is pressed.
    await dialog(page).getByRole("button", { name: "Cancel" }).click();
    await expect(dialog(page)).toBeHidden();
    expect(uploads).toEqual([]);
  } finally {
    await api.dispose();
  }
});

test("upload_error_lists_template_problems", async ({
  as,
  site,
  backend,
  browserName,
  guard,
  snap,
  expectAccessible,
}) => {
  test.skip(browserName === "webkit", "WebKit has no directory upload in Playwright");
  // The browser logs the refused upload's 422; the dialog shows it.
  guard.allowFailedResource(/\/api\/v1\/themes(\?|$)/);
  const name = uid("broken").toLowerCase();
  const folder = brokenTheme(name);
  const api = await ContentApi.as(backend, site.url, "admin");
  const page = await as("admin");
  try {
    await open(page);
    await page.getByRole("button", { name: "Upload theme" }).click();
    await pickFolder(page, browserName, folder);
    await expect(dialog(page).locator("[data-slot=theme-picked-name]")).toHaveText(name);
    await dialog(page).getByRole("button", { name: "Upload" }).click();
    const problems = dialog(page).locator("[data-slot=theme-problems]");
    await expect(problems).toContainText("The theme does not load, so nothing was installed:");
    await expect(problems.locator("li").first()).toContainText("templates/index.html");
    await snap(page, "themes-upload-error", { fullPage: false });
    await expectAccessible(page);
    // Nothing was stored.
    expect((await api.listThemes()).some((t) => t.name === name)).toBe(false);
  } finally {
    rmSync(dirname(folder), { recursive: true, force: true });
    await api.dispose();
  }
});

test("theme_settings_save", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const page = await as("admin");
  try {
    await open(page);
    await card(page, "blog").getByRole("button", { name: "Customize blog" }).click();
    const sheet = page.locator("[data-slot=theme-settings]");
    await expect(sheet.getByRole("heading", { name: "Customize blog" })).toBeVisible();
    const toggle = sheet.getByRole("switch", { name: "Show excerpts in listings" });
    await expect(toggle).toBeVisible();
    await snap(page, "themes-settings", { fullPage: false });
    await expectAccessible(page);

    const was = (await toggle.getAttribute("aria-checked")) === "true";
    await toggle.click();
    const patch = page.waitForRequest(
      (r) => r.method() === "PATCH" && new URL(r.url()).pathname === "/api/v1/themes/blog/settings",
    );
    await sheet.getByRole("button", { name: "Save changes" }).click();
    expect((await patch).postDataJSON()).toEqual({ show_excerpts: !was });
    await expect(toast(page, "Settings saved")).toBeVisible();
    // Stored: the server reads it back.
    expect((await api.themeSettings("blog")).values.show_excerpts).toBe(!was);
  } finally {
    await api.patchThemeSettings("blog", { show_excerpts: null });
    await api.dispose();
  }
});

test("themes_screens", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  // A second theme, so the list has an inactive card to activate.
  await api.installTheme(readFileSync(TAR));
  const page = await as("admin");
  try {
    // `g a` reaches the screen from anywhere.
    await page.goto("/admin/");
    await expect(
      page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Appearance" }),
    ).toBeVisible();
    await page.keyboard.press("g");
    await page.keyboard.press("a");
    await expect(page).toHaveURL(/\/admin\/appearance\/themes$/);
    await expect(card(page, "blog-min")).toBeVisible();
    const active = page.locator("[data-slot=theme-card][data-active]");
    await expect(active.locator("[data-slot=theme-node]")).not.toHaveCount(0);
    await snap(page, "themes");
    await expectAccessible(page);

    await page.getByRole("button", { name: "Upload theme" }).click();
    await expect(dialog(page)).toBeVisible();
    await snap(page, "themes-upload", { fullPage: false });
    await expectAccessible(page);
    await dialog(page).getByRole("button", { name: "Cancel" }).click();
    await expect(dialog(page)).toBeHidden();

    await card(page, "blog-min").getByRole("button", { name: "Activate blog-min" }).click();
    const confirm = page.getByRole("alertdialog", { name: "Activate blog-min?" });
    await expect(confirm).toBeVisible();
    await snap(page, "themes-activate-confirm", { fullPage: false });
    await expectAccessible(page);
    await confirm.getByRole("button", { name: "Cancel" }).click();
    await expect(confirm).toBeHidden();
  } finally {
    await api.dispose();
  }
});
