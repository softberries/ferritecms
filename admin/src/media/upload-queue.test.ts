import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { MediaDto } from "@/api/media";
import { ApiError } from "@/api/problem";
import { UploadNetworkError, type UploadOptions } from "@/api/upload";
import { BUSY_RETRY_DEFAULT_MS, createUploadQueue, type UploadEntry } from "./upload-queue";

/** One call of the fake `uploadMedia`, settled by the test. */
interface Call {
  file: File;
  opts: UploadOptions;
  resolve: (m: MediaDto) => void;
  reject: (e: unknown) => void;
}

function fakeUpload() {
  const calls: Call[] = [];
  const upload = vi.fn(
    (file: File, opts: UploadOptions) =>
      new Promise<MediaDto>((resolve, reject) => {
        calls.push({ file, opts, resolve, reject });
        opts.signal.addEventListener("abort", () => {
          reject(new DOMException("aborted", "AbortError"));
        });
      }),
  );
  return { upload, calls };
}

const file = (name: string) => new File(["x".repeat(10)], name, { type: "image/png" });
const media = (id: string): MediaDto =>
  ({ id, title: id, alt: "", mime: "image/png", variants: [] }) as unknown as MediaDto;
const problem = (status: number, slug: string, retryAfter: number | null = null) =>
  new ApiError(
    { type: `https://ferritecms.dev/problems/${slug}`, title: slug, status, detail: `${slug}!` },
    retryAfter,
  );
/** Lets pending promise callbacks run. */
const flush = () => vi.advanceTimersByTimeAsync(0);

