// The live preview's frame source (design 15 §3, ruling P7 D16): the pane loads a signed
// 1 h preview link of the item with `?working=1` (or `?release=<id>` in change mode),
// loads it again on every refresh, and makes a new link before the old one expires.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, expect, test, vi } from "vitest";
import { json, stubRoutes } from "@/test/render";
import { LINK_REFRESH_MARGIN_MS, usePreviewFrame } from "./use-preview-frame";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";
const RID = "0190f1c0-0000-7000-8000-0000000000cc";
const LINKS = `/api/v1/content/post/${ID}/preview-links`;

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return createElement(QueryClientProvider, { client }, children);
}

let n = 0;
function linkRoute(bodies: unknown[], expiresInMs = 3_600_000) {
  return {
    [LINKS]: async (_url: URL, req: Request) => {
      expect(req.method).toBe("POST");
      bodies.push(await req.json());
      n += 1;
      return json(
        {
          id: `l${n}`,
          token: `ferp_${String(n).padStart(43, "a")}`,
          url: `https://site.example/_preview/link/ferp_${String(n).padStart(43, "a")}`,
          target_kind: "item",
          target_id: ID,
          created_by: "u",
          created_at: new Date().toISOString(),
          expires_at: new Date(Date.now() + expiresInMs).toISOString(),
        },
        201,
      );
    },
  };
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  n = 0;
});

test("loads_a_one_hour_link_with_the_working_copy", async () => {
  const bodies: unknown[] = [];
  stubRoutes(linkRoute(bodies));
  const { result } = renderHook(
    () => usePreviewFrame({ type: "post", itemId: ID, refreshKey: 0, enabled: true }),
    { wrapper },
  );
  await waitFor(() => expect(result.current.src).not.toBeNull());
  expect(bodies).toEqual([{ expires_in_hours: 1 }]);
  // The path on this origin, not the site URL's host.
  expect(result.current.src).toBe(`/_preview/link/ferp_${"1".padStart(43, "a")}?working=1`);
});

test("a_refresh_loads_the_frame_again_with_the_same_link", async () => {
  const bodies: unknown[] = [];
  stubRoutes(linkRoute(bodies));
  const { result, rerender } = renderHook(
    ({ k }: { k: number }) =>
      usePreviewFrame({ type: "post", itemId: ID, refreshKey: k, enabled: true }),
    { wrapper, initialProps: { k: 0 } },
  );
  await waitFor(() => expect(result.current.src).not.toBeNull());
  const first = result.current.src;
  rerender({ k: 1 });
  await waitFor(() => expect(result.current.src).not.toBe(first));
  expect(result.current.src).toMatch(/\?working=1&r=1$/);
  expect(bodies).toHaveLength(1);
});

test("change_mode_previews_the_staged_change", async () => {
  stubRoutes(linkRoute([]));
  const { result } = renderHook(
    () =>
      usePreviewFrame({ type: "post", itemId: ID, releaseId: RID, refreshKey: 0, enabled: true }),
    { wrapper },
  );
  await waitFor(() => expect(result.current.src).not.toBeNull());
  expect(result.current.src).toMatch(new RegExp(`\\?release=${RID}$`));
});

test("a_new_link_is_made_before_the_old_one_expires", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const bodies: unknown[] = [];
  // Expires in 10 minutes: renewed at 10 min minus the margin.
  stubRoutes(linkRoute(bodies, 10 * 60_000));
  const { result } = renderHook(
    () => usePreviewFrame({ type: "post", itemId: ID, refreshKey: 0, enabled: true }),
    { wrapper },
  );
  await waitFor(() => expect(result.current.src).not.toBeNull());
  const first = result.current.src;
  await act(() => vi.advanceTimersByTimeAsync(10 * 60_000 - LINK_REFRESH_MARGIN_MS + 1_000));
  await waitFor(() => expect(bodies).toHaveLength(2));
  await waitFor(() => expect(result.current.src).not.toBe(first));
});

test("nothing_without_an_item_or_while_closed", async () => {
  const seen = stubRoutes({});
  const closed = renderHook(
    () => usePreviewFrame({ type: "post", itemId: ID, refreshKey: 0, enabled: false }),
    { wrapper },
  );
  const fresh = renderHook(
    () => usePreviewFrame({ type: "post", itemId: null, refreshKey: 0, enabled: true }),
    { wrapper },
  );
  await act(() => Promise.resolve());
  expect(closed.result.current.src).toBeNull();
  expect(fresh.result.current.src).toBeNull();
  expect(seen).toEqual([]);
});

test("a_refused_link_says_so_and_retries", async () => {
  let refuse = true;
  const bodies: unknown[] = [];
  const ok = linkRoute(bodies)[LINKS];
  stubRoutes({
    [LINKS]: (url, req) =>
      refuse
        ? json({ type: "about:blank", title: "Too many requests", status: 429 }, 429)
        : ok(url, req),
  });
  const { result } = renderHook(
    () => usePreviewFrame({ type: "post", itemId: ID, refreshKey: 0, enabled: true }),
    { wrapper },
  );
  await waitFor(() => expect(result.current.failed).toBe(true));
  expect(result.current.src).toBeNull();
  refuse = false;
  act(() => result.current.retry());
  await waitFor(() => expect(result.current.src).not.toBeNull());
  expect(result.current.failed).toBe(false);
});
