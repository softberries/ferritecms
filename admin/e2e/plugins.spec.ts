// The Plugins screens under the real server (M2b-3 Task 10; 07 §3 Plugins): install in
// three steps with the permission report and approval (Review Focus 2), a new plugin
// arriving disabled and enabled from the last step; a secret that is kept when untouched
// and kept nowhere in the page (Review Focus 3); delete with the data kept or erased, and
// Erase now under Retained data (Review Focus 4, P47); a hostile manifest shown as text
// (Review Focus 5); the metrics table of this node (P48); and the review screens.
//
// Every test installs the fixture plugin (`plugins/examples/admin-e2e`, built for
// `wasm32-wasip2` before the run) under names of its own (`uid()`), and removes them and
// their data in `finally`.

import type { Page } from "@playwright/test";
import { keptState } from "./support/app-state";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { fixtureComponent, fixtureFiles, fixtureManifest } from "./support/plugin-fixture";
import { uid } from "./support/uid";

const name = () => uid("pl").toLowerCase();
const screen = (page: Page) => page.locator("[data-slot=plugins-screen]");
const detail = (page: Page) => page.locator("[data-slot=plugin-detail]");
const dialog = (page: Page) => page.getByRole("dialog", { name: "Install a plugin" });
const toast = (page: Page, text: string) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });
const row = (page: Page, plugin: string) =>
  screen(page)
    .getByRole("row")
    .filter({ has: page.locator("[data-slot=plugin-name]", { hasText: plugin }) });
const retainedRow = (page: Page, plugin: string) =>
  page.locator(`[data-slot=retained-row][data-name="${plugin}"]`);

async function openList(page: Page): Promise<void> {
  await page.goto("/admin/plugins");
  await expect(screen(page).getByRole("heading", { name: "Plugins", level: 1 })).toBeVisible();
  await expect(screen(page).locator("[aria-busy=true]")).toHaveCount(0);
}

async function openDetail(page: Page, plugin: string): Promise<void> {
  await page.goto(`/admin/plugins/${plugin}`);
  await expect(detail(page).getByRole("heading", { name: plugin, level: 1 })).toBeVisible();
}

/** Installs the fixture under `plugin` through the dialog, up to the report. */
async function inspectInDialog(page: Page, manifest: string): Promise<void> {
  const files = fixtureFiles(manifest);
  await screen(page).getByRole("button", { name: "Install plugin" }).first().click();
  await expect(dialog(page)).toBeVisible();
  await dialog(page).locator("[data-slot=plugin-manifest-input]").setInputFiles(files.manifest);
  await dialog(page).locator("[data-slot=plugin-component-input]").setInputFiles(files.component);
  await expect(dialog(page).locator("[data-slot=plugin-component-input-name]")).toContainText(
    "admin_e2e.wasm",
  );
  await dialog(page).getByRole("button", { name: "Check permissions" }).click();
  await expect(dialog(page).getByRole("group", { name: "What is being installed" })).toBeVisible();
}

