import { screen, waitFor, within } from "@testing-library/react";
import { expect, test } from "vitest";
import type { PluginMetricsDto } from "@/api/plugins";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { NODE_A, NODE_B, plugin } from "./fixtures";
import { PluginsScreen } from "./plugins-list";

const metrics = (failures: { hook: string; count: number }[]): PluginMetricsDto => ({
  window_secs: 86_400,
  failures: failures.map((f) => ({ ...f, bucket_start: new Date().toISOString() })),
  node: { id: NODE_A, since: "2026-10-27T08:00:00Z", hooks: [] },
});

function row(name: string): HTMLElement {
  const cell = screen.getAllByText(name).find((el) => el.dataset.slot === "plugin-name");
  const tr = cell?.closest("tr");
  if (!tr) throw new Error(`no row ${name}`);
  return tr;
}

test("list_shows_state_failures_and_nodes", async () => {
  stubRoutes({
    "/api/v1/plugins": () =>
      json({
        items: [
          plugin({
            name: "flaky",
            enabled: false,
            disabled_reason: "20 failures in 10 minutes",
            nodes: [
              { node: NODE_A, name: "node-a", status: "disabled", version: "1.2.0", error: null },
            ],
          }),
          plugin({ name: "broken-one", report: null, nodes: [] }),
          plugin({
            name: "seo-helper",
            nodes: [
              { node: NODE_A, name: "node-a", status: "loaded", version: "1.2.0", error: null },
              { node: NODE_B, name: "node-b", status: "failed", version: "1.2.0", error: "trap" },
            ],
          }),
        ],
      }),
    "/api/v1/plugins/retained": () => json({ items: [] }),
    "/api/v1/plugins/flaky/metrics": () => json(metrics([{ hook: "render.head", count: 7 }])),
    "/api/v1/plugins/broken-one/metrics": () => json(metrics([])),
    "/api/v1/plugins/seo-helper/metrics": () =>
      json(
        metrics([
          { hook: "render.head", count: 2 },
          { hook: "post.saved", count: 1 },
        ]),
      ),
  });
  renderWithRouter(<PluginsScreen />);
  await screen.findAllByText("seo-helper");
  // By name.
  const names = screen
    .getAllByText(/./, { selector: "[data-slot=plugin-name]" })
    .map((el) => el.textContent);
  expect(names).toEqual(["broken-one", "flaky", "seo-helper"]);

  const flaky = row("flaky");
  expect(within(flaky).getAllByText("Disabled")[0]).toBeVisible();
  expect(within(flaky).getAllByText("20 failures in 10 minutes")[0]).toBeVisible();
  expect(await within(flaky).findByText("7")).toBeVisible();

  expect(within(row("broken-one")).getAllByText("Broken")[0]).toBeVisible();

  const seo = row("seo-helper");
  expect(within(seo).getAllByText("Enabled")[0]).toBeVisible();
  expect(await within(seo).findByText("3")).toBeVisible();
  expect(within(seo).getByText("node-a: Loaded, node-b: Failed")).toBeInTheDocument();
  // No retained data: no section.
  expect(screen.queryByRole("heading", { name: "Retained data" })).toBeNull();
});

test("manifest_text_is_text", async () => {
  stubRoutes({
    "/api/v1/plugins": () =>
      json({
        items: [
          plugin({
            name: "evil",
            description: "<img src=x onerror=alert(1)>",
            publisher: "<script>alert(1)</script>",
          }),
        ],
      }),
    "/api/v1/plugins/retained": () => json({ items: [] }),
    "/api/v1/plugins/evil/metrics": () => json(metrics([])),
  });
  renderWithRouter(<PluginsScreen />);
  await screen.findByText("<img src=x onerror=alert(1)>");
  await waitFor(() => expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeVisible());
  expect(screen.getByText("By <script>alert(1)</script> (not verified)")).toBeVisible();
  expect(document.querySelector("img")).toBeNull();
});

test("retained_data_lists_and_offers_erase", async () => {
  stubRoutes({
    "/api/v1/plugins": () => json({ items: [] }),
    "/api/v1/plugins/retained": () =>
      json({
        items: [
          {
            name: "old-seo",
            deleted_at: "2026-10-20T10:00:00Z",
            purge_after: "2026-11-19T10:00:00Z",
          },
        ],
      }),
  });
  renderWithRouter(<PluginsScreen />);
  expect(await screen.findByText("No plugins installed")).toBeVisible();
  const section = await screen.findByRole("region", { name: "Retained data" });
  expect(within(section).getByText("old-seo")).toBeVisible();
  expect(
    within(section).getByRole("button", { name: "Erase the data of old-seo now" }),
  ).toBeVisible();
});
