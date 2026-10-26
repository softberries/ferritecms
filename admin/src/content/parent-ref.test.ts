// The shared parent resolver (list cell and editor field): found, not available (404 or
// 403: settled, never retried, never "loading" for good), loading, and none.

import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, test, vi } from "vitest";
import { json, stubRoutes } from "@/test/render";
import { fetchParentRef, parentRefQuery, parentState } from "./parent-ref";

afterEach(() => {
  vi.unstubAllGlobals();
});

const problem = (status: number, slug: string) =>
  new Response(
    JSON.stringify({
      type: `https://ferritecms.dev/problems/${slug}`,
      title: slug,
      status,
      detail: "",
    }),
    { status, headers: { "Content-Type": "application/problem+json" } },
  );

describe("parent-ref", () => {
  test("found_unavailable_and_errors", async () => {
    stubRoutes({
      "/api/v1/content/page/p1": () => json({ id: "p1", title: "About", slug: "about" }),
      "/api/v1/content/page/p2": () => problem(404, "not-found"),
      "/api/v1/content/page/p3": () => problem(403, "forbidden"),
      "/api/v1/content/page/p4": () => problem(500, "internal"),
    });
    expect(await fetchParentRef("page", "p1")).toEqual({ title: "About", slug: "about" });
    expect(await fetchParentRef("page", "p2")).toBeNull();
    expect(await fetchParentRef("page", "p3")).toBeNull();
    await expect(fetchParentRef("page", "p4")).rejects.toThrow();
  });

  test("a_404_settles_once_without_retry", async () => {
    const seen = stubRoutes({ "/api/v1/content/page/p2": () => problem(404, "not-found") });
    const qc = new QueryClient();
    const data = await qc.fetchQuery(parentRefQuery("page", "p2"));
    expect(data).toBeNull();
    // Settled: success with `null`, cached, asked once.
    expect(qc.getQueryState(parentRefQuery("page", "p2").queryKey)?.status).toBe("success");
    await qc.fetchQuery(parentRefQuery("page", "p2"));
    expect(seen).toHaveLength(1);
  });

  test("states", () => {
    const pending = { isPending: true, isError: false };
    expect(parentState(null, pending)).toEqual({ kind: "none" });
    expect(parentState(undefined, pending)).toEqual({ kind: "none" });
    expect(parentState("p1", pending)).toEqual({ kind: "loading" });
    expect(
      parentState("p1", { data: { title: "A", slug: "a" }, isPending: false, isError: false }),
    ).toEqual({ kind: "found", ref: { title: "A", slug: "a" } });
    expect(parentState("p1", { data: null, isPending: false, isError: false })).toEqual({
      kind: "unavailable",
    });
    expect(parentState("p1", { isPending: false, isError: true })).toEqual({ kind: "unavailable" });
    // What the page already knows wins over the query.
    expect(parentState("p1", pending, { title: "Known", slug: "k" })).toEqual({
      kind: "found",
      ref: { title: "Known", slug: "k" },
    });
  });
});
