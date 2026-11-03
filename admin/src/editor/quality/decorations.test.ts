import { getSchema } from "@tiptap/core";
import { Node as PMNode } from "@tiptap/pm/model";
import { EditorState } from "@tiptap/pm/state";
import { describe, expect, test, vi } from "vitest";
import type { FindingDto } from "@/api/quality";
import { editorExtensions } from "../schema";
import { decorationsFor, qualityKey, qualityPlugin } from "./decorations";

const schema = getSchema(
  editorExtensions({ placeholder: "", resolveMedia: () => Promise.resolve(null) }),
);

function state() {
  const doc = PMNode.fromJSON(schema, {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          { type: "text", text: "see " },
          { type: "text", text: "here", marks: [{ type: "link", attrs: { href: "/x" } }] },
        ],
      },
      { type: "image", attrs: { src: "/a.png" } },
    ],
  });
  return EditorState.create({ doc, schema });
}

const f = (path: number[], over: Partial<FindingDto> = {}): FindingDto => ({
  rule: "link-text-vague",
  severity: "warning",
  category: "accessibility",
  wcag: [],
  location: { kind: "body", path },
  key: "k",
  context: "here",
  detail: {},
  suppressed: null,
  ...over,
});

describe("quality decorations", () => {
  test("marks text inline and blocks as nodes, each with a labelled widget", () => {
    const label = vi.fn(() => "Warning: vague");
    const set = decorationsFor(
      state(),
      [
        f([0, 1]),
        f([1], { rule: "image-alt-missing", severity: "error" }),
        f([0, 1], {
          suppressed: {
            rule: "image-alt-missing",
            key: "",
            reason: "r",
            user_id: null,
            created_at: "",
          },
        }),
        f([5]),
        f([0], { location: { kind: "field", field: "intro", path: [0] } }),
      ],
      { label, onActivate: vi.fn() },
    );
    const all = set.find();
    // Two findings shown: a mark and a widget each.
    expect(all).toHaveLength(4);
    const widgets = all.filter((d) => (d as unknown as { type: { toDOM?: unknown } }).type.toDOM);
    expect(widgets).toHaveLength(2);
    const dom = (widgets[0] as unknown as { type: { toDOM: () => HTMLElement } }).type.toDOM();
    expect(dom.tagName).toBe("BUTTON");
    expect(dom.getAttribute("aria-label")).toBe("Warning: vague");
  });

  test("the plugin takes findings from a meta and maps them over edits", () => {
    const plugin = qualityPlugin({ label: () => "x", onActivate: vi.fn() });
    let s = EditorState.create({ doc: state().doc, plugins: [plugin] });
    s = s.apply(s.tr.setMeta(qualityKey, [f([0, 1])]));
    expect(qualityKey.getState(s)!.find()).toHaveLength(2);
    s = s.apply(s.tr.insertText("Hello ", 1));
    const marks = qualityKey.getState(s)!.find();
    expect(marks).toHaveLength(2);
    expect(marks.some((d) => d.from > 7)).toBe(true);
  });
});