test("install_inspect_approve_enable", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const plugin = name();
  try {
    const page = await as("admin");
    await openList(page);
    await inspectInDialog(page, fixtureManifest(plugin));
    const report = dialog(page).locator("[data-slot=permission-report]");
    await expect(report.getByRole("group", { name: "What is being installed" })).toContainText(
      plugin,
    );
    // High-risk items first, each marked; the limit above the default marked.
    const items = report.locator("[data-slot=permission]");
    await expect(items.first()).toHaveAttribute("data-risk", "high");
    const risks = await items.evaluateAll((els) => els.map((e) => e.getAttribute("data-risk")));
    expect(risks.indexOf("low")).toBeGreaterThan(risks.lastIndexOf("high"));
    await expect(report.locator("[data-slot=permission][data-key=http]")).toContainText(
      "api.example.com",
    );
    await expect(report.locator("[data-slot=limit][data-above=true]")).toContainText("48 MiB");
    // A new plugin: nothing is marked New (everything is new).
    await expect(report.locator("[data-slot=permission][data-new=true]")).toHaveCount(0);
    await snap(page, "plugins-install-report", { fullPage: false });
    await expectAccessible(page);

    // (Playwright does not expose a multipart body; `install-flow.test.tsx` checks the
    // parts and the `approve` fingerprint.)
    const install = page.waitForResponse(
      (r) => r.request().method() === "POST" && new URL(r.url()).pathname === "/api/v1/plugins",
    );
    await dialog(page).getByRole("button", { name: "Approve and install" }).click();
    expect((await install).status()).toBe(201);
    const done = dialog(page).locator("[data-slot=install-done]");
    await expect(done).toContainText(`${plugin} 0.1.0 is installed and disabled.`);
    expect((await api.listPlugins()).find((p) => p.name === plugin)?.enabled).toBe(false);
    await snap(page, "plugins-install-done", { fullPage: false });
    await expectAccessible(page);

    await dialog(page).getByRole("button", { name: "Enable" }).click();
    await expect(toast(page, `${plugin} is enabled`)).toBeVisible();
    await expect(dialog(page)).toBeHidden();
    await expect(row(page, plugin).locator("[data-slot=plugin-state]").first()).toHaveText(
      "Enabled",
    );
    expect((await api.listPlugins()).find((p) => p.name === plugin)?.enabled).toBe(true);
    // The node loads it.
    await expect
      .poll(
        async () =>
          (await api.listPlugins()).find((p) => p.name === plugin)?.nodes.map((n) => n.status),
        { timeout: 15_000 },
      )
      .toEqual(["loaded"]);
  } finally {
    await api.removePlugin(plugin);
    await api.dispose();
  }
});

test("untouched_secret_is_kept", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const plugin = name();
  const secret = `e2e-secret-${uid("s")}`;
  try {
    await api.installPlugin(fixtureManifest(plugin), fixtureComponent());
    const page = await as("admin");
    await openDetail(page, plugin);
    const form = detail(page).locator("[data-slot=settings-form]");
    const key = form.locator("[data-slot=setting][data-key=api_key]");
    await expect(key.locator("[data-slot=secret-field]")).toHaveAttribute("data-state", "unset");

    // Set the secret.
    await key.getByRole("button", { name: "Set a value" }).click();
    await key.getByLabel("API key").fill(secret);
    const first = page.waitForRequest(
      (r) =>
        r.method() === "PATCH" &&
        new URL(r.url()).pathname === `/api/v1/plugins/${plugin}/settings`,
    );
    await form.getByRole("button", { name: "Save changes" }).click();
    expect((await first).postDataJSON()).toEqual({ api_key: secret });
    await expect(toast(page, "Settings saved")).toBeVisible();
    // The form reads the settings again: the secret shows as set, its value nowhere.
    await expect(key.locator("[data-slot=secret-field]")).toHaveAttribute("data-state", "set");
    expect((await api.pluginSettings(plugin)).secrets.api_key).toBe(true);
    expect(await keptState(page)).not.toContain(secret);
    expect(await page.content()).not.toContain(secret);

    // Save another field: the secret is left out of the PATCH, and stays set.
    await form.getByLabel("Greeting", { exact: true }).fill("Welcome");
    const second = page.waitForRequest(
      (r) =>
        r.method() === "PATCH" &&
        new URL(r.url()).pathname === `/api/v1/plugins/${plugin}/settings`,
    );
    await form.getByRole("button", { name: "Save changes" }).click();
    expect((await second).postDataJSON()).toEqual({ greeting: "Welcome" });
    await expect(toast(page, "Settings saved").last()).toBeVisible();
    const stored = await api.pluginSettings(plugin);
    expect(stored.secrets.api_key).toBe(true);
    expect(stored.values.greeting).toBe("Welcome");
    expect(await keptState(page)).not.toContain(secret);

    await detail(page).locator("#settings").scrollIntoViewIfNeeded();
    await snap(page, "plugin-settings", { fullPage: false });
    await expectAccessible(page);
  } finally {
    await api.removePlugin(plugin);
    await api.dispose();
  }
});

