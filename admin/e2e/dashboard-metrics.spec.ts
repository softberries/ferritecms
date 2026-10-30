// The dashboard's Site traffic cards under the real server (M4 P36): anonymous page hits
// reach `node_metrics` with the node's next whole-minute row (the release binary has no
// test seam for the writer's tick, so the test waits up to a minute and a half), and the
// Traffic card shows the series API's numbers, every node's ("All nodes"). The states a
// live site cannot produce on demand (empty, failed, a failed refresh) are component
// tests (`site-metrics.test.tsx`).

import {
  request as apiRequest,
  type APIRequestContext,
  type Page,
  type Route,
} from "@playwright/test";
import { expect, test } from "./support/fixtures";

const HOUR = 3_600_000;
const HITS = 5;

/** The dashboard's 24-hour window (`seriesWindow`): whole hours ending after this one. */
function window24h(now = Date.now()) {
  const to = Math.floor(now / HOUR) * HOUR + HOUR;
  return { from: new Date(to - 24 * HOUR).toISOString(), to: new Date(to).toISOString() };
}

/** Page views over the last 24 hours, every node's, from the series API. */
async function pageViews(request: APIRequestContext): Promise<number> {
  const { from, to } = window24h();
  const res = await request.get("/api/v1/system/metrics/series", {
    params: { from, to, step: "hour" },
  });
  expect(res.status(), await res.text()).toBe(200);
  const body = (await res.json()) as { buckets: { page_views: number }[] };
  expect(body.buckets).toHaveLength(24);
  return body.buckets.reduce((n, b) => n + b.page_views, 0);
}

/** The number a Traffic total shows (its label and value are one list item). */
async function shownTotal(page: Page, label: string): Promise<number> {
  const totals = page
    .getByRole("region", { name: "Traffic" })
    .getByRole("list", { name: "Totals" });
  const text = await totals.getByRole("listitem").filter({ hasText: label }).textContent();
  const value = /([\d,]+)/.exec((text ?? "").replace(label, ""))?.[1] ?? "";
  return Number(value.replace(/,/g, ""));
}

