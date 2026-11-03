import { describe, expect, test, vi } from "vitest";
import { ApiError } from "@/api/problem";
import type { SchemaAllowedActionsDto, SchemaContentDto } from "@/api/schema";
import {
  type BulkItem,
  countAllowed,
  runBulk,
  trashUndoable,
  undoTrash,
  undoableEntries,
  type UndoTransitions,
} from "./bulk";

const NONE: SchemaAllowedActionsDto = {
  delete: false,
  edit: false,
  publish: false,
  restore: false,
  schedule: false,
  submit: false,
  trash: false,
  trash_undoable: false,
  needs_approval: false,
  stage: false,
  override: false,
  unpublish: false,
};

function item(id: string, allowed: Partial<SchemaAllowedActionsDto> = {}): BulkItem {
  return { id, allowed: { ...NONE, ...allowed } };
}

function problem(slug: string, status: number, detail = ""): ApiError {
  return new ApiError({
    type: `https://ferritecms.dev/problems/${slug}`,
    title: slug,
    status,
    detail,
  });
}

/** A promise with its `resolve`, to hold requests open. */
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe("bulk_test", () => {
  test("concurrency is at most 4", async () => {
    const items = Array.from({ length: 10 }, (_, i) => item(`i${i}`, { publish: true }));
    let inFlight = 0;
    let peak = 0;
    const gates = new Map<string, ReturnType<typeof deferred>>();
    const run = vi.fn(async (it: BulkItem) => {
      inFlight++;
      peak = Math.max(peak, inFlight);
      const gate = deferred();
      gates.set(it.id, gate);
      await gate.promise;
      inFlight--;
    });
    const done = runBulk(items, "publish", run);
    // Release the requests one by one, letting the pool refill in between.
    for (let i = 0; i < items.length; i++) {
      await vi.waitFor(() => {
        expect(gates.size).toBeGreaterThan(i);
      });
      expect(inFlight).toBeLessThanOrEqual(4);
      gates.get(`i${i}`)?.resolve();
    }
    const result = await done;
    expect(peak).toBe(4);
    expect(run).toHaveBeenCalledTimes(10);
    expect(result.ok).toEqual(items.map((i) => i.id));
    expect(result.failed).toEqual([]);
  });

  test("a smaller concurrency is honoured", async () => {
    let inFlight = 0;
    let peak = 0;
    const items = Array.from({ length: 6 }, (_, i) => item(`i${i}`, { trash: true }));
    await runBulk(
      items,
      "trash",
      async () => {
        inFlight++;
        peak = Math.max(peak, inFlight);
        await new Promise((r) => setTimeout(r, 1));
        inFlight--;
      },
      2,
    );
    expect(peak).toBe(2);
  });

  test("skips disallowed", async () => {
    const items = [
      item("a", { publish: true }),
      item("b", { trash: true }),
      item("c", { publish: true }),
      item("d"),
    ];
    const run = vi.fn<(item: BulkItem) => Promise<void>>(() => Promise.resolve());
    const result = await runBulk(items, "publish", run);
    expect(run.mock.calls.map(([it]) => it.id)).toEqual(["a", "c"]);
    expect(result).toEqual({ ok: ["a", "c"], failed: [] });
    expect(countAllowed(items, "publish")).toBe(2);
    expect(countAllowed(items, "delete")).toBe(0);
    // Nothing allowed: nothing sent.
    expect(await runBulk(items, "delete", run)).toEqual({ ok: [], failed: [] });
    expect(run).toHaveBeenCalledTimes(2);
  });

  test("collects failures with their problem", async () => {
    const stale = problem("precondition-failed", 412, "The item changed since you loaded it.");
    const items = ["a", "b", "c", "d"].map((id) => item(id, { publish: true }));
    const result = await runBulk(items, "publish", (it) => {
      if (it.id === "b") return Promise.reject(stale);
      if (it.id === "d") return Promise.reject(new TypeError("fetch failed"));
      return Promise.resolve();
    });
    expect(result.ok).toEqual(["a", "c"]);
    expect(result.failed.map((f) => f.id)).toEqual(["b", "d"]);
    expect(result.failed[0]?.error).toBe(stale);
    expect(result.failed[0]?.error.slug).toBe("precondition-failed");
    expect(result.failed[0]?.error.problem.detail).toBe("The item changed since you loaded it.");
    // A request without an answer is a status-0 problem, never a thrown non-ApiError.
    expect(result.failed[1]?.error).toBeInstanceOf(ApiError);
    expect(result.failed[1]?.error.status).toBe(0);
  });
});

