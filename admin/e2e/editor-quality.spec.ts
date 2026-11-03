// The editor's accessibility checks under the real server (M6-4, design 16 §11.1): an
// image without alt text is flagged inline and in the panel's Checks, fixed through the
// alt popover; the decorative switch clears it; a vague link is flagged; a type with
// "Block publishing on errors" disables the publish button, a forced API publish is
// `422 content-quality`, and an editor's suppression unblocks it. Every item and type
// is the test's own.

import type { Page } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const typeName = () => uid("q").slice(0, 32);
const panel = (page: Page) => page.locator("[data-slot=editor-panel]");
const checks = (page: Page) => page.locator("[data-slot=panel-quality]");
const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });

const doc = (content: unknown[]) => ({ type: "doc", content });
const image = { type: "image", attrs: { src: "/favicon.ico" } };
const vague = {
  type: "paragraph",
  content: [
    { type: "text", text: "Read the guide " },
    { type: "text", text: "here", marks: [{ type: "link", attrs: { href: "/" } }] },
  ],
};

async function openItem(page: Page, type: string, id: string): Promise<void> {
  await page.goto(`/admin/content/${type}/${id}`);
  await expect(title(page)).toBeVisible();
  await expect(panel(page)).toBeVisible();
}

test("checks_flag_fix_and_decorative", async ({ as, site, siteKey, expectAccessible }) => {
  const admin = await ContentApi.as(siteKey, site.url, "admin");
  const type = typeName();
  try {
    await admin.createType(type, `Checks ${type.slice(-6)}`);
    const item = await admin.create(type, `Lake ${uid()}`, { body: doc([image, vague]) });
    const page = await as("admin");
    await openItem(page, type, item.id);

    // Inline: a mark with a named button; in the panel: the finding.
    await expect(page.locator("[data-slot=quality-widget]").first()).toBeVisible();
    const widgets = page.locator("[data-slot=quality-widget]");
    await expect(widgets.filter({ hasText: "!" })).toHaveCount(2);
    await expect(
      page.getByRole("button", { name: /Error: An image has no alt text/ }),
    ).toBeVisible();
    await expect(checks(page).locator("[data-rule=image-alt-missing]")).toBeVisible();
    await expect(checks(page).locator("[data-rule=link-text-vague]")).toBeVisible();
    await expectAccessible(page);

    // Fixed through the alt popover: the finding goes.
    await checks(page)
      .locator("[data-rule=image-alt-missing]")
      .getByRole("button", { name: "Add alt text" })
      .click();
    const alt = page.getByRole("textbox", { name: "Alternative text" });
    await expect(alt).toBeVisible();
    await alt.fill("A calm lake at dawn");
    await page.getByRole("button", { name: "Done" }).click();
    await expect(checks(page).locator("[data-rule=image-alt-missing]")).toHaveCount(0, {
      timeout: 10_000,
    });

    // Decorative: on, the alt is cleared and nothing is flagged for the image.
    await page.getByRole("button", { name: /Edit the alt text/ }).click();
    await page.getByRole("switch", { name: "Decorative image" }).click();
    await page.getByRole("button", { name: "Done" }).click();
    await expect(page.getByRole("button", { name: /Decorative/ })).toBeVisible();
    await expect(checks(page).locator("[data-rule^=image-alt]")).toHaveCount(0, {
      timeout: 10_000,
    });
  } finally {
    await admin.removeType(type);
  }
});

test("gate_blocks_publish_until_suppressed", async ({ as, site, siteKey }) => {
  const admin = await ContentApi.as(siteKey, site.url, "admin");
  const editor = await ContentApi.as(siteKey, site.url, "editor");
  const type = typeName();
  try {
    await admin.createType(type, `Gated ${type.slice(-6)}`);
    await admin.patchType(type, { quality_gate: "errors" });
    const item = await editor.create(type, `Gated ${uid()}`, { body: doc([image]) });

    // The server refuses a forced publish.
    const me = (await (await editor.ctx.get("/api/v1/auth/me")).json()) as {
      csrf_token: string;
    };
    const res = await editor.ctx.post(`/api/v1/content/${type}/${item.id}/publish`, {
      headers: { "X-CSRF-Token": me.csrf_token, "If-Match": `"${item.version}"` },
    });
    expect(res.status()).toBe(422);
    const problem = (await res.json()) as { type: string; findings: { rule: string }[] };
    expect(problem.type).toContain("content-quality");
    expect(problem.findings[0]?.rule).toBe("image-alt-missing");

    // The dialog says why and keeps the button disabled.
    const page = await as("editor");
    await openItem(page, type, item.id);
    await expect(checks(page).locator("[data-rule=image-alt-missing]")).toBeVisible();
    await panel(page)
      .getByRole("button", { name: /^Publish/ })
      .first()
      .click();
    const dialog = page.locator("[data-slot=publish-dialog]");
    await expect(dialog).toContainText("1 accessibility error");
    await expect(dialog.locator("[data-slot=publish-confirm]")).toBeDisabled();
    await page.keyboard.press("Escape");

    // An editor suppresses it with a reason: publishing goes through.
    await checks(page)
      .locator("[data-rule=image-alt-missing]")
      .getByRole("button", { name: "Suppress…" })
      .click();
    const sd = page.locator("[data-slot=suppress-dialog]");
    await sd.getByLabel("Why").fill("A decorative divider");
    await sd.getByRole("button", { name: "Suppress", exact: true }).click();
    await expect(checks(page).getByRole("button", { name: "Suppressed (1)" })).toBeVisible();
    await panel(page)
      .getByRole("button", { name: /^Publish/ })
      .first()
      .click();
    await expect(dialog.locator("[data-slot=publish-confirm]")).toBeEnabled();
    await dialog.locator("[data-slot=publish-confirm]").click();
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "Published" }),
    ).toBeVisible();
  } finally {
    await admin.removeType(type);
  }
});
