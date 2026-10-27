import { screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { NODE_A } from "./fixtures";
import { PluginMetrics } from "./plugin-metrics";
import { metricsInterval } from "./plugin-model";

test("metrics_refresh_only_while_visible", () => {
  expect(metricsInterval(true)).toBe(30_000);
  expect(metricsInterval(false)).toBe(false);
});

test("hook_table_shows_this_node", async () => {
  const hour = new Date();
  stubRoutes({
    "/api/v1/plugins/seo/metrics": () =>
      json({
        window_secs: 86_400,
        failures: [{ hook: "render.head", bucket_start: hour.toISOString(), count: 2 }],
        node: {
          id: NODE_A,
          since: "2026-10-27T08:00:00Z",
          hooks: [
            {
              hook: "render.head",
              calls: 100,
              failures: 2,
              busy: 1,
              budget_cut: 3,
              total_micros: 250_000,
              max_micros: 812_400,
              buckets: [50, 40, 5, 4, 1, 0],
            },
          ],
        },
      }),
  });
  renderWithRouter(<PluginMetrics name="seo" />);
  const scope = await screen.findByText(/^This node, since /);
  expect(scope).toBeVisible();
  const table = screen.getByRole("table");
  const row = within(table).getByRole("row", { name: /render\.head/ });
  const cell = (col: string) => row.querySelector(`[data-col=${col}]`)?.textContent;
  expect(cell("calls")).toBe("100");
  expect(cell("failures")).toBe("2");
  expect(cell("busy")).toBe("1");
  expect(cell("budget-cut")).toBe("3");
  expect(cell("average")).toBe("2.5 ms");
  expect(cell("p95")).toBe("≤ 25 ms");
  expect(cell("max")).toBe("812.4 ms");
  // The failures chart's totals, per hook (every node's).
  expect(screen.getByText("All nodes")).toBeVisible();
  expect(screen.getByText("2 failures in the last 24 hours.")).toBeInTheDocument();
});
