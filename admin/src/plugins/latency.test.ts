import { expect, test } from "vitest";
import { BUCKET_BOUNDS_MS, averageMs, p95Bound } from "./latency";

test("p95_bound", () => {
  // No calls: no value.
  expect(p95Bound([])).toBeNull();
  expect(p95Bound([0, 0, 0, 0, 0, 0])).toBeNull();
  // Every call within 1 ms.
  expect(p95Bound([40, 0, 0, 0, 0, 0])).toEqual({ le_ms: 1 });
  // 95 of 100 calls within 25 ms: the 95th is in the third bucket.
  expect(p95Bound([50, 40, 5, 4, 1, 0])).toEqual({ le_ms: 25 });
  // 94 of 100 within 25 ms: the 95th is in the ≤ 100 ms bucket.
  expect(p95Bound([50, 40, 4, 5, 1, 0])).toEqual({ le_ms: 100 });
  // A tail over 500 ms large enough to hold the 95th call: `null` ("over 500 ms").
  expect(p95Bound([90, 0, 0, 0, 0, 10])).toEqual({ le_ms: null });
  // A tail too small to matter.
  expect(p95Bound([96, 0, 0, 0, 0, 4])).toEqual({ le_ms: 1 });
  // One call over 500 ms.
  expect(p95Bound([0, 0, 0, 0, 0, 1])).toEqual({ le_ms: null });
  expect(BUCKET_BOUNDS_MS).toEqual([1, 5, 25, 100, 500]);
});

test("average_ms", () => {
  expect(averageMs(0, 0)).toBeNull();
  expect(averageMs(4_500, 3)).toBe(1.5);
});
