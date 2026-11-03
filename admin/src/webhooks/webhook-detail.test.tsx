import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import type { WebhookDto } from "@/api/webhooks";
import { json, renderWithRouter, stubRoutes, type Handler } from "@/test/render";
import { CATALOGUE, DELIVERY_ID, HOOK_ID, TYPES, delivery, detail, webhook } from "./fixtures";
import { WebhookDetail } from "./webhook-detail";

beforeAll(() => {
  // jsdom lacks what Radix Select calls on open.
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
  proto.hasPointerCapture ??= () => false;
  proto.releasePointerCapture ??= () => {};
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const BASE = `/api/v1/webhooks/${HOOK_ID}`;

function routes(hook: WebhookDto, extra: Record<string, Handler> = {}) {
  const calls: { method: string; path: string; body: unknown }[] = [];
  const record =
    (h: Handler): Handler =>
    async (url, req) => {
      const text = req.method === "GET" ? "" : await req.clone().text();
      calls.push({ method: req.method, path: url.pathname, body: text ? JSON.parse(text) : null });
      return h(url, req);
    };
  const table: Record<string, Handler> = {
    "/api/v1/webhooks/events": () => json(CATALOGUE),
    "/api/v1/types": () => json(TYPES),
    [BASE]: (_u, req) =>
      req.method === "PATCH"
        ? json({ ...hook, enabled: !hook.enabled, state: hook.enabled ? "disabled" : "active" })
        : req.method === "DELETE"
          ? new Response(null, { status: 204 })
          : json(hook),
    [`${BASE}/deliveries`]: (url) =>
      json(
        url.searchParams.get("cursor")
          ? {
              items: [
                delivery({ id: "0196f6b0-0000-7000-8000-0000000000d9", status: "delivered" }),
              ],
              next_cursor: null,
            }
          : { items: [delivery()], next_cursor: "next-page" },
      ),
    [`${BASE}/deliveries/${DELIVERY_ID}`]: () => json(detail()),
    ...extra,
  };
  stubRoutes(Object.fromEntries(Object.entries(table).map(([k, h]) => [k, record(h)])));
  return calls;
}

test("detail_actions_by_state", async () => {
  routes(webhook({ state: "disabled", enabled: false, disabled_reason: "failing" }));
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  expect(await screen.findByRole("button", { name: "Enable" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Disable" })).toBeNull();
  expect(screen.getByText("kept failing for 5 days")).toBeTruthy();
  // A disabled endpoint can still be tested (to check it before enabling).
  expect(screen.getByRole("button", { name: "Send test" })).toBeTruthy();
});

test("paused_shows_the_reason_and_no_test", async () => {
  routes(webhook({ state: "paused" }));
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  expect(await screen.findByText(/this database was copied or restored/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Send test" })).toBeNull();
  expect(screen.getByRole("button", { name: "Enable" })).toBeTruthy();
});

test("disable_patches_enabled_false", async () => {
  const calls = routes(webhook());
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  await userEvent.click(await screen.findByRole("button", { name: "Disable" }));
  await waitFor(() =>
    expect(
      calls.some((c) => c.method === "PATCH" && JSON.stringify(c.body) === '{"enabled":false}'),
    ).toBe(true),
  );
});

test("rotate_confirms_and_shows_new_secret_once", async () => {
  const calls = routes(webhook(), {
    [`${BASE}/secret`]: () =>
      json({
        secret: "whsec_bmV3LXNlY3JldC1ieXRlcy0xMjM0NTY=",
        previous_valid_until: "2026-11-03T10:00:00Z",
      }),
  });
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  await userEvent.click(await screen.findByRole("button", { name: "Rotate secret" }));
  const dialog = await screen.findByRole("dialog");
  expect(within(dialog).getByText("Rotate the signing secret of Deploy?")).toBeTruthy();
  expect(calls.some((c) => c.path.endsWith("/secret"))).toBe(false);
  await userEvent.click(within(dialog).getByRole("button", { name: "Rotate" }));
  const field = await screen.findByLabelText("Signing secret");
  expect((field as HTMLInputElement).value).toBe("whsec_bmV3LXNlY3JldC1ieXRlcy0xMjM0NTY=");
  await userEvent.click(screen.getByRole("button", { name: "Done" }));
  await waitFor(() => expect(screen.queryByLabelText("Signing secret")).toBeNull());
});

test("delete_confirms_with_name", async () => {
  const calls = routes(webhook());
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  await userEvent.click(await screen.findByRole("button", { name: "Delete" }));
  const dialog = await screen.findByRole("alertdialog");
  expect(within(dialog).getByText("Delete Deploy?")).toBeTruthy();
  await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
  await waitFor(() => expect(calls.some((c) => c.method === "DELETE")).toBe(true));
});

test("deliveries_filter_and_load_more", async () => {
  const calls = routes(webhook());
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  expect(await screen.findByText("status 500")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Load more" }));
  await waitFor(() =>
    expect(
      document.querySelectorAll("[data-slot='delivery-status']").length,
    ).toBeGreaterThanOrEqual(2),
  );
  const lists = calls.filter((c) => c.path.endsWith("/deliveries"));
  expect(lists).toHaveLength(2);
  const user = userEvent.setup();
  await user.click(screen.getByRole("combobox", { name: "Status" }));
  await user.click(await screen.findByRole("option", { name: "Failed" }));
  await waitFor(() => expect(calls.filter((c) => c.path.endsWith("/deliveries")).length).toBe(3));
});

test("sheet_shows_attempts_redeliver_and_excerpt_as_text", async () => {
  const calls = routes(webhook(), {
    [`${BASE}/deliveries/${DELIVERY_ID}/redeliver`]: () => json({ delivery: DELIVERY_ID }, 202),
  });
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  const cell = await screen.findByText("status 500");
  await userEvent.click(cell);
  const sheet = await waitFor(() => {
    const s = document.querySelector("[data-slot='delivery-sheet']");
    if (!s?.querySelector("[data-slot='attempt']")) throw new Error("not loaded");
    return s as HTMLElement;
  });
  expect(within(sheet).getByText("Attempt 1")).toBeTruthy();
  const excerpt = sheet.querySelector("[data-slot='response-excerpt']");
  // Escaped text, never markup.
  expect(excerpt?.textContent).toBe("<b>broken</b>");
  expect(excerpt?.querySelector("b")).toBeNull();
  expect(sheet.querySelector("[data-slot='event-json']")?.textContent).toContain(
    '"title": "Hello"',
  );
  await userEvent.click(within(sheet).getByRole("button", { name: "Redeliver" }));
  await waitFor(() => expect(calls.some((c) => c.path.endsWith("/redeliver"))).toBe(true));
});

test("test_delivery_polls_and_reports", async () => {
  let polls = 0;
  routes(webhook(), {
    [`${BASE}/test`]: () => json({ delivery: "0196f6b0-0000-7000-8000-0000000000t1" }, 202),
    [`${BASE}/deliveries/0196f6b0-0000-7000-8000-0000000000t1`]: () => {
      polls += 1;
      return json(
        polls < 2
          ? detail({ delivery: delivery({ status: "pending" }), attempts: [] })
          : detail({
              delivery: delivery({ status: "delivered", last_status_code: 204 }),
              attempts: [
                {
                  attempt: 1,
                  started_at: "2026-11-02T09:00:00Z",
                  duration_ms: 37,
                  status_code: 204,
                  error: null,
                  response_excerpt: null,
                },
              ],
            }),
      );
    },
  });
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  await userEvent.click(await screen.findByRole("button", { name: "Send test" }));
  const result = document.querySelector("[data-slot='test-result']") as HTMLElement;
  expect(result.getAttribute("aria-live")).toBe("polite");
  await waitFor(() => expect(result.textContent).toBe("Delivered: 204 in 37 ms"), {
    timeout: 4_000,
  });
  expect(polls).toBe(2);
});

test("redeliver_since_reports_count", async () => {
  const calls = routes(webhook(), {
    [`${BASE}/redeliver`]: () => json({ count: 7 }, 202),
  });
  renderWithRouter(<WebhookDetail id={HOOK_ID} />);
  await userEvent.click(await screen.findByRole("button", { name: "Redeliver failed since…" }));
  const dialog = await screen.findByRole("dialog");
  await userEvent.click(within(dialog).getByRole("button", { name: "Redeliver" }));
  await waitFor(() => expect(calls.some((c) => c.path.endsWith("/redeliver"))).toBe(true));
  const sent = calls.find((c) => c.path.endsWith("/redeliver"))?.body as { since: string };
  expect(Number.isNaN(new Date(sent.since).getTime())).toBe(false);
  // Done: the dialog closes (the count goes to a toast).
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});
