import { afterEach, describe, expect, it, vi } from "vitest";
import { seriesWindow } from "@/api/system";
import type { SchemaSeriesBucketDto } from "@/api/schema";
import {
  bucketTooltipLabel,
  byLocalDay,
  dayTicks,
  durationParts,
  hitRatioText,
  hourLabel,
  p95Parts,
  points,
  summarise,
  tickFormat,
} from "./metrics-series";

const HOUR = 3_600_000;

/** ICU's thin and narrow no-break spaces as plain spaces. */
function norm(text: string): string {
  return text.replace(/[\u2009\u202f\u00a0]/g, " ");
}

/** Runs the rest of the test in time zone `tz` (Node applies `process.env.TZ` at runtime). */
function inZone(tz: string): void {
  vi.stubEnv("TZ", tz);
}

afterEach(() => {
  vi.unstubAllEnvs();
});

/** `n` hourly points from `from` (an instant), every count 1. */
function hourly(from: string, n: number) {
  const start = Date.parse(from);
  return points(
    Array.from({ length: n }, (_, i) =>
      bucket(new Date(start + i * HOUR).toISOString(), { requests: 1, page_views: 1 }),
    ),
  );
}

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
    page_latency: Array<number>(12).fill(0),
    ...counts,
  };
}

/** A page latency histogram: `pages` responses in each `[slot, pages]`. */
function latency(...slots: Array<[number, number]>): number[] {
  const h = Array<number>(12).fill(0);
  for (const [slot, pages] of slots) h[slot] = (h[slot] ?? 0) + pages;
  return h;
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

  it("sums_counts_and_takes_the_p95_of_the_summed_histogram", () => {
    const totals = summarise([
      bucket("2026-10-30T12:00:00Z", {
        requests: 10,
        page_views: 6,
        errors_5xx: 1,
        cache_hits: 3,
        cache_misses: 1,
        p95_ms: 5,
        page_latency: latency([0, 19]),
      }),
      // This hour alone is slow (1 of its 5 responses took up to a second)...
      bucket("2026-10-30T13:00:00Z", {
        requests: 5,
        page_views: 2,
        cache_hits: 1,
        cache_misses: 3,
        p95_ms: 1000,
        page_latency: latency([7, 1]),
      }),
      bucket("2026-10-30T14:00:00Z"),
    ]);
    // ...but over the range 19 of 20 took at most 5 ms: the p95 is not the slowest hour's.
    expect(totals).toEqual({
      requests: 15,
      pageViews: 8,
      errors: 1,
      hits: 4,
      misses: 4,
      p95: 5,
    });
    expect(summarise([bucket("2026-10-30T14:00:00Z")]).p95).toBeNull();
    // Two slow responses of 20: the p95 is slow.
    const slow = summarise([
      bucket("2026-10-30T12:00:00Z", { page_latency: latency([0, 18]) }),
      bucket("2026-10-30T13:00:00Z", { page_latency: latency([7, 2]) }),
    ]);
    expect(slow.p95).toBe(1000);
    // Anything over 10 s reads as the largest bound.
    expect(
      summarise([bucket("2026-10-30T12:00:00Z", { page_latency: latency([11, 1]) })]).p95,
    ).toBe(10_000);
  });

  it("points_leave_gaps_where_nothing_was_measured", () => {
    const rows = points([
      bucket("2026-10-30T12:00:00Z", {
        cache_hits: 3,
        cache_misses: 1,
        p95_ms: 50,
        page_latency: latency([3, 1]),
      }),
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

  it("axis_durations_are_positions_not_bounds", () => {
    // 5 s and 10 s read differently on the axis (both are "> 5 s" as a p95 bound).
    expect(durationParts(5_000)).toEqual({ value: 5, unit: "s" });
    expect(durationParts(10_000)).toEqual({ value: 10, unit: "s" });
    expect(durationParts(250)).toEqual({ value: 250, unit: "ms" });
  });

  it("half_hour_zone_ticks_fall_on_the_bucket_holding_midnight", () => {
    inZone("Asia/Kolkata");
    // 168 hourly buckets from 2026-10-23T09:00Z (14:30 IST): no bucket starts at a
    // local midnight; the 00:30 buckets carry the day ticks.
    const rows = hourly("2026-10-23T09:00:00Z", 168);
    const ticks = dayTicks(rows);
    expect(ticks).toHaveLength(7);
    for (const t of ticks) {
      const d = new Date(t);
      expect([d.getHours(), d.getMinutes()]).toEqual([0, 30]);
    }
    expect(tickFormat("7d")(ticks[0]!)).toBe("Sat");
  });

  it("half_hour_zone_hours_name_their_minutes", () => {
    inZone("Asia/Kolkata");
    const t = Date.parse("2026-10-30T09:00:00Z"); // 14:30 IST
    expect(norm(tickFormat("24h")(t))).toBe("2:30 PM");
    expect(norm(hourLabel(t))).toBe("Fri, Oct 30, 2:30 – 3:30 PM");
    // A whole-hour zone keeps the short form.
    inZone("UTC");
    expect(norm(tickFormat("24h")(t))).toBe("9 AM");
    expect(norm(hourLabel(t))).toBe("Fri, Oct 30, 9 – 10 AM");
  });

  it("dst_change_keeps_seven_midnights_and_whole_days", () => {
    inZone("Europe/Warsaw");
    // 2026-10-25 03:00 CEST becomes 02:00 CET: that day has 25 hours.
    const rows = hourly("2026-10-21T22:00:00Z", 168); // Thu 00:00 CEST
    const ticks = dayTicks(rows);
    expect(ticks).toHaveLength(6);
    for (const t of ticks) expect(new Date(t).getHours()).toBe(0);
    const days = byLocalDay(rows, Date.parse("2026-11-30T00:00:00Z"));
    const sunday = days.find((d) => new Date(d.day).getDate() === 25);
    expect(sunday).toMatchObject({ requests: 25, partial: false });
    expect(days[0]).toMatchObject({ requests: 24, partial: false });
    // 168 hours from a midnight with one 25-hour day: the last day is 23 hours, partial.
    expect(days.at(-1)).toMatchObject({ requests: 23, partial: true });
  });

  it("partial_days_and_the_hour_in_progress", () => {
    inZone("Asia/Kolkata");
    const rows = hourly("2026-10-23T09:00:00Z", 168);
    const now = Date.parse("2026-10-30T08:20:00Z"); // inside the last bucket
    const days = byLocalDay(rows, now);
    // 14:30 on the first day to 14:30 on the last: both ends partial, the middle whole;
    // the 23:30 bucket counts for the day it starts on.
    expect(days).toHaveLength(8);
    expect(days[0]?.partial).toBe(true);
    expect(days.at(-1)?.partial).toBe(true);
    for (const d of days.slice(1, -1)) expect(d).toMatchObject({ partial: false, requests: 24 });
    expect(days.reduce((n, d) => n + d.requests, 0)).toBe(168);
  });

  it("tooltip_marks_the_hour_in_progress", () => {
    inZone("UTC");
    const label = bucketTooltipLabel("so far");
    const done = Date.parse("2020-01-01T10:00:00Z");
    expect(norm(label(null, [{ payload: { t: done } }]) ?? "")).toBe("Wed, Jan 1, 10 – 11 AM");
    const current = Math.floor(Date.now() / HOUR) * HOUR;
    expect(label(null, [{ payload: { t: current } }])).toMatch(/ · so far$/);
    expect(label(null, [])).toBeNull();
  });

  it("hit_ratio_never_rounds_up_to_100", () => {
    expect(hitRatioText(0.75)).toBe("75%");
    expect(hitRatioText(0.989)).toBe("99%");
    expect(hitRatioText(0.996)).toBe("99.6%");
    expect(hitRatioText(0.99999)).toBe("99.9%");
    expect(hitRatioText(1)).toBe("100%");
  });
});
