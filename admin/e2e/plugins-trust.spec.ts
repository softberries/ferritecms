// Signed plugins and the registry under the real server (M6-2, design 14 §10): each site
// of the run has a signed `file://` registry made with throwaway keys
// (`support/registry-fixture.ts`). Check for updates fetches it; the Registry tab installs
// the signed plugin with "Verified publisher" in its report; the list's Update chip moves
// it to the next version; an unsigned upload under the same name is refused with the
// publisher message; revoking the publisher key as compromised disables it; axe on the
// Registry tab and the Publishers page. One serial flow: the steps build on each other and
// the revocation is for good on this site. A site-wide spec (`SITE_WIDE_SPECS`): it runs
// after the rest, alone, since it installs and removes a plugin every list shows.

import type { Page } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { fixtureFiles, fixtureManifest } from "./support/plugin-fixture";
import { REGISTRY_PLUGIN, REGISTRY_PUBLISHER } from "./support/registry-fixture";

test.describe.configure({ mode: "serial" });

const screen = (page: Page) => page.locator("[data-slot=plugins-screen]");
const toast = (page: Page, text: string) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });
const row = (page: Page, plugin: string) =>
  screen(page)
    .getByRole("row")
    .filter({ has: page.locator("[data-slot=plugin-name]", { hasText: plugin }) });

async function openRegistry(page: Page): Promise<void> {
  await page.goto("/admin/plugins?tab=registry");
  await expect(screen(page).getByRole("heading", { name: "Plugins", level: 1 })).toBeVisible();
  await expect(screen(page).getByRole("tab", { name: "Registry" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
}

test("registry_install_update_refuse_and_revoke", async ({
  as,
  site,
  siteKey,
  guard,
  expectAccessible,
}) => {
  // The unsigned upload is refused with a 422 the browser logs.
  guard.allowFailedResource(/\/api\/v1\/plugins\/inspect$/);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  try {
    const page = await as("admin");
    await openRegistry(page);
    // Check for updates fetches the index; the plugin is listed, its publisher verified.
    await screen(page).locator("[data-slot=registry-check]").click();
    await expect(toast(page, "The registry was checked")).toBeVisible();
    const entry = page.locator(`[data-slot=registry-plugin][data-name=${REGISTRY_PLUGIN}]`);
    await expect(entry).toBeVisible();
    await expect(entry).toContainText(`${REGISTRY_PUBLISHER} · Verified`);
    await expect(entry).toContainText("Latest 0.2.0");
    await expectAccessible(page);

    // Install from the Registry tab: the report step says Verified publisher.
    await entry.getByRole("button", { name: `Install ${REGISTRY_PLUGIN}` }).click();
    const dialog = page.getByRole("dialog", {
      name: `Install ${REGISTRY_PLUGIN} from the registry`,
    });
    await expect(dialog.locator("[data-slot=trust-summary]")).toHaveAttribute(
      "data-state",
      "verified",
    );
    await expect(dialog).toContainText("Verified publisher");
    await expect(dialog).toContainText(REGISTRY_PUBLISHER);
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();

    // 0.1.0 through the API, then the list's Update chip installs 0.2.0.
    await api.installFromRegistry(REGISTRY_PLUGIN, "0.1.0");
    await page.goto("/admin/plugins");
    const chip = row(page, REGISTRY_PLUGIN).getByRole("button", {
      name: `Update ${REGISTRY_PLUGIN} to 0.2.0`,
    });
    await expect(chip).toBeVisible();
    await expect(row(page, REGISTRY_PLUGIN).locator("[data-slot=plugin-publisher]")).toHaveText(
      `${REGISTRY_PUBLISHER} · Verified`,
    );
    await chip.click();
    const update = page.getByRole("dialog", {
      name: `Install ${REGISTRY_PLUGIN} from the registry`,
    });
    await expect(update).toContainText("Verified publisher");
    await update.getByRole("button", { name: "Approve and install" }).click();
    await expect(update.locator("[data-slot=install-done]")).toContainText(
      `${REGISTRY_PLUGIN} 0.2.0`,
    );
    await update.getByRole("button", { name: "Close", exact: true }).first().click();
    await expect(row(page, REGISTRY_PLUGIN)).toContainText("0.2.0");
    await expect(chip).toBeHidden();

    // An unsigned upload under the same name: refused with the publisher message.
    const files = fixtureFiles(
      fixtureManifest(REGISTRY_PLUGIN, { version: "0.3.0", publisher: "ferritecms.dev" }),
    );
    await screen(page).getByRole("button", { name: "Install plugin" }).first().click();
    const upload = page.getByRole("dialog", { name: "Install a plugin" });
    await upload.locator("[data-slot=plugin-manifest-input]").setInputFiles(files.manifest);
    await upload.locator("[data-slot=plugin-component-input]").setInputFiles(files.component);
    await upload.getByRole("button", { name: "Check permissions" }).click();
    await expect(upload.locator("[data-slot=trust-summary]")).toHaveAttribute(
      "data-state",
      "refused",
    );
    await expect(upload).toContainText(`belongs to publisher ${REGISTRY_PUBLISHER} (registry)`);
    await expect(upload.getByRole("button", { name: "Approve and install" })).toBeDisabled();
    await upload.getByRole("button", { name: "Back" }).click();
    await upload.getByRole("button", { name: "Cancel" }).click();

    // Enabled, then its publisher key revoked as compromised: the plugin is disabled.
    await api.enablePlugin(REGISTRY_PLUGIN);
    await page.goto("/admin/plugins/publishers");
    const publishers = page.locator("[data-slot=publishers-screen]");
    await expect(
      publishers.getByRole("heading", { name: "Publisher keys", level: 1 }),
    ).toBeVisible();
    await expect(publishers.locator("[data-slot=signature-policy]")).toContainText("any");
    const key = publishers
      .locator("[data-slot=publisher-key]")
      .filter({ hasText: REGISTRY_PUBLISHER });
    await expect(key).toContainText(REGISTRY_PLUGIN);
    await expectAccessible(page);
    await key.getByRole("button", { name: /^Revoke key / }).click();
    const revoke = page.locator("[data-slot=revoke-key]");
    await revoke.getByLabel("Reason").fill("leaked in e2e");
    await revoke.getByLabel("The key was stolen or leaked (disables its plugins)").check();
    await revoke.getByRole("button", { name: "Revoke key" }).click();
    await expect(key.locator("[data-slot=publisher-key-state]")).toContainText(
      "Revoked, compromised: leaked in e2e",
    );
    await page.goto(`/admin/plugins/${REGISTRY_PLUGIN}`);
    const detail = page.locator("[data-slot=plugin-detail]");
    await expect(detail.locator("[data-slot=plugin-disabled-reason]")).toContainText(
      "publisher key revoked: leaked in e2e",
    );
    await expect(detail.locator("[data-slot=plugin-publisher]")).toHaveAttribute(
      "data-trust",
      "revoked",
    );
  } finally {
    await api.removePlugin(REGISTRY_PLUGIN);
    await api.dispose();
  }
});
