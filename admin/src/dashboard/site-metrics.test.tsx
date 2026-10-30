import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SchemaSeriesBucketDto } from "@/api/schema";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { SiteMetrics } from "./site-metrics";

const HOUR = 3_600_000;
const NOW = new Date("2026-10-30T14:25:00Z");
const SERIES = "/api/v1/system/metrics/series";

/** `hours` hourly buckets ending with the current hour, `fill(i)` giving the counts. */
function series(
  url: URL,
  fill: (i: number) => Partial<Omit<SchemaSeriesBucketDto, "start">> = () => ({}),
) {
  const from = Date.parse(url.searchParams.get("from") ?? "");
  const to = Date.parse(url.searchParams.get("to") ?? "");
  const buckets = Array.from({ length: (to - from) / HOUR }, (_, i) => ({
    start: new Date(from + i * HOUR).toISOString(),
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
    page_latency: Array<number>(12).fill(0),
    ...fill(i),
  }));
  return json({
    from: new Date(from).toISOString(),
    to: new Date(to).toISOString(),
    step: "hour",
    node: null,
    buckets,
  });
}

/** A page latency histogram with `pages` responses in `index`. */
function slot(index: number, pages: number): number[] {
  const h = Array<number>(12).fill(0);
  h[index] = pages;
  return h;
}

