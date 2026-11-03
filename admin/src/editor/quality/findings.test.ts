import { getSchema } from "@tiptap/core";
import { Node as PMNode } from "@tiptap/pm/model";
import { describe, expect, expectTypeOf, test } from "vitest";
import { i18n } from "@/i18n";
import { RULE_IDS, type FindingDto } from "@/api/quality";
import type { components } from "@/api/schema";
import { editorExtensions } from "../schema";
import { blockingFindings, findingPos, groupFindings, messageFor } from "./findings";

const schema = getSchema(
  editorExtensions({ placeholder: "", resolveMedia: () => Promise.resolve(null) }),
);
const doc = (content: unknown[]) => PMNode.fromJSON(schema, { type: "doc", content });
const p = (...text: string[]) => ({
  type: "paragraph",
  content: text.map((t) => ({ type: "text", text: t })),
});

function finding(over: Partial<FindingDto>): FindingDto {
  return {
    rule: "image-alt-missing",
    severity: "error",
    category: "accessibility",
    wcag: ["1.1.1"],
    location: { kind: "body", path: [0] },
    key: "/a.png",
    context: null,
    detail: {},
    suppressed: null,
    ...over,
  };
}

describe("findingPos", () => {
  test("maps top-level, nested list and table paths to node positions", () => {
    const d = doc([
      p("one"),
      {
        type: "bulletList",
        content: [
          { type: "listItem", content: [p("a")] },
          {
            type: "listItem",
            content: [
              {
                type: "paragraph",
                content: [
                  { type: "text", text: "b" },
                  { type: "text", text: "c", marks: [{ type: "bold" }] },
                ],
              },
            ],
          },
        ],
      },
      {
        type: "table",
        content: [
          {
            type: "tableRow",
            content: [
              { type: "tableHeader", content: [p("H")] },
              { type: "tableCell", content: [p("x")] },
            ],
          },
        ],
      },
    ]);
    for (const path of [[0], [1, 1, 0], [1, 1, 0, 1], [2, 0, 1, 0, 0]]) {
      const pos = findingPos(d, path);
      expect(pos).not.toBeNull();
      // The node at `pos` is the one the path names.
      let n: PMNode = d;
      for (const i of path) n = n.child(i);
      expect(d.nodeAt(pos!)).toBe(n);
    }
    expect(findingPos(d, [9])).toBeNull();
    expect(findingPos(d, [0, 5])).toBeNull();
    expect(findingPos(d, [])).toBeNull();
    expect(findingPos(d, [-1])).toBeNull();
  });
});

describe("grouping and words", () => {
  test("groups by severity, collapses suppressed, lists gate errors", () => {
    const fs = [
      finding({ rule: "link-text-vague", severity: "warning", key: "a" }),
      finding({ key: "b" }),
      finding({
        key: "c",
        suppressed: {
          rule: "image-alt-missing",
          key: "c",
          reason: "r",
          user_id: null,
          created_at: "x",
        },
      }),
      finding({ rule: "media-missing", category: "media", key: "d" }),
    ];
    const g = groupFindings(fs);
    expect(g.bySeverity.map((x) => x.severity)).toEqual(["error", "warning"]);
    expect(g.suppressed).toHaveLength(1);
    expect(blockingFindings(fs).map((f) => f.key)).toEqual(["b"]);
  });

  test("every rule id of the API has a title and a message", () => {
    // Every id of the schema's enum is listed (a type check), each once.
    expectTypeOf<
      Exclude<components["schemas"]["RuleId"], (typeof RULE_IDS)[number]>
    >().toEqualTypeOf<never>();
    expect(new Set(RULE_IDS).size).toBe(RULE_IDS.length);
    const t = i18n.getFixedT("en", "quality");
    for (const rule of RULE_IDS) {
      expect(i18n.exists(`quality:rules.${rule}.title`), rule).toBe(true);
      expect(i18n.exists(`quality:rules.${rule}.message`), rule).toBe(true);
    }
    expect(messageFor(t, { rule: "link-text-vague", context: "here" })).toContain("here");
  });
});
