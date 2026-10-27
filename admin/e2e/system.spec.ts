// The System screens under the real server (M2b-3 Task 11; 07 §3 System): status with
// the backends, schema, mail, theme, jobs and nodes; the node cards; jobs with the
// filters in the URL and Retry on a dead job; the audit log filtered by an action prefix;
// and job errors and audit details shown as text (Review Focus 5).
//
// A dead job comes from the public API: an upload that sniffs as PNG but does not decode
// makes a `media.variants` job that fails at once (`Permanent`, so dead after one
// attempt). The test deletes the item first and then retries the job, which finds the
// item gone and ends `done`, so no dead job stays behind.

import type { Page } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const screen = (page: Page) => page.locator("[data-slot=system-screen]");
const toast = (page: Page, text: string) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });
const jobRow = (page: Page, id: string) => page.locator(`tr[data-row-id="${id}"]`);

async function open(page: Page, path: string): Promise<void> {
  await page.goto(`/admin/system/${path}`);
  await expect(screen(page).getByRole("heading", { name: "System", level: 1 })).toBeVisible();
  await expect(screen(page).locator("[aria-busy=true]")).toHaveCount(0);
}

interface Job {
  id: string;
  kind: string;
  status: string;
  last_error?: string | null;
}

async function jobs(api: ContentApi, query: string): Promise<Job[]> {
  const res = await api.ctx.get(`/api/v1/system/jobs?${query}&limit=200`);
  expect(res.ok(), await res.text()).toBe(true);
  return ((await res.json()) as { items: Job[] }).items;
}

/** A file the upload types as PNG (its magic bytes) that no decoder can read. */
const brokenPng = (name: string) => ({
  name,
  mimeType: "image/png",
  buffer: Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    Buffer.from("not really a png at all, just bytes after the signature"),
  ]),
});

test("status_shows_backends_and_schema", async ({ as, site, snap, expectAccessible }) => {
  const page = await as("admin");
  // `/admin/system` opens Status.
  await page.goto("/admin/system");
  await expect(page).toHaveURL(/\/admin\/system\/status$/);
  await expect(screen(page).locator("[aria-busy=true]")).toHaveCount(0);

  const region = (name: string) => screen(page).getByRole("region", { name });
  const facts = region("Site");
  await expect(facts.getByText("SQLite", { exact: true })).toBeVisible();
  await expect(facts.getByText("Local folder", { exact: true })).toBeVisible();
  await expect(facts.getByText("Solo", { exact: true })).toBeVisible();
  await expect(facts.getByText(site.url, { exact: true })).toBeVisible();
  await expect(facts.getByText(/^\d+\.\d+\.\d+/)).toBeVisible();

  // M2b-3 adds no migration: the schema stays at 8.
  const schema = region("Database schema");
  await expect(schema.getByText("Up to date", { exact: true })).toBeVisible();
  await expect(schema).toContainText(
    "The database is at version 8, the version this binary expects.",
  );

  // The E2E site sends mail to a local sink.
  const mail = region("Mail");
  await expect(mail.getByText("Mail is set up")).toBeVisible();
  await expect(mail.getByText("FerriteCMS <admin@example.test>")).toBeVisible();

  await expect(region("Theme").getByText("Every live node serves it.")).toBeVisible();

  // Jobs by status, each a link to the filtered list.
  const deadLink = region("Background jobs").getByRole("link", { name: /Dead/ });
  await expect(deadLink).toHaveAttribute("href", "/admin/system/jobs?status=dead");
  await expect(region("Nodes").getByRole("link", { name: /Live/ })).toContainText("1");

  await snap(page, "system-status");
  await expectAccessible(page);

  await region("Background jobs").getByRole("link", { name: /Done/ }).click();
  await expect(page).toHaveURL(/\/admin\/system\/jobs\?status=done$/);
  await expect(
    screen(page).getByRole("group", { name: "Status" }).getByRole("button", { name: "Done" }),
  ).toHaveAttribute("aria-pressed", "true");
});