/** The last two hours had traffic: 1,200 + 34 page views, a 5xx, 75 % cache hits. */
function busy(url: URL) {
  return series(url, (i) => {
    const hours =
      (Date.parse(url.searchParams.get("to") ?? "") -
        Date.parse(url.searchParams.get("from") ?? "")) /
      HOUR;
    if (i === hours - 2)
      return {
        requests: 1500,
        page_views: 1200,
        errors_5xx: 3,
        cache_hits: 900,
        cache_misses: 300,
        p95_ms: 250,
        page_latency: slot(5, 1200),
      };
    if (i === hours - 1)
      return {
        requests: 50,
        page_views: 34,
        cache_hits: 0,
        cache_misses: 0,
        p95_ms: 25,
        page_latency: slot(2, 34),
      };
    return {};
  });
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("site-metrics", () => {
  it("loading_marks_both_cards_busy", async () => {
    stubRoutes({ [SERIES]: () => new Promise<Response>(() => {}) });
    renderWithRouter(<SiteMetrics />);
    const traffic = await screen.findByRole("region", { name: "Traffic" });
    const performance = screen.getByRole("region", { name: "Performance" });
    expect(traffic).toHaveAttribute("aria-busy", "true");
    expect(performance).toHaveAttribute("aria-busy", "true");
    expect(traffic.querySelector("[data-slot=chart]")).toBeNull();
  });

  it("all_zero_series_is_an_empty_state", async () => {
    stubRoutes({ [SERIES]: (url) => series(url) });
    renderWithRouter(<SiteMetrics />);
    const traffic = await screen.findByRole("region", { name: "Traffic" });
    expect(
      await within(traffic).findByRole("heading", { name: "No traffic in the last 24 hours" }),
    ).toBeVisible();
    expect(traffic.querySelector("[data-slot=chart]")).toBeNull();
    expect(within(traffic).queryByRole("table")).toBeNull();
    const performance = screen.getByRole("region", { name: "Performance" });
    expect(
      within(performance).getByRole("heading", {
        name: "No performance data in the last 24 hours",
      }),
    ).toBeVisible();
    expect(traffic).not.toHaveAttribute("aria-busy");
  });

  it("shows_totals_charts_and_cluster_scope", async () => {
    const seen = stubRoutes({ [SERIES]: busy });
    renderWithRouter(<SiteMetrics />);
    const traffic = await screen.findByRole("region", { name: "Traffic" });
    const totals = await within(traffic).findByRole("list", { name: "Totals" });
    const item = (name: string) =>
      within(totals)
        .getAllByRole("listitem")
        .find((li) => li.textContent?.includes(name));
    expect(item("Page views")).toHaveTextContent("1,234");
    expect(item("All requests")).toHaveTextContent("1,550");
    expect(item("5xx errors")).toHaveTextContent("3");
    expect(item("5xx errors")).toHaveTextContent("0.2% of all requests");
    expect(within(traffic).getByText("All nodes")).toBeVisible();
    expect(within(traffic).getByTestId("traffic-chart")).toBeInTheDocument();
    expect(within(traffic).getByTestId("errors-chart")).toBeInTheDocument();
    // The text twin: one row per hour.
    const table = within(traffic).getByRole("table", { hidden: true });
    const tableRows = within(table).getAllByRole("row", { hidden: true });
    expect(tableRows).toHaveLength(25);
    // Only the current hour is marked as still in progress; the caption names all requests.
    expect(tableRows.at(-1)).toHaveTextContent("(in progress)");
    expect(tableRows.at(-2)).not.toHaveTextContent("(in progress)");
    expect(table).toHaveTextContent("including assets, the API and the admin");

    const performance = screen.getByRole("region", { name: "Performance" });
    expect(within(performance).getByText("All nodes")).toBeVisible();
    const ratio = within(performance).getByTestId("hit-ratio");
    expect(ratio).toHaveTextContent("75%");
    expect(ratio).toHaveTextContent("900 of 1,200 lookups");
    const p95 = within(performance).getByTestId("page-p95");
    expect(p95).toHaveTextContent("≤ 250 ms");
    expect(p95).toHaveTextContent("95th percentile of the page responses in the last 24 hours");

    // One request, summed over nodes (no `node`), hourly over exactly 24 hours.
    expect(seen).toHaveLength(1);
    const url = seen[0]!;
    expect(url.searchParams.get("step")).toBe("hour");
    expect(url.searchParams.has("node")).toBe(false);
    expect(url.searchParams.get("to")).toBe("2026-10-30T15:00:00.000Z");
    expect(url.searchParams.get("from")).toBe("2026-10-29T15:00:00.000Z");
  });

  it("toggle_switches_to_seven_days", async () => {
    const user = userEvent.setup();
    const seen = stubRoutes({ [SERIES]: busy });
    renderWithRouter(<SiteMetrics />);
    const range = await screen.findByRole("radiogroup", { name: "Time range" });
    const day = within(range).getByRole("radio", { name: "24 hours" });
    const week = within(range).getByRole("radio", { name: "7 days" });
    expect(day).toHaveAttribute("aria-checked", "true");
    expect(
      await screen.findByText(
        "Page views, and all requests including assets, the API and the admin, per hour, the last 24 hours",
      ),
    ).toBeVisible();

    await user.click(week);
    expect(week).toHaveAttribute("aria-checked", "true");
    expect(
      await screen.findByText(
        "Page views, and all requests including assets, the API and the admin, per hour, the last 7 days",
      ),
    ).toBeVisible();
    await waitFor(() => expect(seen).toHaveLength(2));
    const url = seen[1]!;
    expect(url.searchParams.get("step")).toBe("hour");
    expect(
      Date.parse(url.searchParams.get("to")!) - Date.parse(url.searchParams.get("from")!),
    ).toBe(168 * HOUR);
    // The 7-day text twin has a row per day, not 168.
    const traffic = screen.getByRole("region", { name: "Traffic" });
    await waitFor(() => {
      const rows = within(within(traffic).getByRole("table", { hidden: true })).getAllByRole(
        "row",
        { hidden: true },
      );
      expect(rows.length).toBeLessThanOrEqual(9);
    });
    // Back to 24 hours: answered from the cache, no third request.
    await user.click(day);
    expect(
      await screen.findByText(
        "Page views, and all requests including assets, the API and the admin, per hour, the last 24 hours",
      ),
    ).toBeVisible();
    expect(seen).toHaveLength(2);
  });

  it("arrow_keys_select_the_range", async () => {
    // The ARIA radio group pattern: Tab reaches the checked radio, an arrow moves and
    // selects.
    const user = userEvent.setup();
    const seen = stubRoutes({ [SERIES]: busy });
    renderWithRouter(<SiteMetrics />);
    const range = await screen.findByRole("radiogroup", { name: "Time range" });
    const day = within(range).getByRole("radio", { name: "24 hours" });
    const week = within(range).getByRole("radio", { name: "7 days" });
    await user.tab();
    expect(day).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(week).toHaveFocus();
    expect(week).toHaveAttribute("aria-checked", "true");
    expect(day).toHaveAttribute("aria-checked", "false");
    await waitFor(() => expect(seen).toHaveLength(2));
    await user.keyboard("{ArrowLeft}");
    expect(day).toHaveFocus();
    expect(day).toHaveAttribute("aria-checked", "true");
  });

  it("a_failed_load_offers_a_retry", async () => {
    const user = userEvent.setup();
    let fail = true;
    stubRoutes({
      [SERIES]: (url) =>
        fail
          ? new Response(
              JSON.stringify({
                type: "https://ferritecms.dev/problems/internal",
                title: "Internal error",
                status: 500,
                detail: "The server could not answer. It has been logged.",
              }),
              { status: 500, headers: { "Content-Type": "application/problem+json" } },
            )
          : busy(url),
    });
    renderWithRouter(<SiteMetrics />);
    const traffic = await screen.findByRole("region", { name: "Traffic" });
    const alert = await within(traffic).findByRole("alert");
    expect(alert).toHaveTextContent("The server could not answer.");
    expect(
      within(screen.getByRole("region", { name: "Performance" })).getByRole("alert"),
    ).toBeVisible();
    fail = false;
    await user.click(within(alert).getByRole("button", { name: "Try again" }));
    expect(await within(traffic).findByRole("list", { name: "Totals" })).toBeVisible();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("a_failed_refresh_keeps_the_figures", async () => {
    let fail = false;
    stubRoutes({
      [SERIES]: (url) =>
        fail
          ? new Response(JSON.stringify({ title: "Internal error", status: 500 }), {
              status: 500,
              headers: { "Content-Type": "application/problem+json" },
            })
          : busy(url),
    });
    const client = renderWithRouter(<SiteMetrics />);
    const traffic = await screen.findByRole("region", { name: "Traffic" });
    expect(await within(traffic).findByRole("list", { name: "Totals" })).toBeVisible();
    fail = true;
    await client.refetchQueries();
    // The chart stays, with a note instead of an error state.
    expect(await within(traffic).findByText(/^Couldn't refresh\./)).toBeVisible();
    expect(within(traffic).getByTestId("traffic-chart")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