describe("undo_trash", () => {
  const NOW = Date.parse("2026-10-25T12:00:00Z");

  function dto(status: string, version: number, allowed: Partial<SchemaAllowedActionsDto> = {}) {
    return {
      id: "x",
      status,
      version,
      allowed: { ...NONE, ...allowed },
    } as unknown as SchemaContentDto;
  }

  function transitions(restored: SchemaContentDto) {
    const calls: string[] = [];
    const ops: UndoTransitions = {
      restore: (id, v) => {
        calls.push(`restore ${id} ${v}`);
        return Promise.resolve(restored);
      },
      publish: (id, v) => {
        calls.push(`publish ${id} ${v}`);
        return Promise.resolve(dto("published", v + 1));
      },
      submit: (id, v) => {
        calls.push(`submit ${id} ${v}`);
        return Promise.resolve(dto("pending", v + 1));
      },
      schedule: (id, v, at) => {
        calls.push(`schedule ${id} ${v} ${at}`);
        return Promise.resolve(dto("scheduled", v + 1));
      },
    };
    return { ops, calls };
  }

  const trashed = (status: string, scheduled_at: string | null = null) => ({
    before: { id: "x", status, scheduled_at },
    after: dto("trash", 7, { restore: true }),
  });

  test("published comes back published", async () => {
    const { ops, calls } = transitions(dto("draft", 8, { publish: true }));
    const undone = await undoTrash(trashed("published"), ops, NOW);
    expect(calls).toEqual(["restore x 7", "publish x 8"]);
    expect(undone.item.status).toBe("published");
    expect(undone.notice).toBeUndefined();
  });

  test("pending is submitted again", async () => {
    const { ops, calls } = transitions(dto("draft", 8, { submit: true }));
    expect((await undoTrash(trashed("pending"), ops, NOW)).item.status).toBe("pending");
    expect(calls).toEqual(["restore x 7", "submit x 8"]);
  });

  test("scheduled keeps a future time, else stays a draft", async () => {
    const future = "2026-10-26T09:00:00Z";
    let t = transitions(dto("draft", 8, { schedule: true }));
    expect((await undoTrash(trashed("scheduled", future), t.ops, NOW)).item.status).toBe(
      "scheduled",
    );
    expect(t.calls).toEqual(["restore x 7", `schedule x 8 ${future}`]);

    t = transitions(dto("draft", 8, { schedule: true }));
    const lapsed = await undoTrash(trashed("scheduled", "2026-10-25T11:00:00Z"), t.ops, NOW);
    expect(lapsed).toMatchObject({ notice: "scheduleLapsed", item: { status: "draft" } });
    expect(t.calls).toEqual(["restore x 7"]);
  });

  test("private and drafts come back as drafts", async () => {
    let t = transitions(dto("draft", 8, { publish: true }));
    expect(await undoTrash(trashed("private"), t.ops, NOW)).toMatchObject({
      notice: "privateLost",
    });
    t = transitions(dto("draft", 8, { publish: true }));
    expect(await undoTrash(trashed("draft"), t.ops, NOW)).toEqual({
      item: dto("draft", 8, { publish: true }),
    });
    expect(t.calls).toEqual(["restore x 7"]);
  });

  test("a status the restored item may not reach is not attempted", async () => {
    // An author without publish rights on this item: the server says no publish.
    const { ops, calls } = transitions(dto("draft", 8));
    expect(await undoTrash(trashed("published"), ops, NOW)).toMatchObject({
      notice: "notAllowed",
    });
    expect(calls).toEqual(["restore x 7"]);
  });

  test("a sign-in problem while re-applying reaches the caller", async () => {
    const { ops } = transitions(dto("draft", 8, { publish: true }));
    const expired = problem("unauthenticated", 401);
    ops.publish = () => Promise.reject(expired);
    await expect(undoTrash(trashed("published"), ops, NOW)).rejects.toBe(expired);
  });

  test("trash is undoable only when some item may be restored afterwards", () => {
    // A contributor's own drafts: trash yes, restore never (07 §1).
    expect(trashUndoable([item("a", { trash: true }), item("b", { trash: true })])).toBe(false);
    expect(
      trashUndoable([item("a", { trash: true }), item("b", { trash: true, trash_undoable: true })]),
    ).toBe(true);
    // Undoable but not trashable is not counted.
    expect(trashUndoable([item("a", { trash_undoable: true })])).toBe(false);
  });

  test("a mixed trash offers Undo only for the restorable items", () => {
    const entry = (id: string, restore: boolean) => ({
      before: { id, status: "draft", scheduled_at: null },
      after: { ...dto("trash", 3, { restore }), id },
    });
    const mixed = [entry("a", true), entry("b", false), entry("c", true)];
    expect(undoableEntries(mixed).map((e) => e.after.id)).toEqual(["a", "c"]);
    expect(undoableEntries([entry("b", false)])).toEqual([]);
  });

  test("a failed re-apply keeps the restored draft", async () => {
    const { ops } = transitions(dto("draft", 8, { publish: true }));
    ops.publish = () => Promise.reject(problem("precondition-failed", 412));
    expect(await undoTrash(trashed("published"), ops, NOW)).toMatchObject({
      notice: "failed",
      item: { status: "draft" },
    });
  });
});
