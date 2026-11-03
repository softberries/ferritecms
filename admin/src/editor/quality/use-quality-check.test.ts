import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { CheckResponse } from "@/api/quality";
import { CHECK_DEBOUNCE_MS, QualityChecker } from "./use-quality-check";

const report = (n: number): CheckResponse => ({
  findings: [],
  counts: { error: n, warning: 0, notice: 0, suppressed: 0 },
  gate: { mode: "off", blocked: false },
});

describe("QualityChecker", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  test("debounces 1.5 s after the last change", async () => {
    const send = vi.fn(() => Promise.resolve(report(1)));
    const onResult = vi.fn();
    const c = new QualityChecker({ draft: () => ({ body: {} }), send, onResult });
    c.changed();
    vi.advanceTimersByTime(CHECK_DEBOUNCE_MS - 1);
    c.changed();
    vi.advanceTimersByTime(CHECK_DEBOUNCE_MS - 1);
    expect(send).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    await vi.waitFor(() => expect(onResult).toHaveBeenCalledTimes(1));
    expect(send).toHaveBeenCalledTimes(1);
    c.dispose();
  });

  test("one request at a time; a stale result is dropped and one more check runs", async () => {
    let release: ((r: CheckResponse) => void) | null = null;
    const send = vi.fn(
      () =>
        new Promise<CheckResponse>((resolve) => {
          release = resolve;
        }),
    );
    const onResult = vi.fn<(r: CheckResponse) => void>();
    const c = new QualityChecker({ draft: () => ({ body: {} }), send, onResult });
    void c.now();
    expect(send).toHaveBeenCalledTimes(1);
    // A change while the first runs: its answer is for an older revision.
    c.changed();
    void c.now();
    expect(send).toHaveBeenCalledTimes(1);
    release!(report(1));
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(2));
    expect(onResult).not.toHaveBeenCalled();
    release!(report(2));
    await vi.waitFor(() => expect(onResult).toHaveBeenCalledTimes(1));
    expect(onResult.mock.calls[0]![0].counts.error).toBe(2);
    c.dispose();
  });

  test("errors are reported, disposed checkers stay quiet", async () => {
    const onError = vi.fn();
    const onResult = vi.fn();
    const c = new QualityChecker({
      draft: () => ({ body: {} }),
      send: () => Promise.reject(new Error("down")),
      onResult,
      onError,
    });
    await c.now();
    expect(onError).toHaveBeenCalledTimes(1);
    c.dispose();
    await c.now();
    expect(onResult).not.toHaveBeenCalled();
  });
});
