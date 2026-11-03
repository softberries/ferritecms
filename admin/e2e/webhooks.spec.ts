// Webhooks under the real server (M6-6, design 18 §10.1): create a webhook pointing at a
// local sink (the site runs with `[webhooks] local_targets`), copy the secret once, Send
// test (the sink checks the signature with the copied secret), a published post becomes a
// `content.published` delivery, a failing sink schedules attempt 2, rotation signs with
// both secrets, a disabled webhook sends nothing, delete; axe on the list, the dialog, the
// detail and the delivery sheet. Other workers publish too: the test looks for its own
// items only.

import type { Page } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";
import { type WebhookSink, startWebhookSink, verify } from "./support/webhook-sink";

const dialog = (page: Page) => page.locator("[data-slot=new-webhook-dialog]");
const detail = (page: Page) => page.locator("[data-slot=webhook-detail]");

/** The sink's hits whose `data.id` is `item`. */
const hitsFor = (sink: WebhookSink, item: string) =>
  sink.hits.filter((h) => h.json.data?.id === item);

test("webhook_end_to_end", async ({ as, site, siteKey, guard, expectAccessible }) => {
  test.setTimeout(120_000);
  // A refused test while paused, and the sink's 500s, show as failed requests in the log
  // pane; the 500s never reach the browser, but the list refetches race the delete.
  guard.allowFailedResource(/\/api\/v1\/webhooks/);
  const sink = await startWebhookSink();
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const name = `Sink ${uid("")}`;
  const page = await as("admin");
  try {
    await page.goto("/admin/webhooks");
    await expect(page.locator("[data-slot=webhooks-screen]")).toBeVisible();
    await expectAccessible(page);

    // Create, with every content event.
    await page.getByRole("button", { name: "New webhook" }).first().click();
    await expect(dialog(page)).toBeVisible();
    await dialog(page).getByLabel("Name").fill(name);
    await dialog(page).getByLabel("URL").fill(sink.url);
    await dialog(page).getByLabel("All Content events").check();
    await expectAccessible(page);
    await dialog(page).getByRole("button", { name: "Create webhook" }).click();
    const field = dialog(page).getByLabel("Signing secret");
    await expect(field).toBeVisible();
    const secret = await field.inputValue();
    expect(secret).toMatch(/^whsec_/);

    // Send test: delivered, and the signature checks out with the copied secret.
    await dialog(page).getByRole("button", { name: "Send test" }).click();
    await expect(dialog(page).locator("[data-slot=test-result]")).toHaveText(/^Delivered: 200/, {
      timeout: 25_000,
    });
    const ping = sink.hits.find((h) => h.json.type === "webhook.ping");
    expect(ping, "the sink got the ping").toBeTruthy();
    expect(verify(ping!, secret)).toBe(true);
    expect(ping!.headers["user-agent"]).toMatch(/ webhooks$/);
    await dialog(page).getByRole("button", { name: "Done" }).click();
    await expect(dialog(page)).toHaveCount(0);

    // The detail.
    await page.getByRole("table", { name: "Webhooks" }).getByText(name).click();
    await expect(detail(page)).toBeVisible();
    const id = page.url().split("/").pop() ?? "";

    // A published post is delivered.
    const post = await api.create("post", `Webhook post ${uid("")}`);
    await api.transition(post, "publish");
    await expect
      .poll(() => hitsFor(sink, post.id).map((h) => h.json.type), { timeout: 20_000 })
      .toContain("content.published");
    const published = hitsFor(sink, post.id).find((h) => h.json.type === "content.published")!;
    expect(verify(published, secret)).toBe(true);
    await page.reload();
    await expect(detail(page).getByText("content.published").first()).toBeVisible();
    await expectAccessible(page);

    // The sink fails: attempt 2 is scheduled (shown on the delivery).
    sink.answer = 500;
    const failing = await api.create("post", `Webhook failing ${uid("")}`);
    await api.transition(failing, "publish");
    await expect
      .poll(() => hitsFor(sink, failing.id).length, { timeout: 20_000 })
      .toBeGreaterThan(0);
    // The sink answers before the job writes the attempt, so this is polled.
    await expect
      .poll(async () => {
        const log = await api.ctx.get(`/api/v1/webhooks/${id}/deliveries?status=pending`);
        const pending = (await log.json()) as {
          items: { id: string; attempts: number; next_attempt_at: string | null }[];
        };
        return pending.items.some((d) => d.attempts >= 1 && d.next_attempt_at !== null);
      })
      .toBe(true);
    await page.reload();
    const row = detail(page)
      .getByRole("table", { name: "Deliveries" })
      .getByText("status 500")
      .first();
    await row.click();
    const sheet = page.locator("[data-slot=delivery-sheet]");
    await expect(sheet.locator("[data-slot=attempt]").first()).toContainText("500");
    await expect(sheet.locator("[data-slot=response-excerpt]").first()).toHaveText(
      "receiver says no",
    );
    await expectAccessible(page);
    await page.keyboard.press("Escape");
    sink.answer = 200;

    // Rotate: the next delivery carries the new and the old signature.
    await detail(page).getByRole("button", { name: "Rotate secret" }).click();
    const rotate = page.locator("[data-slot=rotate-secret-dialog]");
    await rotate.getByRole("button", { name: "Rotate" }).click();
    const fresh = await rotate.getByLabel("Signing secret").inputValue();
    expect(fresh).not.toBe(secret);
    await rotate.getByRole("button", { name: "Done" }).click();
    const before = sink.hits.length;
    await detail(page).getByRole("button", { name: "Send test" }).click();
    await expect(detail(page).locator("[data-slot=test-result]")).toHaveText(/^Delivered: 200/, {
      timeout: 25_000,
    });
    const rotated = sink.hits.slice(before).find((h) => h.json.type === "webhook.ping")!;
    expect(rotated.headers["webhook-signature"]?.split(" ")).toHaveLength(2);
    expect(verify(rotated, fresh)).toBe(true);
    expect(verify(rotated, secret)).toBe(true);

    // Disabled: a publish sends nothing.
    await detail(page).getByRole("button", { name: "Disable" }).click();
    await expect(detail(page).getByRole("button", { name: "Enable" })).toBeVisible();
    const quiet = await api.create("post", `Webhook quiet ${uid("")}`);
    await api.transition(quiet, "publish");
    await page.waitForTimeout(2_000);
    expect(hitsFor(sink, quiet.id)).toHaveLength(0);

    // Delete.
    await detail(page).getByRole("button", { name: "Delete" }).click();
    const confirm = page.locator("[data-slot=delete-webhook]");
    await expect(confirm.getByText(`Delete ${name}?`)).toBeVisible();
    await confirm.getByRole("button", { name: "Delete" }).click();
    await expect(page.locator("[data-slot=webhooks-screen]")).toBeVisible();
    await expect(page.getByRole("table", { name: "Webhooks" }).getByText(name)).toHaveCount(0);
  } finally {
    // Leave no webhook behind for the other specs.
    const list = await api.ctx.get("/api/v1/webhooks");
    if (list.ok()) {
      const { items } = (await list.json()) as { items: { id: string; name: string }[] };
      for (const w of items.filter((i) => i.name === name)) {
        const me = await api.ctx.get("/api/v1/auth/me");
        const { csrf_token } = (await me.json()) as { csrf_token: string };
        await api.ctx.delete(`/api/v1/webhooks/${w.id}`, {
          headers: { "X-CSRF-Token": csrf_token },
        });
      }
    }
    await sink.stop();
  }
});
