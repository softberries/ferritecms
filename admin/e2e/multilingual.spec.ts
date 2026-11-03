// Multilingual content under the real server (M6-1, design 13 §6): add German in
// Settings → Languages, translate a post from the editor's Translations section, see the
// German page under `/de/` with its hreflang links and switcher, filter the list by
// German, and be refused when removing German while it holds content. Site-wide (it
// changes `site.locales`): it runs with the settings specs, one at a time, and puts the
// one-locale site back in `finally`.

import type { Page } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const group = (page: Page) => page.locator("[data-slot=settings-group]");
const toast = (page: Page, text: string) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });

async function languages(page: Page): Promise<void> {
  await page.goto("/admin/settings/languages");
  await expect(group(page)).toHaveAttribute("data-group", "languages");
  await expect(group(page).locator("[aria-busy=true]")).toHaveCount(0);
}

test("translate_a_post_into_german", async ({ as, site, siteKey, expectAccessible, guard }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const before = await api.getSettings();
  const page = await as("admin");
  const title = uid("Spring");
  const made: Item[] = [];
  try {
    // Settings → Languages: add German.
    await languages(page);
    await page.getByRole("textbox", { name: "Language tag" }).fill("de");
    await page.getByRole("textbox", { name: "Label", exact: true }).fill("Deutsch");
    await page.getByRole("button", { name: "Add language" }).click();
    await page.getByRole("button", { name: "Save changes" }).click();
    await expect(toast(page, "Settings saved")).toBeVisible();
    await expect(page.locator("[data-locale=de]")).toBeVisible();
    await expectAccessible(page);

    // A published English post, translated from its editor.
    const post = await api.transition(await api.create("post", title), "publish");
    made.push(post);
    await page.goto(`/admin/content/post/${post.id}`);
    const section = page.locator("[data-slot=panel-translations]");
    await expect(section).toBeVisible();
    await section.getByRole("button", { name: "Translate into Deutsch" }).click();
    await expect(toast(page, "Draft in Deutsch made")).toBeVisible();
    await expect(page).not.toHaveURL(new RegExp(`/content/post/${post.id}$`));
    const id = page.url().split("/").pop() ?? "";
    const draft = await api.get("post", id);
    made.push(draft);
    const german = await api.transition(
      await api.patch(draft, { title: `${title} DE` }),
      "publish",
    );
    await expectAccessible(page);

    // The German page is under /de/, with its alternate and the switcher.
    await expect
      .poll(async () => (await page.request.get(`/de/${german.slug}/`)).status(), {
        timeout: 10_000,
      })
      .toBe(200);
    await page.goto(`/de/${german.slug}/`);
    await expect(page.locator("html")).toHaveAttribute("lang", "de");
    await expect(page.locator('link[rel=alternate][hreflang="en"]')).toHaveCount(1);
    await expect(page.locator('link[rel=alternate][hreflang="de"]')).toHaveCount(1);

    // The list filtered by German.
    await page.goto("/admin/content/post?locale=de");
    await expect(page.getByRole("link", { name: `${title} DE` })).toBeVisible();
    await expect(page.getByRole("link", { name: title, exact: true })).toHaveCount(0);

    // German holds content: removing it is refused (the `409` is expected).
    guard.allowFailedResource(/\/api\/v1\/settings$/);
    await languages(page);
    await page.getByRole("button", { name: "Remove de" }).click();
    await page.getByRole("button", { name: "Save changes" }).click();
    await expect(group(page).getByRole("alert")).toContainText("de");
    await expect(page.locator("[data-locale=de]")).toHaveCount(0);
  } finally {
    for (const item of made.reverse()) await api.purge(item).catch(() => undefined);
    await api
      .patchSettings({ "site.locales": before["site.locales"] ?? [{ tag: "en" }] })
      .catch(() => undefined);
    await api.dispose();
  }
});

// M6-1 Task 16: a category translated from the term sheet shows under Deutsch; the
// primary menu translated from its editor becomes `primary-de`, listed at its location,
// and an edit of it shows on the cached German home page.
test("terms_and_menus_per_locale", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const before = await api.getSettings();
  const page = await as("admin");
  const name = uid("Season");
  const terms: string[] = [];
  try {
    await api.patchSettings({
      "site.locales": [{ tag: "en" }, { tag: "de", label: "Deutsch" }],
    });
    const en = await api.createTerm("category", name);
    terms.push(en.id);

    await page.goto("/admin/structure/taxonomies/category");
    await page.getByRole("button", { name: `Actions for ${name}`, exact: true }).click();
    await page.getByRole("menuitem", { name: "Edit" }).click();
    const sheet = page.locator("[data-slot=term-sheet]");
    await sheet.getByRole("button", { name: "Translate into Deutsch" }).click();
    await expect(toast(page, "made in Deutsch")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet).toBeHidden();
    const tabs = page.locator("[data-slot=locale-tabs]");
    await tabs.getByRole("button", { name: /Deutsch/ }).click();
    await expect(
      page.getByRole("button", { name: `Actions for ${name}`, exact: true }),
    ).toBeVisible();
    for (const t of await api.listTerms("category")) {
      if (t.name === name && t.id !== en.id) terms.push(t.id);
    }
    expect(terms).toHaveLength(2);

    // The German home page as cached now (it shows the default menu).
    await page.request.get("/de/");
    await page.goto("/admin/structure/menus/primary");
    await page
      .locator("[data-slot=menu-translate]")
      .getByRole("button", { name: "Translate into Deutsch" })
      .click();
    await expect(page).toHaveURL(/\/admin\/structure\/menus\/primary-de$/);
    const de = await api.getMenu("primary-de");
    expect(de?.items.length ?? 0).toBeGreaterThan(0);
    await page.goto("/admin/structure/menus");
    await expect(
      page
        .locator("[data-slot=menu-location][data-name=primary]")
        .locator("[data-locale=de]")
        .getByRole("link"),
    ).toHaveText(/primary-de|Primary/);

    // An entry renamed in the German menu shows on /de/ (its cached copy is flushed).
    const items = (de?.items ?? []).map((i, n) => (n === 0 ? { ...i, label: "Startseite" } : i));
    await api.putMenu("primary-de", {
      label: de?.label ?? "",
      items,
      locale: "de",
      translation_of: "primary",
    });
    await expect
      .poll(async () => (await (await page.request.get("/de/")).text()).includes("Startseite"), {
        timeout: 10_000,
      })
      .toBe(true);
  } finally {
    await api.deleteMenu("primary-de").catch(() => undefined);
    for (const id of terms.reverse()) await api.deleteTerm(id).catch(() => undefined);
    await api
      .patchSettings({ "site.locales": before["site.locales"] ?? [{ tag: "en" }] })
      .catch(() => undefined);
    await api.dispose();
  }
});