let keys = 0;
beforeEach(() => {
  vi.useFakeTimers();
  keys = 0;
  vi.spyOn(crypto, "randomUUID").mockImplementation(
    () =>
      `00000000-0000-4000-8000-${String(++keys).padStart(12, "0")}` as `${string}-${string}-${string}-${string}-${string}`,
  );
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const states = (entries: readonly UploadEntry[]) => entries.map((e) => e.state);

describe("upload_queue_test", () => {
  test("concurrency_3", async () => {
    const { upload, calls } = fakeUpload();
    const done = vi.fn();
    const q = createUploadQueue({ upload, concurrency: 3, onDone: done });
    q.add([file("a.png"), file("b.png"), file("c.png"), file("d.png"), file("e.png")]);
    await flush();
    expect(calls).toHaveLength(3);
    expect(states(q.entries())).toEqual([
      "uploading",
      "uploading",
      "uploading",
      "queued",
      "queued",
    ]);
    calls[1]?.resolve(media("m-b"));
    await flush();
    expect(calls).toHaveLength(4);
    expect(calls[3]?.file.name).toBe("d.png");
    expect(done).toHaveBeenCalledWith(media("m-b"));
    expect(q.entries()[1]?.state).toBe("done");
    expect(q.entries()[1]?.media?.id).toBe("m-b");
  });

  test("progress_is_recorded", async () => {
    const { upload, calls } = fakeUpload();
    const q = createUploadQueue({ upload, concurrency: 3, onDone: () => {} });
    const seen = vi.fn();
    q.subscribe(seen);
    q.add([file("a.png")]);
    await flush();
    calls[0]?.opts.onProgress(5, 10);
    expect(q.entries()[0]).toMatchObject({ loaded: 5, total: 10, state: "uploading" });
    expect(seen).toHaveBeenCalled();
  });

  test("retry_reuses_idempotency_key", async () => {
    const { upload, calls } = fakeUpload();
    const q = createUploadQueue({ upload, concurrency: 3, onDone: () => {} });
    q.add([file("a.png")]);
    await flush();
    const key = calls[0]?.opts.key;
    expect(key).toMatch(/^00000000-/);
    // A 422 is not retried by itself: it waits for Retry.
    calls[0]?.reject(problem(422, "validation"));
    await flush();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(calls).toHaveLength(1);
    const entry = q.entries()[0];
    expect(entry?.state).toBe("failed");
    expect(entry?.error).toBeInstanceOf(ApiError);
    q.retry(entry?.id ?? "");
    await flush();
    expect(calls).toHaveLength(2);
    expect(calls[1]?.opts.key).toBe(key);
    expect(q.entries()[0]?.key).toBe(key);

    // A network failure retries by itself once, after the default delay, same key.
    calls[1]?.reject(new UploadNetworkError());
    await flush();
    expect(q.entries()[0]?.state).toBe("queued");
    await vi.advanceTimersByTimeAsync(BUSY_RETRY_DEFAULT_MS);
    expect(calls).toHaveLength(3);
    expect(calls[2]?.opts.key).toBe(key);
    // Only once: the second network failure waits for Retry.
    calls[2]?.reject(new UploadNetworkError());
    await flush();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(calls).toHaveLength(3);
    expect(q.entries()[0]?.state).toBe("failed");
    expect(q.entries()[0]?.error).toBeInstanceOf(UploadNetworkError);
  });

  test("busy_waits_retry_after", async () => {
    const { upload, calls } = fakeUpload();
    const q = createUploadQueue({ upload, concurrency: 3, onDone: () => {} });
    q.add([file("a.png")]);
    await flush();
    calls[0]?.reject(problem(503, "busy", 5));
    await flush();
    expect(q.entries()[0]?.state).toBe("queued");
    await vi.advanceTimersByTimeAsync(4_999);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(calls).toHaveLength(2);
    expect(calls[1]?.opts.key).toBe(calls[0]?.opts.key);
    calls[1]?.resolve(media("m-1"));
    await flush();
    expect(q.entries()[0]?.state).toBe("done");

    // Without `Retry-After`: 2 s.
    q.add([file("b.png")]);
    await flush();
    calls[2]?.reject(problem(503, "busy"));
    await flush();
    await vi.advanceTimersByTimeAsync(BUSY_RETRY_DEFAULT_MS - 1);
    expect(calls).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(1);
    expect(calls).toHaveLength(4);
    expect(BUSY_RETRY_DEFAULT_MS).toBe(2_000);
  });

  test("a waiting retry frees its slot", async () => {
    const { upload, calls } = fakeUpload();
    const q = createUploadQueue({ upload, concurrency: 3, onDone: () => {} });
    q.add([file("a.png"), file("b.png"), file("c.png"), file("d.png")]);
    await flush();
    calls[0]?.reject(problem(503, "busy", 3));
    await flush();
    // d starts while a waits.
    expect(calls).toHaveLength(4);
    expect(calls[3]?.file.name).toBe("d.png");
  });

  test("cancel_aborts", async () => {
    const { upload, calls } = fakeUpload();
    const q = createUploadQueue({ upload, concurrency: 1, onDone: () => {} });
    q.add([file("a.png"), file("b.png"), file("c.png")]);
    await flush();
    const [a, b, c] = q.entries();
    q.cancel(a?.id ?? "");
    expect(calls[0]?.opts.signal.aborted).toBe(true);
    await flush();
    expect(q.entries()[0]?.state).toBe("cancelled");
    // The next one starts; a queued one cancelled never starts.
    expect(calls).toHaveLength(2);
    expect(calls[1]?.file.name).toBe("b.png");
    q.cancel(c?.id ?? "");
    calls[1]?.resolve(media("m-b"));
    await flush();
    expect(calls).toHaveLength(2);
    expect(states(q.entries())).toEqual(["cancelled", "done", "cancelled"]);
    expect(b?.key).not.toBe(a?.key);
    // A waiting retry is cancelled with its timer.
    q.add([file("d.png")]);
    await flush();
    calls[2]?.reject(new UploadNetworkError());
    await flush();
    q.cancel(q.entries()[3]?.id ?? "");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(calls).toHaveLength(3);
    expect(q.entries()[3]?.state).toBe("cancelled");
    // Done and cancelled rows can be cleared.
    q.clearDone();
    expect(q.entries()).toEqual([]);
  });

  test("active", async () => {
    const { upload, calls } = fakeUpload();
    const q = createUploadQueue({ upload, concurrency: 3, onDone: () => {} });
    expect(q.active()).toBe(false);
    q.add([file("a.png")]);
    expect(q.active()).toBe(true);
    await flush();
    calls[0]?.reject(problem(415, "unsupported-media-type"));
    await flush();
    // A failed row is not running.
    expect(q.active()).toBe(false);
    q.retry(q.entries()[0]?.id ?? "");
    expect(q.active()).toBe(true);
    await flush();
    calls[1]?.resolve(media("m"));
    await flush();
    expect(q.active()).toBe(false);
  });

  test("dispose cancels everything", async () => {
    const { upload, calls } = fakeUpload();
    const q = createUploadQueue({ upload, concurrency: 3, onDone: () => {} });
    q.add([file("a.png"), file("b.png")]);
    await flush();
    q.dispose();
    expect(calls.every((c) => c.opts.signal.aborted)).toBe(true);
    await flush();
    expect(states(q.entries())).toEqual(["cancelled", "cancelled"]);
    expect(q.active()).toBe(false);
  });
});
