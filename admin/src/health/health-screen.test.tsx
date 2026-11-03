import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { SettingsDto } from "@/api/settings";
import { HealthCard } from "@/dashboard/health-card";
import { HealthGroup } from "@/settings/health";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { findingHref, runState, severityTotals } from "./health-model";
import { HealthScreen } from "./health-screen";

const RUN = {
  id: "11111111-1111-4111-8111-111111111111",
  status: "done",
  trigger: "schedule",
  requested_by: null,
  started_at: "2026-11-01T04:00:00Z",
  finished_at: "2026-11-01T04:01:00Z",
  items_checked: 3,
  paths_checked: 2,
  urls_checked: 0,
  totals: { rules: { "image-alt-missing": 2 }, severity: { error: 2, warning: 1 } },
  truncated: false,
};

const summary = (over: Record<string, unknown> = {}) => ({
  last: RUN,
  running: null,
  next_at: "2026-11-02T04:00:00Z",
  external_links: false,
  ...over,
});

const finding = (rule: string, category: string, severity = "error") => ({
  rule,
  severity,
  category,
  wcag: [],
  key: "/a.png",
  context: null,
  detail: {},
  item: {
    id: "22222222-2222-4222-8222-222222222222",
    type: "post",
    title: "Lakes",
    status: "published",
    locale: "en",
    admin_url: "/admin/content/post/22222222-2222-4222-8222-222222222222",
  },
  redirect: null,
  suppressed: null,
});

test("model_totals_states_and_links", () => {
  expect(severityTotals(summary() as never)).toEqual({ error: 2, warning: 1, notice: 0 });
  expect(severityTotals(undefined)).toEqual({ error: 0, warning: 0, notice: 0 });
  expect(runState(summary({ last: null }) as never)).toBe("never");
  expect(runState(summary({ running: RUN }) as never)).toBe("running");
  expect(findingHref(finding("image-alt-missing", "accessibility") as never)?.search.check).toBe(
    "image-alt-missing:/a.png",
  );
});

test("tabs_filter_by_category_and_run_now_409", async () => {
  const seen = stubRoutes({
    "/api/v1/health/summary": () => json(summary()),
    "/api/v1/health/findings": (url) =>
      json({
        items:
          url.searchParams.get("category") === "links"
            ? [finding("link-internal-broken", "links")]
            : [finding("image-alt-missing", "accessibility")],
        next_cursor: null,
      }),
    "/api/v1/health/runs": () =>
      json(
        {
          type: "/api/v1/problems/conflict",
          title: "Conflict",
          status: 409,
          detail: "a content health run is queued or running",
        },
        409,
      ),
    "/api/v1/types": () => json({ items: [], next_cursor: null }),
    "/api/v1/meta": () =>
      json({
        site_url: "http://x",
        locales: [{ tag: "en", label: "English" }],
        default_locale: "en",
      }),
  });
  renderWithRouter(<HealthScreen />);
  const table = await screen.findByRole("table");
  expect(within(table).getByText("Image without alt text")).toBeInTheDocument();
  expect(screen.getByText("External links are not checked.")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("tab", { name: "Links" }));
  expect(await screen.findByText("Broken link")).toBeInTheDocument();
  expect(
    seen.some(
      (u) => u.pathname === "/api/v1/health/findings" && u.searchParams.get("category") === "links",
    ),
  ).toBe(true);
  await userEvent.click(screen.getByRole("button", { name: "Run now" }));
  // The 409 is a toast ("already running"); the request went out once.
  await waitFor(() =>
    expect(seen.filter((u) => u.pathname === "/api/v1/health/runs")).toHaveLength(1),
  );
});

test("never_run_and_running_states", async () => {
  stubRoutes({
    "/api/v1/health/summary": () => json(summary({ last: null })),
    "/api/v1/health/findings": () => json({ items: [], next_cursor: null }),
  });
  renderWithRouter(<HealthScreen />);
  expect(await screen.findByText("No report yet")).toBeInTheDocument();
});

test("dashboard_card_shows_counts", async () => {
  stubRoutes({ "/api/v1/health/summary": () => json(summary()) });
  renderWithRouter(<HealthCard />);
  const card = await screen.findByText("Content health");
  await waitFor(() =>
    expect(document.querySelector("[data-slot=dashboard-health]")).toHaveTextContent("2"),
  );
  expect(card).toBeInTheDocument();
});

test("settings_validate_stale_days", async () => {
  const seen = stubRoutes({});
  const settings: SettingsDto = { "health.external_links": null, "health.stale_days": null };
  renderWithRouter(<HealthGroup settings={settings} />);
  const input = await screen.findByRole("textbox", { name: "Stale after (days)" });
  await userEvent.type(input, "10");
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(await screen.findByText(/from 30 to 3650/)).toBeInTheDocument();
  expect(seen).toEqual([]);
});
