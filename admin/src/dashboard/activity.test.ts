import { afterEach, describe, expect, it, vi } from "vitest";
import { PUBLISHED_CAP, bucketByDay, dayKey, fetchPublished, isCapped, lastDays } from "./activity";

const WARSAW = "Europe/Warsaw";
const NEW_YORK = "America/New_York";

const at = (iso: string) => ({ published_at: iso });

describe("activity_test", () => {
  it("buckets split at local midnight, not at UTC midnight", () => {
    const now = new Date("2026-10-20T12:00:00Z");
    // Warsaw is UTC+2 in October: local midnight of the 20th is 22:00Z on the 19th.
    const buckets = bucketByDay(
      [at("2026-10-19T21:59:59Z"), at("2026-10-19T22:00:00Z"), at("2026-10-20T11:00:00Z")],
      2,
      now,
      WARSAW,
    );
    expect(buckets).toEqual([
      { day: "2026-10-19", count: 1 },
      { day: "2026-10-20", count: 2 },
    ]);
    // The same instants in UTC fall on other days.
    expect(bucketByDay([at("2026-10-19T22:00:00Z")], 2, now, "UTC")).toEqual([
      { day: "2026-10-19", count: 1 },
      { day: "2026-10-20", count: 0 },
    ]);
  });

  it("a DST change day is one bucket of 25 (or 23) hours", () => {
    // Warsaw leaves summer time on 2026-10-25 (03:00 CEST → 02:00 CET).
    const now = new Date("2026-10-27T10:00:00Z");
    const buckets = bucketByDay(
      [
        at("2026-10-24T21:59:59Z"), // 23:59:59 CEST on the 24th
        at("2026-10-24T22:00:00Z"), // 00:00 CEST on the 25th
        at("2026-10-25T00:30:00Z"), // 02:30 CEST (the first of the two 02:30s)
        at("2026-10-25T01:30:00Z"), // 02:30 CET (the second)
        at("2026-10-25T22:59:59Z"), // 23:59:59 CET on the 25th
        at("2026-10-25T23:00:00Z"), // 00:00 CET on the 26th
      ],
      5,
      now,
      WARSAW,
    );
    expect(buckets).toEqual([
      { day: "2026-10-23", count: 0 },
      { day: "2026-10-24", count: 1 },
      { day: "2026-10-25", count: 4 },
      { day: "2026-10-26", count: 1 },
      { day: "2026-10-27", count: 0 },
    ]);

    // New York enters summer time on 2026-03-08 (02:00 EST → 03:00 EDT): a 23-hour day.
    const spring = bucketByDay(
      [
        at("2026-03-08T04:59:59Z"), // 23:59:59 EST on the 7th
        at("2026-03-08T05:00:00Z"), // 00:00 EST on the 8th
        at("2026-03-09T03:59:59Z"), // 23:59:59 EDT on the 8th
        at("2026-03-09T04:00:00Z"), // 00:00 EDT on the 9th
      ],
      3,
      new Date("2026-03-09T15:00:00Z"),
      NEW_YORK,
    );
    expect(spring).toEqual([
      { day: "2026-03-07", count: 1 },
      { day: "2026-03-08", count: 2 },
      { day: "2026-03-09", count: 1 },
    ]);
  });

  it("always returns `days` zero-filled buckets, oldest first", () => {
    const now = new Date("2026-10-25T08:00:00Z");
    const buckets = bucketByDay([], 30, now, WARSAW);
    expect(buckets).toHaveLength(30);
    expect(buckets.every((b) => b.count === 0)).toBe(true);
    expect(buckets[0]?.day).toBe("2026-09-26");
    expect(buckets.at(-1)?.day).toBe("2026-10-25");
    // Consecutive calendar days across the DST change and a month end.
    expect(lastDays(3, new Date("2026-11-01T12:00:00Z"), WARSAW)).toEqual([
      "2026-10-30",
      "2026-10-31",
      "2026-11-01",
    ]);
    // "Today" is the local date: 23:30Z on the 31st is already 1 November in Warsaw.
    expect(dayKey(new Date("2026-10-31T23:30:00Z"), WARSAW)).toBe("2026-11-01");
  });

  it("ignores items without published_at and outside the window", () => {
    const now = new Date("2026-10-25T08:00:00Z");
    const buckets = bucketByDay(
      [
        { published_at: null },
        {},
        at("not a date"),
        at("2026-09-01T10:00:00Z"), // before the window
        at("2026-10-26T10:00:00Z"), // in the future
        at("2026-10-24T10:00:00Z"),
      ],
      7,
      now,
      WARSAW,
    );
    expect(buckets.reduce((n, b) => n + b.count, 0)).toBe(1);
    expect(buckets.find((b) => b.day === "2026-10-24")?.count).toBe(1);
  });
});

function item(id: number, publishedAt: string | null) {
  return { id: `id-${id}`, published_at: publishedAt };
}

function page(items: unknown[], next: string | null): Response {
  return new Response(JSON.stringify({ items, next_cursor: next }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

describe("fetchPublished", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("pages until an item is older than since, with the published order", async () => {
    const urls: URL[] = [];
    const answers = [
      page([item(1, "2026-10-24T10:00:00Z"), item(2, "2026-10-20T10:00:00Z")], "c1"),
      page([item(3, "2026-10-10T10:00:00Z"), item(4, "2026-09-01T10:00:00Z")], "c2"),
    ];
    vi.stubGlobal(
      "fetch",
      vi.fn((req: Request) => {
        urls.push(new URL(req.url));
        const next = answers.shift();
        if (!next) throw new Error("one request too many");
        return Promise.resolve(next);
      }),
    );
    const items = await fetchPublished("post", new Date("2026-09-25T00:00:00Z"));
    expect(items.map((i) => i.id)).toEqual(["id-1", "id-2", "id-3"]);
    expect(urls).toHaveLength(2);
    expect(urls[0]?.pathname).toBe("/api/v1/content/post");
    expect(Object.fromEntries(urls[0]?.searchParams ?? [])).toEqual({
      status: "published",
      order: "published",
      limit: "200",
    });
    expect(urls[1]?.searchParams.get("cursor")).toBe("c1");
    expect(isCapped(items)).toBe(false);
  });

  it("stops at the last page and at five pages (1 000 items)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(page([item(1, "2026-10-24T10:00:00Z")], null))),
    );
    expect(await fetchPublished("page", new Date("2026-09-25T00:00:00Z"))).toHaveLength(1);

    let n = 0;
    const fetch = vi.fn(() => {
      const items = Array.from({ length: 200 }, () => item(n++, "2026-10-24T10:00:00Z"));
      return Promise.resolve(page(items, `c${n}`));
    });
    vi.stubGlobal("fetch", fetch);
    const many = await fetchPublished("post", new Date("2026-09-25T00:00:00Z"));
    expect(fetch).toHaveBeenCalledTimes(5);
    expect(many).toHaveLength(PUBLISHED_CAP);
    expect(isCapped(many)).toBe(true);
  });
});
