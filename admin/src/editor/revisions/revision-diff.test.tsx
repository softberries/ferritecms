// The revision diff's marks carry more than colour (an underline, a strike-through) and
// say "added"/"removed" to screen readers; the drawer's model: the `restored from …`
// note, word counts, day groups and the revision an Undo goes back to.

import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { diffWords } from "../diff";
import { RevisionDiff } from "./revision-diff";
import {
  type RevisionSummary,
  diffStats,
  groupByDay,
  initials,
  restoredFrom,
  undoTarget,
} from "./revision-model";

const RID_A = "0190f1c2-6d2e-7a51-9a3b-1c2d3e4f5a6b";
const RID_B = "0190f1c2-6d2e-7a51-9a3b-1c2d3e4f5a6c";
const RID_C = "0190f1c2-6d2e-7a51-9a3b-1c2d3e4f5a6d";

function rev(id: string, at: string, note: string | null = null): RevisionSummary {
  return {
    id,
    created_at: at,
    note,
    title: "T",
    author_id: "u",
    author: { id: "u", login: "ada", display_name: "Ada Lovelace" },
  };
}

describe("RevisionDiff", () => {
  test("revision_diff_marks_are_not_colour_only", () => {
    const { container } = render(
      <RevisionDiff parts={diffWords("the quick fox", "the slow fox")} />,
    );
    const ins = container.querySelector("ins");
    const del = container.querySelector("del");
    expect(ins).not.toBeNull();
    expect(del).not.toBeNull();
    // An underline and a strike-through, not only a tint.
    expect(ins).toHaveClass("underline");
    expect(del).toHaveClass("line-through");
    // Reachable as "added"/"removed" text.
    expect(ins).toHaveAttribute("aria-label", "Added: slow");
    expect(del).toHaveAttribute("aria-label", "Removed: quick");
    expect(ins).toHaveTextContent(/added/i);
    expect(del).toHaveTextContent(/removed/i);
    // Unchanged text is plain.
    expect(screen.getByText(/fox/)).not.toHaveClass("underline");
  });

  test("no_parts_says_empty", () => {
    render(<RevisionDiff parts={[]} />);
    expect(screen.getByText("Empty")).toBeInTheDocument();
  });

  test("whitespace_only_marks_are_labelled", () => {
    const { container } = render(<RevisionDiff parts={[{ kind: "added", text: "\n\n" }]} />);
    expect(container.querySelector("ins")).toHaveAttribute("aria-label", "Added: line break");
  });

  test("paragraphs_and_a_lone_break_are_shown", () => {
    const { container } = render(
      <RevisionDiff
        parts={[
          { kind: "same", text: "One" },
          { kind: "added", text: "\n" },
          { kind: "same", text: "Two\nThree" },
        ]}
      />,
    );
    expect([...container.querySelectorAll("p")].map((p) => p.textContent)).toEqual([
      "Oneadded: ¶",
      "Two",
      "Three",
    ]);
    expect(container.querySelector("ins")).toHaveAttribute("aria-label", "Added: line break");
  });
});

describe("revision model", () => {
  test("restored_from_reads_the_note", () => {
    expect(restoredFrom(`restored from ${RID_A}`)).toBe(RID_A);
    expect(restoredFrom(`restored from ${RID_A.toUpperCase()}`)).toBe(RID_A);
    expect(restoredFrom(null)).toBeNull();
    expect(restoredFrom("imported")).toBeNull();
    expect(restoredFrom("restored from nowhere")).toBeNull();
  });

  test("diff_stats_count_words", () => {
    expect(diffStats(diffWords("one two three", "one four three five"))).toEqual({
      added: 2,
      removed: 1,
    });
    expect(diffStats(diffWords("a b", "a  b"))).toEqual({ added: 0, removed: 0 });
  });

  test("undo_goes_to_the_one_after_the_restore_snapshot", () => {
    const after = [
      rev(RID_C, "2026-10-26T10:03:00Z", `restored from ${RID_A}`),
      rev(RID_B, "2026-10-26T10:02:00Z"),
      rev(RID_A, "2026-10-26T10:01:00Z"),
    ];
    expect(undoTarget(after, RID_A)).toBe(RID_B);
    // Someone saved after the restore: no safe target.
    expect(undoTarget([rev(RID_C, "2026-10-26T10:04:00Z"), ...after], RID_A)).toBeNull();
    expect(undoTarget([], RID_A)).toBeNull();
  });

  test("groups_by_local_day", () => {
    const now = new Date(2026, 9, 26, 15, 0);
    const at = (d: number, h: number) => new Date(2026, 9, d, h, 0).toISOString();
    const groups = groupByDay(
      [rev(RID_C, at(26, 9)), rev(RID_B, at(25, 23)), rev(RID_A, at(20, 8))],
      now,
    );
    expect(groups.map((g) => [g.day, g.items.length])).toEqual([
      ["today", 1],
      ["yesterday", 1],
      ["earlier", 1],
    ]);
  });

  test("initials_of_names", () => {
    expect(initials("Ada Lovelace")).toBe("AL");
    expect(initials("ada")).toBe("A");
    expect(initials("  ")).toBe("?");
  });
});
