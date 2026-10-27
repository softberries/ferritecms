import { describe, expect, test, vi } from "vitest";
import { moderateComment } from "@/api/comments";
import { ApiError } from "@/api/problem";
import {
  type CommentAction,
  actionsFor,
  canDelete,
  nextStatuses,
  runDeletion,
  runModeration,
  statusOf,
} from "./moderate";

function problem(slug: string, status: number): ApiError {
  return new ApiError({
    type: `https://ferritecms.dev/problems/${slug}`,
    title: slug,
    status,
    detail: "",
  });
}

describe("moderate_test", () => {
  test("next statuses per status", () => {
    expect(nextStatuses("pending")).toEqual(["approved", "spam", "trash"]);
    expect(nextStatuses("approved")).toEqual(["pending", "spam", "trash"]);
    expect(nextStatuses("spam")).toEqual(["pending"]);
    expect(nextStatuses("trash")).toEqual(["pending"]);
  });

  test("only spam and trash can be deleted (the server's rule)", () => {
    expect(canDelete("pending")).toBe(false);
    expect(canDelete("approved")).toBe(false);
    expect(canDelete("spam")).toBe(true);
    expect(canDelete("trash")).toBe(true);
  });

  test("actions per status, as the queue offers them", () => {
    const table: Record<string, CommentAction[]> = {
      pending: ["approve", "reply", "spam", "trash"],
      approved: ["unapprove", "reply", "spam", "trash"],
      spam: ["notSpam", "delete"],
      trash: ["restore", "delete"],
    };
    for (const [status, actions] of Object.entries(table)) {
      expect(actionsFor(status as "pending"), status).toEqual(actions);
    }
    // Every status action leads to a status `nextStatuses` allows.
    for (const status of ["pending", "approved", "spam", "trash"] as const) {
      for (const action of actionsFor(status)) {
        const to = statusOf(action);
        if (to !== null) expect(nextStatuses(status), `${status} ${action}`).toContain(to);
      }
    }
  });

  test("bulk moderation collects failures and keeps the rest", async () => {
    const apply = vi.fn((id: string, status: string) =>
      id === "b" ? Promise.reject(problem("not-found", 404)) : Promise.resolve({ id, status }),
    );
    const result = await runModeration(["a", "b", "c"], "trash", apply);
    expect(result.ok).toEqual(["a", "c"]);
    expect(result.failed.map((f) => [f.id, f.error.status, f.error.slug])).toEqual([
      ["b", 404, "not-found"],
    ]);
    expect(apply.mock.calls.map(([id, s]) => `${id}:${s}`).sort()).toEqual([
      "a:trash",
      "b:trash",
      "c:trash",
    ]);
  });

  test("a request without an answer is a failure with status 0", async () => {
    const result = await runModeration(["a"], "approved", () =>
      Promise.reject(new TypeError("network")),
    );
    expect(result.ok).toEqual([]);
    expect(result.failed[0]?.error.status).toBe(0);
  });

  test("fan-out is at most 4", async () => {
    let inFlight = 0;
    let peak = 0;
    const ids = Array.from({ length: 10 }, (_, i) => `c${i}`);
    const result = await runModeration(ids, "spam", async () => {
      inFlight++;
      peak = Math.max(peak, inFlight);
      await new Promise((r) => setTimeout(r, 1));
      inFlight--;
    });
    expect(peak).toBe(4);
    expect(result.ok).toEqual(ids);
  });

  test("deletion fans out the same way", async () => {
    const result = await runDeletion(["a", "b"], (id) =>
      id === "a" ? Promise.reject(problem("validation", 422)) : Promise.resolve(),
    );
    expect(result.ok).toEqual(["b"]);
    expect(result.failed.map((f) => f.id)).toEqual(["a"]);
  });
});

test("moderate_comment_takes_only_a_comment_status", () => {
  // Compile-time: `tsc` (the typecheck gate) refuses anything but the four statuses.
  const wrong = () =>
    // @ts-expect-error "approve" is an action, not a status
    moderateComment("id", "approve");
  const right = () => moderateComment("id", "approved");
  expect([typeof wrong, typeof right]).toEqual(["function", "function"]);
});
