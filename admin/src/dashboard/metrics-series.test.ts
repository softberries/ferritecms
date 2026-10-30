import { describe, expect, it } from "vitest";
import { seriesWindow } from "@/api/system";
import type { SchemaSeriesBucketDto } from "@/api/schema";
import { dayTicks, p95Parts, points, summarise } from "./metrics-series";

const HOUR = 3_600_000;

function bucket(
  start: string,
  counts: Partial<Omit<SchemaSeriesBucketDto, "start">> = {},
): SchemaSeriesBucketDto {
  return {
    start,
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
    ...counts,
  };
}

describe("metrics-series", () => {
  it("windows_end_after_the_current_hour", () => {
    const now = new Date("2026-10-30T14:25:10.500Z");
    const day = seriesWindow("24h", now);
    expect(day).toEqual({
      from: "2026-10-29T15:00:00.000Z",
      to: "2026-10-30T15:00:00.000Z",
      step: "hour",
    });
    const week = seriesWindow("7d", now);
    expect(week.step).toBe("hour");
    expect(week.to).toBe("2026-10-30T15:00:00.000Z");
    // Exactly 7 days: the API's limit for an hour series.
    expect(Date.parse(week.to) - Date.parse(week.from)).toBe(168 * HOUR);
    // On the hour: the hour that just started is the last bucket.
    const sharp = seriesWindow("24h", new Date("2026-10-30T14:00:00Z"));
    expect(sharp.to).toBe("2026-10-30T15:00:00.000Z");
  });

  it("sums_counts_and_keeps_the_slowest_p95", () => {
    const totals = summarise([
      bucket("2026-10-30T12:00:00Z", {
        requests: 10,
        page_views: 6,
        errors_5xx: 1,
        cache_hits: 3,
        cache_misses: 1,
        p95_ms: 25,
      }),
      bucket("2026-10-30T13:00:00Z", {
        requests: 5,
        page_views: 2,
        cache_hits: 1,
        cache_misses: 3,
        p95_ms: 100,
      }),
      bucket("2026-10-30T14:00:00Z"),
    ]);
    expect(totals).toEqual({
      requests: 15,
      pageViews: 8,
      errors: 1,
      hits: 4,
      misses: 4,
      p95: 100,
    });
    expect(summarise([bucket("2026-10-30T14:00:00Z")]).p95).toBeNull();
  });

  it("points_leave_gaps_where_nothing_was_measured", () => {
    const rows = points([
      bucket("2026-10-30T12:00:00Z", { cache_hits: 3, cache_misses: 1, p95_ms: 50 }),
      bucket("2026-10-30T13:00:00Z", { requests: 2 }),
    ]);
    expect(rows[0]).toMatchObject({
      t: Date.parse("2026-10-30T12:00:00Z"),
      hitRatio: 75,
      p95: 50,
    });
    // No lookups and no page responses: no ratio and no p95 (a gap, not a zero).
    expect(rows[1]).toMatchObject({ requests: 2, hitRatio: null, p95: null });
  });

  it("day_ticks_fall_on_local_midnight", () => {
    const start = new Date(2026, 9, 28, 22).getTime();
    const rows = points(
      Array.from({ length: 30 }, (_, i) => bucket(new Date(start + i * HOUR).toISOString())),
    );
    const ticks = dayTicks(rows);
    expect(ticks).toEqual([new Date(2026, 9, 29).getTime(), new Date(2026, 9, 30).getTime()]);
  });

  it("p95_bounds_read_as_upper_limits", () => {
    expect(p95Parts(25)).toEqual({ kind: "le", value: 25, unit: "ms" });
    expect(p95Parts(1000)).toEqual({ kind: "le", value: 1000, unit: "ms" });
    expect(p95Parts(2500)).toEqual({ kind: "le", value: 2.5, unit: "s" });
    // The last bound also holds everything slower than 5 s.
    expect(p95Parts(10_000)).toEqual({ kind: "over", value: 5, unit: "s" });
  });
});
