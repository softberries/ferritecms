import { describe, expect, it } from "vitest";
import { FAILURE_HOURS, hourlyFailures } from "./failures";

const now = new Date("2026-10-25T14:20:00Z");
const bucket = (hook: string, iso: string, count: number) => ({
  hook,
  bucket_start: iso,
  count,
});

describe("hourlyFailures", () => {
  it("sums hooks per plugin and hour over the last 24 hours", () => {
    const result = hourlyFailures(
      [
        {
          name: "seo",
          failures: [
            bucket("render", "2026-10-25T14:00:00Z", 2),
            bucket("save", "2026-10-25T14:00:00Z", 1),
            bucket("render", "2026-10-24T15:00:00Z", 4), // the oldest hour in the window
            bucket("render", "2026-10-24T14:00:00Z", 9), // just outside it
          ],
        },
        { name: "quiet", failures: [] },
      ],
      now,
    );
    expect(result.rows).toHaveLength(FAILURE_HOURS);
    expect(result.rows[0]?.hour).toBe(Date.parse("2026-10-24T15:00:00Z"));
    expect(result.rows.at(-1)?.hour).toBe(Date.parse("2026-10-25T14:00:00Z"));
    expect(result.rows[0]?.s0).toBe(4);
    expect(result.rows.at(-1)?.s0).toBe(3);
    expect(result.series).toEqual([{ key: "s0", plugin: "seo", total: 7 }]);
    expect(result.total).toBe(7);
  });

  it("keeps four plugins and folds the rest into other", () => {
    const plugins = ["a", "b", "c", "d", "e", "f"].map((name, i) => ({
      name,
      failures: [bucket("h", "2026-10-25T13:00:00Z", 10 - i)],
    }));
    const result = hourlyFailures(plugins, now);
    expect(result.series.map((s) => s.plugin)).toEqual(["a", "b", "c", "d", null]);
    expect(result.series.at(-1)?.total).toBe(6 + 5);
    expect(result.rows.at(-2)?.s4).toBe(11);
    expect(result.total).toBe(10 + 9 + 8 + 7 + 6 + 5);
  });

  it("has no series when nothing failed", () => {
    const result = hourlyFailures([{ name: "ok", failures: [] }], now);
    expect(result.series).toEqual([]);
    expect(result.total).toBe(0);
    expect(result.rows).toHaveLength(FAILURE_HOURS);
  });
});