test("system_screens", async ({ as, snap, expectAccessible }) => {
  const page = await as("admin");
  // The sidebar's System item and `g y`.
  await page.goto("/admin/");
  const nav = page.getByRole("navigation", { name: "Main" });
  await expect(nav.getByRole("link", { name: "System" })).toBeVisible();
  await page.keyboard.press("g");
  await page.keyboard.press("y");
  await expect(page).toHaveURL(/\/admin\/system\/status$/);

  // Nodes: this one node, live, with what it reports.
  await screen(page)
    .getByRole("navigation", { name: "System" })
    .getByRole("link", { name: "Nodes" })
    .click();
  await expect(page).toHaveURL(/\/admin\/system\/nodes$/);
  const card = screen(page).locator("[data-slot=node-card]").first();
  await expect(card).toBeVisible();
  await expect(card.getByText("Live", { exact: true })).toBeVisible();
  await expect(card.getByText("This node")).toBeVisible();
  await expect(card.getByRole("list", { name: "Job kinds" })).toContainText("media.variants");
  await snap(page, "system-nodes");
  await expectAccessible(page);

  // Jobs: the whole queue, newest first.
  await open(page, "jobs");
  await expect(screen(page).locator("[data-slot=data-table]")).toBeVisible();
  await snap(page, "system-jobs");
  await expectAccessible(page);

  // Audit: the log, newest first.
  await open(page, "audit");
  await expect(screen(page).getByRole("table", { name: "Audit log" })).toBeVisible();
  await snap(page, "system-audit");
  await expectAccessible(page);

  // An unknown tab is not found, like any unknown admin URL.
  await page.goto("/admin/system/backups");
  await expect(page.getByRole("heading", { name: /not found/i })).toBeVisible();
});

test("retry_dead_job", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  let mediaId: string | undefined;
  try {
    const before = new Set((await jobs(api, "kind=media.variants")).map((j) => j.id));
    const media = await api.upload(brokenPng(`${uid("broken")}.png`));
    mediaId = media.id;
    // The job fails at its first run and is dead at once.
    let dead: Job | undefined;
    await expect
      .poll(
        async () => {
          dead = (await jobs(api, "kind=media.variants&status=dead")).find(
            (j) => !before.has(j.id),
          );
          return dead?.status;
        },
        { timeout: 30_000 },
      )
      .toBe("dead");
    if (!dead) throw new Error("no dead job");
    const id = dead.id;
    // Without its item the retried job has nothing to do and ends `done`.
    await api.purgeMedia(media.id);
    mediaId = undefined;

    const page = await as("admin");
    await open(page, "jobs?status=dead&kind=media.variants");
    const row = jobRow(page, id);
    await expect(row).toBeVisible();
    await expect(row).toContainText("image could not be processed");
    await expect(row).toContainText("1 / 5");
    const retried = page.waitForResponse(
      (r) => r.request().method() === "POST" && r.url().endsWith(`/api/v1/system/jobs/${id}/retry`),
    );
    await row.getByRole("button", { name: "Retry media.variants" }).click();
    const res = await retried;
    expect(res.status()).toBe(200);
    const body = (await res.json()) as { status: string; handled: boolean | null };
    expect(body.status).toBe("queued");
    expect(body.handled).toBe(true);
    await expect(toast(page, "Queued again")).toBeVisible();

    // It runs again and is done; the Done filter lists it.
    await expect
      .poll(async () => (await jobs(api, "kind=media.variants")).find((j) => j.id === id)?.status, {
        timeout: 30_000,
      })
      .toBe("done");
    await screen(page)
      .getByRole("group", { name: "Status" })
      .getByRole("button", { name: "Done" })
      .click();
    await expect(page).toHaveURL(/status=done/);
    await expect(page).toHaveURL(/kind=media\.variants/);
    await expect(jobRow(page, id)).toBeVisible();
  } finally {
    if (mediaId) await api.purgeMedia(mediaId);
    await api.dispose();
  }
});