test("traffic_card_shows_page_hits", async ({ as, site, snap, expectAccessible }) => {
  test.setTimeout(180_000);
  const page = await as("admin");
  const before = await pageViews(page.request);

  // Anonymous reads of the home page: page views (not a bot, not signed in).
  const anon = await apiRequest.newContext({
    baseURL: site.url,
    userAgent: "Mozilla/5.0 (X11; Linux x86_64) FerriteE2E/1.0",
  });
  for (let i = 0; i < HITS; i++) expect((await anon.get("/")).status()).toBe(200);
  await anon.dispose();

  // The node writes the minute's row at the next whole minute; other specs only add.
  await expect
    .poll(() => pageViews(page.request), { timeout: 90_000, intervals: [2_000] })
    .toBeGreaterThanOrEqual(before + HITS);

  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
  const section = page.getByRole("region", { name: "Site traffic" });
  const traffic = section.getByRole("region", { name: "Traffic" });
  const performance = section.getByRole("region", { name: "Performance" });
  await expect(traffic).not.toHaveAttribute("aria-busy", "true");
  // Cluster-wide figures: both cards say so.
  await expect(traffic.getByText("All nodes")).toBeVisible();
  await expect(performance.getByText("All nodes")).toBeVisible();
  await expect(traffic.getByTestId("traffic-chart").locator(".recharts-area")).toHaveCount(2);

  // The card shows what the API answers; another spec's hits may land between two reads,
  // so a reload and a fresh read converge once no minute row is written in between.
  await expect
    .poll(
      async () => {
        await page.reload();
        await expect(traffic.getByRole("list", { name: "Totals" })).toBeVisible();
        const shown = await shownTotal(page, "Page views");
        const api = await pageViews(page.request);
        return shown === api ? "equal" : JSON.stringify({ shown, api });
      },
      { timeout: 60_000 },
    )
    .toBe("equal");
  expect(await shownTotal(page, "Page views")).toBeGreaterThanOrEqual(before + HITS);

  // The home page reads went through the page cache: a ratio and a p95 bound.
  await expect(performance.getByTestId("hit-ratio")).toContainText(/\d+%/);
  await expect(performance.getByTestId("page-p95")).toContainText(/[≤>] [\d.,]+ m?s/);
  await snap(page, "dashboard-metrics");
  await expectAccessible(page);

  // Seven days: the range switch asks for hourly buckets over exactly 168 hours.
  const day = await shownTotal(page, "Page views");
  const asked = page.waitForRequest((r) => r.url().includes("/api/v1/system/metrics/series"));
  await section.getByRole("radio", { name: "7 days" }).click();
  const url = new URL((await asked).url());
  expect(url.searchParams.get("step")).toBe("hour");
  expect(
    Date.parse(url.searchParams.get("to") ?? "") - Date.parse(url.searchParams.get("from") ?? ""),
  ).toBe(168 * HOUR);
  await expect(
    traffic.getByText(
      "Page views, and all requests including assets, the API and the admin, per hour, the last 7 days",
    ),
  ).toBeVisible();
  await expect(traffic).not.toHaveAttribute("aria-busy", "true");
  expect(await shownTotal(page, "Page views")).toBeGreaterThanOrEqual(day);
  await expect(section.getByRole("radio", { name: "7 days" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
});

/** A series answer for the requested window, `fill(start, i)` giving each bucket's counts. */
function fulfilSeries(fill: (start: Date, i: number) => Record<string, number>) {
  return (route: Route) => {
    const url = new URL(route.request().url());
    const from = Date.parse(url.searchParams.get("from") ?? "");
    const to = Date.parse(url.searchParams.get("to") ?? "");
    const zero = {
      requests: 0,
      errors_5xx: 0,
      page_views: 0,
      cache_hits: 0,
      cache_misses: 0,
      plugin_calls: 0,
      plugin_failures: 0,
      jobs_done: 0,
      jobs_failed: 0,
      p95_ms: 0,
    };
    const buckets = Array.from({ length: (to - from) / HOUR }, (_, i) => {
      const start = new Date(from + i * HOUR);
      return { start: start.toISOString(), ...zero, ...fill(start, i) };
    });
    return route.fulfill({
      json: {
        from: new Date(from).toISOString(),
        to: new Date(to).toISOString(),
        step: "hour",
        node: null,
        buckets,
      },
    });
  };
}

test("screens", async ({ as, snap, expectAccessible }) => {
  // For review only (the states are component-tested): an empty site, then a week of a
  // busy one with a daily rhythm, slow evenings and a few bursts of 5xx errors.
  const page = await as("admin");
  const section = page.getByRole("region", { name: "Site traffic" });
  await page.route(
    "**/api/v1/system/metrics/series?*",
    fulfilSeries(() => ({})),
  );
  await page.goto("/admin/");
  await expect(
    section.getByRole("heading", { name: "No traffic in the last 24 hours" }),
  ).toBeVisible();
  await expect(
    section.getByRole("heading", { name: "No performance data in the last 24 hours" }),
  ).toBeVisible();
  await snap(page, "dashboard-metrics-empty");
  await expectAccessible(page);
  await page.unrouteAll();

  await page.route(
    "**/api/v1/system/metrics/series?*",
    fulfilSeries((start, i) => {
      const h = start.getHours();
      const views = Math.round(
        40 + 160 * Math.max(0, Math.sin(((h - 6) / 24) * 2 * Math.PI)) + (i % 7) * 3,
      );
      return {
        page_views: views,
        requests: Math.round(views * 1.6 + 25),
        errors_5xx: i % 53 === 7 ? 6 : i % 41 === 3 ? 2 : 0,
        cache_hits: Math.round(views * 0.82),
        cache_misses: Math.round(views * 0.18) + 1,
        p95_ms: h >= 19 && h <= 21 ? 250 : h % 5 === 0 ? 100 : 50,
      };
    }),
  );
  await page.goto("/admin/");
  const traffic = section.getByRole("region", { name: "Traffic" });
  await expect(traffic.getByTestId("errors-chart")).toBeVisible();
  await snap(page, "dashboard-metrics-day");
  await section.getByRole("radio", { name: "7 days" }).click();
  await expect(
    traffic.getByText(
      "Page views, and all requests including assets, the API and the admin, per hour, the last 7 days",
    ),
  ).toBeVisible();
  await expect(traffic).not.toHaveAttribute("aria-busy", "true");
  await expect(traffic.getByTestId("errors-chart")).toBeVisible();
  // After the click: no focused control and the page at its top for the full-page snap.
  await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
    window.scrollTo(0, 0);
  });
  await snap(page, "dashboard-metrics-week");
  await expectAccessible(page);
  await page.unrouteAll();
});
