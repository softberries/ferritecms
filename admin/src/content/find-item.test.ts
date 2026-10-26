import { describe, expect, it, vi } from "vitest";
import type { SchemaContentRefDto } from "@/api/schema";
import { MAX_REFS, createRefLoader } from "./find-item";

const ref = (id: string): SchemaContentRefDto => ({
  id,
  type: "post",
  title: `Title ${id}`,
  status: "published",
  permalink: `/${id}/`,
});

describe("createRefLoader", () => {
  it("batches_one_tick_into_one_request", async () => {
    const fetch = vi.fn((ids: string[]) =>
      Promise.resolve(ids.filter((id) => id !== "gone").map(ref)),
    );
    const load = createRefLoader(fetch);
    const [a, b, gone, again] = await Promise.all([load("a"), load("b"), load("gone"), load("a")]);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch).toHaveBeenCalledWith(["a", "b", "gone"]);
    expect(a?.title).toBe("Title a");
    expect(b?.id).toBe("b");
    expect(again).toEqual(a);
    // Unknown or unreadable: absent from the answer, so `null`.
    expect(gone).toBeNull();
  });

  it("splits_more_than_max_refs", async () => {
    const fetch = vi.fn((ids: string[]) => Promise.resolve(ids.map(ref)));
    const load = createRefLoader(fetch);
    const ids = Array.from({ length: MAX_REFS + 5 }, (_, i) => `id${i}`);
    const got = await Promise.all(ids.map((id) => load(id)));
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(fetch.mock.calls[0]?.[0]).toHaveLength(MAX_REFS);
    expect(fetch.mock.calls[1]?.[0]).toHaveLength(5);
    expect(got.every((r, i) => r?.id === ids[i])).toBe(true);
  });

  it("a_failed_request_rejects_its_batch_only", async () => {
    let calls = 0;
    const fetch = vi.fn((ids: string[]) => {
      calls += 1;
      if (calls === 1) return Promise.reject(new Error("offline"));
      return Promise.resolve(ids.map(ref));
    });
    const load = createRefLoader(fetch);
    await expect(Promise.all([load("a"), load("b")])).rejects.toThrow("offline");
    await expect(load("a")).resolves.toMatchObject({ id: "a" });
  });
});
