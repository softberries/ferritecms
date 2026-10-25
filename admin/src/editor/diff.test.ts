import { describe, expect, test } from "vitest";
import { diffRevision, diffWords } from "./diff";

describe("diffWords", () => {
  test("word_level_additions_and_removals", () => {
    expect(diffWords("the quick fox", "the slow fox")).toEqual([
      { kind: "same", text: "the " },
      { kind: "removed", text: "quick" },
      { kind: "added", text: "slow" },
      { kind: "same", text: " fox" },
    ]);
    expect(diffWords("a b", "a b c")).toEqual([
      { kind: "same", text: "a b" },
      { kind: "added", text: " c" },
    ]);
  });

  test("whitespace_is_kept", () => {
    const parts = diffWords("one two", "one  two");
    expect(parts.map((p) => p.text).join("")).not.toBe("one two");
    expect(parts.some((p) => p.kind === "added" && /^\s+$/.test(p.text))).toBe(true);
    // Rebuilding either side from the parts gives it back exactly.
    const older = parts
      .filter((p) => p.kind !== "added")
      .map((p) => p.text)
      .join("");
    const newer = parts
      .filter((p) => p.kind !== "removed")
      .map((p) => p.text)
      .join("");
    expect(older).toBe("one two");
    expect(newer).toBe("one  two");
  });

  test("empty_to_text", () => {
    expect(diffWords("", "Hello world")).toEqual([{ kind: "added", text: "Hello world" }]);
    expect(diffWords("Hello world", "")).toEqual([{ kind: "removed", text: "Hello world" }]);
  });

  test("identical_inputs_give_one_same_part", () => {
    expect(diffWords("same text here", "same text here")).toEqual([
      { kind: "same", text: "same text here" },
    ]);
  });
});

describe("diffRevision", () => {
  test("compares_title_excerpt_and_body_text", () => {
    const p = (t: string) => ({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: t }] }],
    });
    const d = diffRevision(
      { title: "Old", excerpt: "same", body: p("first draft") },
      { title: "New", excerpt: "same", body: p("second draft") },
    );
    expect(d.title).toEqual([
      { kind: "removed", text: "Old" },
      { kind: "added", text: "New" },
    ]);
    expect(d.excerpt).toEqual([{ kind: "same", text: "same" }]);
    expect(d.body).toEqual([
      { kind: "removed", text: "first" },
      { kind: "added", text: "second" },
      { kind: "same", text: " draft" },
    ]);
  });
});
