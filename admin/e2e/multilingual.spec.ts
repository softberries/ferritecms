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