test("delete_keep_vs_erase", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const kept = name();
  const erased = name();
  try {
    await api.installPlugin(fixtureManifest(kept), fixtureComponent());
    await api.installPlugin(fixtureManifest(erased), fixtureComponent());
    const page = await as("admin");
    // From the plugin's page: keep the data.
    await openDetail(page, kept);
    await detail(page).getByRole("button", { name: "Delete" }).click();
    const confirm = page.getByRole("alertdialog", { name: `Delete ${kept}?` });
    await expect(confirm).toContainText("30 days");
    await expect(confirm.getByRole("button", { name: "Delete and erase data" })).toBeVisible();
    await snap(page, "plugins-delete", { fullPage: false });
    await expectAccessible(page);
    await confirm.getByRole("button", { name: "Delete, keep data for 30 days" }).click();
    await expect(toast(page, `${kept} deleted; its data is kept for 30 days`)).toBeVisible();
    await expect(page).toHaveURL(/\/admin\/plugins$/);
    await expect(retainedRow(page, kept)).toBeVisible();
    expect((await api.retainedPlugins()).map((r) => r.name)).toContain(kept);

    // From the list's row menu: erase the data.
    await row(page, erased).hover();
    await row(page, erased)
      .getByRole("button", { name: `Actions for ${erased}` })
      .click();
    await page.getByRole("menuitem", { name: "Delete" }).click();
    const second = page.getByRole("alertdialog", { name: `Delete ${erased}?` });
    await second.getByRole("button", { name: "Delete and erase data" }).click();
    await expect(toast(page, `${erased} deleted and its data erased`)).toBeVisible();
    await expect(row(page, erased)).toHaveCount(0);
    expect((await api.retainedPlugins()).map((r) => r.name)).not.toContain(erased);
    await expect(retainedRow(page, erased)).toHaveCount(0);

    // Erase now: the kept data goes.
    await retainedRow(page, kept).scrollIntoViewIfNeeded();
    await snap(page, "plugins-retained", { fullPage: false });
    await retainedRow(page, kept)
      .getByRole("button", { name: `Erase the data of ${kept} now` })
      .click();
    const erase = page.getByRole("alertdialog", { name: `Erase the data of ${kept}?` });
    await erase.getByRole("button", { name: "Erase" }).click();
    await expect(toast(page, `The data of ${kept} was erased`)).toBeVisible();
    await expect(retainedRow(page, kept)).toHaveCount(0);
    expect((await api.retainedPlugins()).map((r) => r.name)).not.toContain(kept);
  } finally {
    await api.removePlugin(kept);
    await api.removePlugin(erased);
    await api.dispose();
  }
});

test("hostile_manifest_renders_as_text", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const plugin = name();
  const fields = {
    description: "<img src=x onerror=alert(1)>",
    publisher: "<script>alert(1)</script>",
    license: "<i>MIT</i>",
    homepage: "https://example.com/?q=<b>x</b>",
  };
  try {
    const page = await as("admin");
    page.on("dialog", (d) => {
      throw new Error(`a dialog opened: ${d.message()}`);
    });
    // The report of the upload shows the manifest's text as text.
    await openList(page);
    await inspectInDialog(page, fixtureManifest(plugin, fields));
    await expect(dialog(page).getByText(fields.description, { exact: true })).toBeVisible();
    await expect(dialog(page)).toContainText(`${fields.publisher} (not verified)`);
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toBeHidden();

    await api.installPlugin(fixtureManifest(plugin, fields), fixtureComponent());
    await openList(page);
    const r = row(page, plugin);
    await expect(r.locator("[data-slot=plugin-description]")).toHaveText(fields.description);
    await expect(r.locator("[data-slot=plugin-publisher]")).toHaveText(
      `By ${fields.publisher} (not verified)`,
    );

    await openDetail(page, plugin);
    await expect(detail(page).locator("[data-slot=plugin-description]")).toHaveText(
      fields.description,
    );
    await expect(detail(page).locator("[data-slot=plugin-license]")).toHaveText(
      `Licence ${fields.license}`,
    );
    // An http(s) homepage is a link, its text the manifest's.
    const link = detail(page).locator("[data-slot=plugin-homepage] a");
    await expect(link).toHaveText(fields.homepage);
    await expect(link).toHaveAttribute("rel", "noopener noreferrer nofollow");
    // Nothing became markup.
    await expect(page.locator("img[src=x], i:text('MIT'), b:text('x')")).toHaveCount(0);
    await expect(page.locator("script:text('alert(1)')")).toHaveCount(0);
  } finally {
    await api.removePlugin(plugin);
    await api.dispose();
  }
});

