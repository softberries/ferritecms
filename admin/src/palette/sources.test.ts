import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { SEARCH_DEBOUNCE_MS, SEARCH_LIMIT, searchContent } from "./sources";

interface Summary {
  type: string;
  id: string;
  title: string;
  status: string;
  slug: string;
}

function summary(type: string, id: string, title: string, status = "published"): Summary {
  return { type, id, title, status, slug: title.toLowerCase().replace(/\s+/g, "-") };
}

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function problem(status: number) {
  return new Response(
    JSON.stringify({ type: "/api/v1/problems/internal", title: "x", status, detail: "" }),
    { status, headers: { "Content-Type": "application/problem+json" } },
  );
}

/** The type a request lists (`/api/v1/content/<type>`). */
const typeOf = (r: Request) => new URL(r.url).pathname.split("/").at(-1) ?? "";

/**
 * Stubs `fetch`: `answer` gives each request's response (or a promise of one). A pending
 * request rejects with an `AbortError` once its signal aborts, as the browser's does.
 */
function stubFetch(answer: (r: Request) => Response | Promise<Response>) {
  const requests: Request[] = [];
  const fetch = vi.fn((r: Request) => {
    requests.push(r);
    return new Promise<Response>((resolve, reject) => {
      const abort = () => reject(new DOMException("The operation was aborted.", "AbortError"));
      if (r.signal.aborted) {
        abort();
        return;
      }
      r.signal.addEventListener("abort", abort);
      void Promise.resolve(answer(r)).then(resolve, reject);
    });
  });
  vi.stubGlobal("fetch", fetch);
  return { requests, fetch };
}

const list = (items: Summary[]) => json({ items, next_cursor: null });

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

test("under_two_characters_skips_the_search", async () => {
  const { fetch } = stubFetch(() => list([]));
  const signal = new AbortController().signal;
  await expect(searchContent("", ["post"], signal)).resolves.toEqual([]);
  await expect(searchContent("a", ["post"], signal)).resolves.toEqual([]);
  // Surrounding blanks do not count.
  await expect(searchContent("  a  ", ["post"], signal)).resolves.toEqual([]);
  await vi.runAllTimersAsync();
  expect(fetch).not.toHaveBeenCalled();
});

test("debounce_waits_150_ms_then_one_request_per_type", async () => {
  const { requests } = stubFetch(() => list([]));
  const done = searchContent("wa", ["post", "page"], new AbortController().signal);
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS - 1);
  expect(requests).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(1);
  await done;
  expect(requests.map(typeOf)).toEqual(["post", "page"]);
  for (const r of requests) {
    const url = new URL(r.url);
    expect(r.method).toBe("GET");
    expect(url.searchParams.get("q")).toBe("wa");
    expect(url.searchParams.get("limit")).toBe(String(SEARCH_LIMIT));
  }
  expect(SEARCH_DEBOUNCE_MS).toBe(150);
  expect(SEARCH_LIMIT).toBe(8);
});

test("typing_fast_sends_only_the_last_round", async () => {
  const { requests } = stubFetch(() => list([]));
  // What the palette does on every keystroke: abort the previous round, start a new one.
  let controller = new AbortController();
  const rounds: Promise<unknown>[] = [];
  for (const q of ["we", "web", "weba"]) {
    controller.abort();
    controller = new AbortController();
    rounds.push(searchContent(q, ["post"], controller.signal).catch((e: unknown) => e));
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS / 2);
  }
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  const results = await Promise.all(rounds);
  expect(requests.map((r) => new URL(r.url).searchParams.get("q"))).toEqual(["weba"]);
  expect(results.slice(0, 2).map((e) => (e as DOMException).name)).toEqual([
    "AbortError",
    "AbortError",
  ]);
  expect(results[2]).toEqual([]);
});

test("abort_cancels_requests_in_flight", async () => {
  // The server never answers; only the abort ends the round.
  const { requests } = stubFetch(() => new Promise<Response>(() => {}));
  const controller = new AbortController();
  const done = searchContent("sandbox", ["post", "page"], controller.signal);
  const outcome = done.catch((e: unknown) => e);
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  expect(requests).toHaveLength(2);
  controller.abort();
  const error = await outcome;
  expect(error).toBeInstanceOf(DOMException);
  expect((error as DOMException).name).toBe("AbortError");
  expect(requests.every((r) => r.signal.aborted)).toBe(true);
});

test("abort_during_the_debounce_sends_nothing", async () => {
  const { fetch } = stubFetch(() => list([]));
  const controller = new AbortController();
  const outcome = searchContent("sandbox", ["post"], controller.signal).catch((e: unknown) => e);
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS - 50);
  controller.abort();
  expect(((await outcome) as DOMException).name).toBe("AbortError");
  await vi.runAllTimersAsync();
  expect(fetch).not.toHaveBeenCalled();
});

test("merge_order_is_by_type_then_relevance", async () => {
  // `page` answers first; the hits still come in the caller's type order, and each
  // type's hits in the server's (relevance) order.
  stubFetch(async (r) => {
    const type = typeOf(r);
    if (type === "post") {
      await new Promise((res) => setTimeout(res, 40));
      return list([
        summary("post", "p1", "Why WebAssembly sandboxes"),
        summary("post", "p2", "Writing your first plugin", "draft"),
      ]);
    }
    if (type === "page") return list([summary("page", "g1", "About WebAssembly")]);
    return list([summary(type, "r1", "WebAssembly stew", "pending")]);
  });
  const done = searchContent(
    "webassembly",
    ["post", "page", "recipe"],
    new AbortController().signal,
  );
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS + 40);
  const hits = await done;
  expect(hits.map((h) => `${h.type}:${h.id}`)).toEqual([
    "post:p1",
    "post:p2",
    "page:g1",
    "recipe:r1",
  ]);
  expect(hits[1]).toEqual({
    type: "post",
    id: "p2",
    title: "Writing your first plugin",
    status: "draft",
    slug: "writing-your-first-plugin",
  });
});

test("one_failing_type_keeps_the_others", async () => {
  stubFetch((r) =>
    typeOf(r) === "page" ? problem(500) : list([summary("post", "p1", "Hello world")]),
  );
  const done = searchContent("hello", ["post", "page"], new AbortController().signal);
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  expect((await done).map((h) => h.id)).toEqual(["p1"]);
});

test("every_type_failing_rejects", async () => {
  stubFetch(() => problem(500));
  const outcome = searchContent("hello", ["post", "page"], new AbortController().signal).catch(
    (e: unknown) => e,
  );
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  expect(await outcome).toMatchObject({ status: 500 });
});
