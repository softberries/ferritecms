// The publishing-activity data of the dashboard (M2b P16): published items fetched in
// pages, bucketed per calendar day in the browser's time zone. There is no server-side
// time series; at most `PUBLISHED_CAP` items per type are read.

import { queryOptions } from "@tanstack/react-query";
import { listContent } from "@/api/content";
import { qk } from "@/api/keys";
import type { SchemaContentSummaryDto } from "@/api/schema";

export type ContentSummaryDto = SchemaContentSummaryDto;

/** The window of the activity chart and the Published sparkline (exact value, plan). */
export const ACTIVITY_DAYS = 30;
/** Items per request (the API's maximum page). */
export const PUBLISHED_PAGE = 200;
/** Requests per type at most: 5 × 200 = 1 000 items. */
export const PUBLISHED_MAX_PAGES = 5;
export const PUBLISHED_CAP = PUBLISHED_PAGE * PUBLISHED_MAX_PAGES;

export interface DayBucket {
  /** The calendar day, `YYYY-MM-DD`, in the given time zone. */
  day: string;
  count: number;
}

/** The browser's IANA time zone (`Intl` falls back to UTC). */
export function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

const dayFormats = new Map<string, Intl.DateTimeFormat>();

/** The calendar day (`YYYY-MM-DD`) of an instant in `timeZone`. */
export function dayKey(at: Date, timeZone: string): string {
  let format = dayFormats.get(timeZone);
  if (!format) {
    // `en-CA` formats as YYYY-MM-DD; only digits are read, so the locale is irrelevant
    // to the user.
    format = new Intl.DateTimeFormat("en-CA", {
      timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    });
    dayFormats.set(timeZone, format);
  }
  const parts = Object.fromEntries(format.formatToParts(at).map((p) => [p.type, p.value]));
  return `${parts.year ?? ""}-${parts.month ?? ""}-${parts.day ?? ""}`;
}

/**
 * The `days` calendar days ending with today (in `timeZone`), oldest first. Calendar
 * arithmetic runs on the date alone (as a UTC date), so a day with a DST change is one
 * day like any other.
 */
export function lastDays(days: number, now: Date, timeZone: string): string[] {
  const [y, m, d] = dayKey(now, timeZone).split("-").map(Number);
  const today = Date.UTC(y ?? 1970, (m ?? 1) - 1, d ?? 1);
  return Array.from({ length: days }, (_, i) =>
    new Date(today - (days - 1 - i) * 86_400_000).toISOString().slice(0, 10),
  );
}

/**
 * Items per calendar day over the last `days` days, in `timeZone`: always `days`
 * buckets, oldest first, zero-filled. Items without `published_at` (never published),
 * with an unparsable one or outside the window are ignored.
 */
export function bucketByDay(
  items: readonly { published_at?: string | null }[],
  days: number,
  now: Date,
  timeZone: string,
): DayBucket[] {
  const buckets = lastDays(days, now, timeZone).map((day) => ({ day, count: 0 }));
  const index = new Map(buckets.map((b, i) => [b.day, i]));
  for (const item of items) {
    if (!item.published_at) continue;
    const at = new Date(item.published_at);
    if (Number.isNaN(at.getTime())) continue;
    const i = index.get(dayKey(at, timeZone));
    const bucket = i === undefined ? undefined : buckets[i];
    if (bucket) bucket.count += 1;
  }
  return buckets;
}

/**
 * The published items of `type` published at or after `since`, newest first. Pages
 * `status=published&order=published&limit=200` until an item is older than `since` or
 * there is no next page, at most `PUBLISHED_MAX_PAGES` requests: a result of
 * `PUBLISHED_CAP` items may be cut short (`isCapped`).
 */
export async function fetchPublished(
  type: string,
  since: Date,
  options: { signal?: AbortSignal } = {},
): Promise<ContentSummaryDto[]> {
  const out: ContentSummaryDto[] = [];
  const from = since.getTime();
  let cursor: string | undefined;
  for (let page = 0; page < PUBLISHED_MAX_PAGES; page++) {
    const list = await listContent(
      type,
      {
        status: "published",
        order: "published",
        limit: PUBLISHED_PAGE,
        ...(cursor ? { cursor } : {}),
      },
      options,
    );
    let older = false;
    for (const item of list.items) {
      const at = item.published_at ? new Date(item.published_at).getTime() : Number.NaN;
      // Newest first, never-published last: the first older (or undated) item ends it.
      if (Number.isNaN(at) || at < from) {
        older = true;
        break;
      }
      out.push(item);
    }
    if (older || !list.next_cursor) break;
    cursor = list.next_cursor;
  }
  return out;
}

/** Whether `fetchPublished` stopped at its cap (older items in the window may be missing). */
export function isCapped(items: readonly unknown[]): boolean {
  return items.length >= PUBLISHED_CAP;
}

/**
 * The published items of the activity window. `since` is a day before the oldest
 * bucket's midnight in any time zone, so every bucket is complete; `bucketByDay` drops
 * the extra day.
 */
export function publishedQuery(type: string) {
  return queryOptions({
    queryKey: qk.content.published(type, ACTIVITY_DAYS),
    queryFn: ({ signal }) =>
      fetchPublished(type, new Date(Date.now() - (ACTIVITY_DAYS + 1) * 86_400_000), { signal }),
    staleTime: 60_000,
  });
}