test("metrics_table_shows_this_node", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const plugin = name();
  try {
    await api.installPlugin(fixtureManifest(plugin), fixtureComponent());
    await api.enablePlugin(plugin);
    const page = await as("admin");
    await expect
      .poll(
        async () =>
          (await api.listPlugins()).find((p) => p.name === plugin)?.nodes.map((n) => n.status),
        { timeout: 15_000 },
      )
      .toEqual(["loaded"]);
    // Pages the cache has not seen, so the theme renders and the hook runs.
    await expect
      .poll(
        async () => {
          await page.request.get(`/?e2e=${uid("m")}`);
          await page.request.get(`/${uid("missing")}/`);
          const res = await api.ctx.get(`/api/v1/plugins/${plugin}/metrics`);
          const body = (await res.json()) as { node: { hooks: { hook: string; calls: number }[] } };
          return body.node.hooks.find((h) => h.hook === "render.head")?.calls ?? 0;
        },
        { timeout: 15_000 },
      )
      .toBeGreaterThan(0);

    await openDetail(page, plugin);
    await expect(detail(page).locator("[data-slot=metrics-scope]")).toHaveText(
      /^This node, since /,
    );
    const hook = detail(page).locator("[data-slot=hook-row][data-hook='render.head']");
    await expect(hook).toBeVisible();
    expect(Number(await hook.locator("[data-col=calls]").textContent())).toBeGreaterThan(0);
    await expect(hook.locator("[data-col=p95]")).toHaveText(/^(≤ \d+ ms|> 500 ms)$/);
    await expect(detail(page).locator("[data-slot=plugin-node][data-status=loaded]")).toHaveCount(
      1,
    );
    await snap(page, "plugin-detail");
    await expectAccessible(page);
  } finally {
    await api.removePlugin(plugin);
    await api.dispose();
  }
});

test("plugins_screens", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  const shown = name();
  const gone = name();
  try {
    await api.installPlugin(fixtureManifest(shown), fixtureComponent());
    await api.enablePlugin(shown);
    await api.installPlugin(fixtureManifest(gone), fixtureComponent());
    const del = await api.ctx.delete(`/api/v1/plugins/${gone}`, {
      headers: { "X-CSRF-Token": await csrf(api) },
    });
    expect(del.status()).toBe(204);
    const page = await as("admin");
    // The node has loaded it, so the row shows its dot.
    await expect
      .poll(
        async () =>
          (await api.listPlugins()).find((p) => p.name === shown)?.nodes.map((n) => n.status),
        { timeout: 15_000 },
      )
      .toEqual(["loaded"]);
    // `g l` reaches the screen from anywhere.
    await page.goto("/admin/");
    await expect(
      page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Plugins" }),
    ).toBeVisible();
    await page.keyboard.press("g");
    await page.keyboard.press("l");
    await expect(page).toHaveURL(/\/admin\/plugins$/);
    await expect(row(page, shown)).toBeVisible();
    await expect(retainedRow(page, gone)).toBeVisible();
    await snap(page, "plugins");
    await expectAccessible(page);

    await screen(page).getByRole("button", { name: "Install plugin" }).first().click();
    await expect(dialog(page)).toBeVisible();
    await snap(page, "plugins-install", { fullPage: false });
    await expectAccessible(page);
    await dialog(page).getByRole("button", { name: "Cancel" }).click();
    await expect(dialog(page)).toBeHidden();

    // A row opens the plugin.
    await row(page, shown).locator("[data-slot=plugin-name]").click();
    await expect(page).toHaveURL(new RegExp(`/admin/plugins/${shown}$`));
    await expect(detail(page).getByRole("heading", { name: shown, level: 1 })).toBeVisible();
  } finally {
    await api.removePlugin(shown);
    await api.removePlugin(gone);
    await api.dispose();
  }
});

test("plugins_need_manage_plugins", async ({ as }) => {
  const page = await as("editor");
  await page.goto("/admin/plugins");
  await expect(page.getByRole("heading", { name: "You don't have access to this" })).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Plugins" }),
  ).toHaveCount(0);
});

/** The session's CSRF token (for a raw request outside the helper's methods). */
async function csrf(api: ContentApi): Promise<string> {
  const me = (await (await api.ctx.get("/api/v1/auth/me")).json()) as { csrf_token: string };
  return me.csrf_token;
}
