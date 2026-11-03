import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { qk } from "@/api/keys";
import { i18n } from "@/i18n";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { auditActionText } from "@/system/audit-text";
import { SCOPE_AREAS } from "@/api/tokens";
import { CATALOGUE, HOOK_ID, TYPES, webhook } from "./fixtures";
import { NewWebhookDialog } from "./new-webhook-dialog";
import { WebhooksScreen } from "./webhooks-list";
import { covers, eventsSummary, hostOf, needsAttention } from "./webhooks-model";

const t = i18n.getFixedT(null, "webhooks");

afterEach(() => {
  vi.unstubAllGlobals();
});

test("model_hosts_summaries_and_cover", () => {
  expect(hostOf("https://hooks.example.com:8443/x?token=t")).toBe("hooks.example.com:8443");
  expect(hostOf("not a url")).toBe("not a url");
  expect(eventsSummary(t, ["*"])).toBe("All events");
  expect(eventsSummary(t, ["content.*", "media.uploaded", "media.updated"])).toBe(
    "Content (all), Media (2)",
  );
  expect(covers(["content.*"], "content.published")).toBe(true);
  expect(covers(["media.uploaded"], "media.updated")).toBe(false);
  expect(covers(["*"], "user.created")).toBe(true);
  expect(
    needsAttention([
      webhook({ state: "disabled", disabled_reason: "gone" }),
      webhook({ state: "disabled", disabled_reason: "manual" }),
      webhook({ state: "paused" }),
    ]),
  ).toEqual({ disabled: 1, paused: 1 });
});

test("list_shows_state_badges_and_host_only", async () => {
  stubRoutes({
    "/api/v1/webhooks": () =>
      json({
        items: [
          webhook(),
          webhook({
            id: "0196f6b0-0000-7000-8000-0000000000a2",
            name: "Purge",
            state: "failing",
            failing_since: new Date(Date.now() - 3 * 3600_000).toISOString(),
          }),
          webhook({
            id: "0196f6b0-0000-7000-8000-0000000000a3",
            name: "Chat",
            state: "disabled",
            enabled: false,
            disabled_reason: "gone",
          }),
        ],
      }),
  });
  renderWithRouter(<WebhooksScreen creating={false} setCreating={() => {}} />);
  await screen.findByText("Purge");
  // The host is shown, the full URL only as the tooltip; never the token as text.
  const hosts = screen.getAllByText("hooks.example.com");
  expect(hosts[0]?.getAttribute("title")).toBe("https://hooks.example.com/build?token=s3cr3t");
  expect(screen.queryByText(/s3cr3t/)).toBeNull();
  const states = document.querySelectorAll("[data-slot='webhook-state']");
  const byState = [...states].map((s) => s.getAttribute("data-state"));
  expect(byState).toEqual(expect.arrayContaining(["active", "failing", "disabled"]));
  expect(screen.getAllByText(/Failing since/).length).toBeGreaterThan(0);
  expect(screen.getAllByText("the receiver answered 410 Gone").length).toBeGreaterThan(0);
  // One auto-disabled endpoint: the banner says so.
  const banner = document.querySelector("[data-slot='webhooks-banner']");
  expect(banner?.textContent).toContain("1 webhook was disabled");
});

test("empty_state_offers_new", async () => {
  stubRoutes({ "/api/v1/webhooks": () => json({ items: [] }) });
  const setCreating = vi.fn();
  renderWithRouter(<WebhooksScreen creating={false} setCreating={setCreating} />);
  const empty = await screen.findByText("No webhooks yet");
  const box = empty.closest("[data-slot='empty-state']") as HTMLElement;
  await userEvent.click(within(box).getByRole("button", { name: "New webhook" }));
  expect(setCreating).toHaveBeenCalledWith(true);
  expect(document.querySelector("[data-slot='webhooks-banner']")).toBeNull();
});

function dialogRoutes(create: (body: Record<string, unknown>) => Response) {
  const bodies: Record<string, unknown>[] = [];
  stubRoutes({
    "/api/v1/webhooks/events": () => json(CATALOGUE),
    "/api/v1/types": () => json(TYPES),
    "/api/v1/webhooks": async (_u, req) => {
      const body = (await req.json()) as Record<string, unknown>;
      bodies.push(body);
      return create(body);
    },
  });
  return bodies;
}