test("audit_filter_by_action_prefix", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "admin");
  // A token named with markup: its audit row's details carry the name.
  const tokenName = `<img src=x onerror=alert(1)> ${uid("tok")}`;
  const created = await api.ctx.post(`/api/v1/users/${api.me.id}/tokens`, {
    headers: { "X-CSRF-Token": await csrf(api) },
    data: { name: tokenName, scopes: ["content:read"] },
  });
  expect(created.status(), await created.text()).toBe(201);
  const token = (await created.json()) as { id: string };
  const revoked = await api.ctx.delete(`/api/v1/users/${api.me.id}/tokens/${token.id}`, {
    headers: { "X-CSRF-Token": await csrf(api) },
  });
  expect(revoked.ok(), await revoked.text()).toBe(true);
  try {
    const page = await as("admin");
    page.on("dialog", (d) => {
      throw new Error(`a dialog opened: ${d.message()}`);
    });
    await open(page, "audit");
    await screen(page).getByRole("button", { name: "Action", exact: true }).click();
    await page.getByRole("combobox", { name: "Find an action" }).fill("token.");
    await page.getByRole("option", { name: "Starts with token." }).click();
    await expect(page).toHaveURL(/\/admin\/system\/audit\?action=token\.$/);
    await expect(screen(page).locator("[aria-busy=true]")).toHaveCount(0);

    const rows = screen(page).locator("tr[data-row]");
    await expect(rows.first()).toBeVisible();
    const codes = await screen(page).locator("[data-slot=audit-action-code]").allTextContents();
    expect(codes.length).toBeGreaterThan(0);
    for (const code of codes) expect(code.startsWith("token.")).toBe(true);

    // Both rows of this test, readable, the name as text.
    const mine = rows.filter({ hasText: token.id });
    await expect(mine).toHaveCount(2);
    await expect(mine.filter({ hasText: "Created an API token" })).toContainText(tokenName);
    await expect(mine.filter({ hasText: "Revoked an API token" })).toHaveCount(1);
    await expect(screen(page).locator("img")).toHaveCount(0);

    // The filter is in the URL: a reload keeps it.
    await page.reload();
    await expect(
      screen(page).getByRole("button", { name: "Action: starts with token.. Change action" }),
    ).toBeVisible();
    await snap(page, "system-audit-filtered");
    await expectAccessible(page);
  } finally {
    await api.dispose();
  }
});

test("hostile_job_error_renders_as_text", async ({ as, snap, expectAccessible }) => {
  const page = await as("admin");
  page.on("dialog", (d) => {
    throw new Error(`a dialog opened: ${d.message()}`);
  });
  const hostile = '<img src=x onerror="alert(1)"><script>alert(2)</script>';
  const long = `${hostile} plugin trap: ${"wasm backtrace frame; ".repeat(20)}`;
  const at = new Date(Date.now() - 3_600_000).toISOString();
  // States the site cannot produce on demand (P17): a dead job with markup in its error,
  // and a queued job of a kind no node runs (H-Y1).
  await page.route(/\/api\/v1\/system\/jobs\?/, (route) =>
    route.fulfill({
      json: {
        items: [
          {
            id: "0190a1b2-c3d4-7e5f-8a9b-0000000000d1",
            kind: "plugin.action",
            status: "dead",
            attempts: 5,
            max_attempts: 5,
            priority: 0,
            created_at: at,
            run_at: at,
            finished_at: at,
            last_error: long,
            locked_by: null,
            handled: true,
          },
          {
            id: "0190a1b2-c3d4-7e5f-8a9b-0000000000d2",
            kind: "search.reindex",
            status: "queued",
            attempts: 0,
            max_attempts: 5,
            priority: 0,
            created_at: at,
            run_at: at,
            finished_at: null,
            last_error: null,
            locked_by: null,
            handled: false,
          },
        ],
        next_cursor: null,
      },
    }),
  );
  await open(page, "jobs");
  const dead = jobRow(page, "0190a1b2-c3d4-7e5f-8a9b-0000000000d1");
  await expect(dead.getByText(long, { exact: true })).toBeAttached();
  await expect(screen(page).locator("img, script")).toHaveCount(0);
  await dead.getByRole("button", { name: "Show the whole error" }).click();
  await expect(dead.getByText(long, { exact: true })).toBeVisible();
  await expect(
    jobRow(page, "0190a1b2-c3d4-7e5f-8a9b-0000000000d2").getByText("No node handles this kind yet"),
  ).toBeVisible();
  await snap(page, "system-jobs-states");
  await expectAccessible(page);
});

async function csrf(api: ContentApi): Promise<string> {
  const me = (await (await api.ctx.get("/api/v1/auth/me")).json()) as { csrf_token: string };
  return me.csrf_token;
}
