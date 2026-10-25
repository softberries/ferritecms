// `blockText` against core `blocks::extract_text`: the same fixture and expectations as
// the tests in `crates/ferrite-core/src/blocks/text.rs` (`extracts_with_boundaries`),
// copied here; keep the two in step.

import { describe, expect, test } from "vitest";
import { MAX_DEPTH, blockText } from "./text";
import { readingMinutes, wordCount } from "./word-count";

const doc = {
  type: "doc",
  content: [
    { type: "heading", attrs: { level: 1 }, content: [{ type: "text", text: "Title" }] },
    {
      type: "paragraph",
      content: [
        { type: "text", text: "Hello " },
        { type: "text", text: "world", marks: [{ type: "bold" }] },
        { type: "hardBreak" },
        { type: "text", text: "next" },
      ],
    },
    {
      type: "bulletList",
      content: [
        {
          type: "listItem",
          content: [{ type: "paragraph", content: [{ type: "text", text: "one" }] }],
        },
        {
          type: "listItem",
          content: [{ type: "paragraph", content: [{ type: "text", text: "two" }] }],
        },
      ],
    },
  ],
};

describe("blockText", () => {
  test("block_text_matches_core", () => {
    expect(blockText(doc)).toBe("Title\nHello world\nnext\none\ntwo");
    expect(blockText(null)).toBe("");
    expect(blockText({ type: "doc" })).toBe("");
  });

  test("tables_rules_and_code_end_lines", () => {
    const cell = (t: string) => ({
      type: "tableCell",
      content: [{ type: "paragraph", content: [{ type: "text", text: t }] }],
    });
    const d = {
      type: "doc",
      content: [
        { type: "codeBlock", content: [{ type: "text", text: "a\nb" }] },
        { type: "horizontalRule" },
        { type: "table", content: [{ type: "tableRow", content: [cell("x"), cell("y")] }] },
        { type: "html", attrs: { html: "<b>not text</b>" } },
      ],
    };
    expect(blockText(d)).toBe("a\nb\nx\ny");
  });

  test("deep_nodes_give_their_text_only", () => {
    let node: object = { type: "text", text: "deep" };
    for (let i = 0; i < MAX_DEPTH + 10; i++) node = { type: "paragraph", content: [node] };
    expect(
      blockText({
        type: "doc",
        content: [node, { type: "paragraph", content: [{ type: "text", text: "x" }] }],
      }),
    ).toBe("deep\nx");
  });
});

describe("wordCount", () => {
  test("counts_words_not_punctuation", () => {
    expect(wordCount("")).toBe(0);
    expect(wordCount("   \n ")).toBe(0);
    expect(wordCount("Hello, world — it's 2026!")).toBe(4);
    expect(wordCount(blockText(doc))).toBe(6);
  });

  test("reading_time", () => {
    expect(readingMinutes(0)).toBe(0);
    expect(readingMinutes(10)).toBe(1);
    expect(readingMinutes(238 * 5)).toBe(5);
  });
});