test("dialog_validates_and_shows_server_field_errors", async () => {
  const bodies = dialogRoutes(
    () =>
      new Response(
        JSON.stringify({
          type: "/api/v1/problems/validation",
          title: "Invalid input",
          status: 422,
          detail: "the URL must use https",
          errors: [{ field: "url", detail: "the URL must use https" }],
        }),
        { status: 422, headers: { "Content-Type": "application/problem+json" } },
      ),
  );
  renderWithRouter(<NewWebhookDialog open onOpenChange={() => {}} />);
  await userEvent.click(await screen.findByRole("button", { name: "Create webhook" }));
  expect(screen.getByText("Enter a name.")).toBeTruthy();
  expect(screen.getByText("Enter the URL.")).toBeTruthy();
  expect(screen.getByText("Choose at least one event.")).toBeTruthy();
  expect(bodies).toHaveLength(0);
  await userEvent.type(screen.getByLabelText("Name"), "Deploy");
  await userEvent.type(screen.getByLabelText("URL"), "http://hooks.example.com/");
  await userEvent.click(await screen.findByLabelText("All Content events"));
  await userEvent.click(screen.getByRole("button", { name: "Create webhook" }));
  expect(await screen.findByText("the URL must use https")).toBeTruthy();
  expect(bodies[0]).toMatchObject({ name: "Deploy", events: ["content.*"], content_types: null });
});

test("event_picker_groups_and_all_events", async () => {
  dialogRoutes(() => json({}, 500));
  renderWithRouter(<NewWebhookDialog open onOpenChange={() => {}} />);
  const all = await screen.findByLabelText("All events");
  await screen.findByLabelText("content.published");
  // A group covers its types (checked and locked).
  await userEvent.click(screen.getByLabelText("All Media events"));
  const uploaded = screen.getByLabelText("media.uploaded");
  expect(uploaded.getAttribute("data-state")).toBe("checked");
  expect(uploaded.hasAttribute("disabled")).toBe(true);
  await userEvent.click(all);
  const created = screen.getByLabelText("user.created");
  expect(created.getAttribute("data-state")).toBe("checked");
  expect(screen.getByLabelText("All Content events").hasAttribute("disabled")).toBe(true);
});

test("content_type_filter_only_with_content_events", async () => {
  const bodies = dialogRoutes(() => json({}, 500));
  renderWithRouter(<NewWebhookDialog open onOpenChange={() => {}} />);
  await userEvent.click(await screen.findByLabelText("media.uploaded"));
  expect(document.querySelector("[data-slot='content-type-filter']")).toBeNull();
  await userEvent.click(screen.getByLabelText("content.published"));
  const filter = await waitFor(() => {
    const f = document.querySelector("[data-slot='content-type-filter']");
    if (!f) throw new Error("no filter");
    return f as HTMLElement;
  });
  // Media items are not offered (they have their own events).
  expect(within(filter).queryByText("Media")).toBeNull();
  await userEvent.click(within(filter).getByLabelText("Posts"));
  await userEvent.type(screen.getByLabelText("Name"), "Posts only");
  await userEvent.type(screen.getByLabelText("URL"), "https://h.example/");
  await userEvent.click(screen.getByRole("button", { name: "Create webhook" }));
  await waitFor(() => expect(bodies).toHaveLength(1));
  expect(bodies[0]).toMatchObject({
    events: ["media.uploaded", "content.published"],
    content_types: ["post"],
  });
});

test("secret_shown_once_with_copy_once", async () => {
  dialogRoutes(() =>
    json({ webhook: webhook(), secret: "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw" }, 201),
  );
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  let open = true;
  const onOpenChange = vi.fn((next: boolean) => {
    open = next;
  });
  renderWithRouter(<NewWebhookDialog open={open} onOpenChange={onOpenChange} />, qc);
  await userEvent.type(await screen.findByLabelText("Name"), "Deploy");
  await userEvent.type(screen.getByLabelText("URL"), "https://hooks.example.com/");
  await userEvent.click(await screen.findByLabelText("All events"));
  await userEvent.click(screen.getByRole("button", { name: "Create webhook" }));
  const field = await screen.findByLabelText("Signing secret");
  expect((field as HTMLInputElement).value).toBe("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw");
  expect(screen.getByRole("button", { name: "Send test" })).toBeTruthy();
  // Not in the query cache.
  const cached = JSON.stringify(
    qc
      .getQueryCache()
      .getAll()
      .map((q) => q.state.data),
  );
  expect(cached).not.toContain("whsec_");
  expect(qc.getQueryData(qk.webhooks.list)).toBeUndefined();
  await userEvent.click(screen.getByRole("button", { name: "Done" }));
  expect(onOpenChange).toHaveBeenCalledWith(false);
});

test("token_dialog_offers_webhooks_area_and_audit_texts", () => {
  expect(SCOPE_AREAS).toContain("webhooks");
  expect(SCOPE_AREAS[SCOPE_AREAS.length - 1]).toBe("webhooks");
  expect(i18n.getFixedT(null, "admin")("account.tokenDialog.areas.webhooks")).toBe("Webhooks");
  const admin = i18n.getFixedT(null, "admin");
  expect(auditActionText(admin, "webhook.create")).toBe("Created a webhook");
  expect(auditActionText(admin, "webhook.auto_disabled")).toBe(
    "Disabled a failing webhook automatically",
  );
  expect(HOOK_ID).toBeTruthy();
});
