// The editor's document model against the core renderer's (Review Focus 3): every node
// and mark the renderer knows survives load → `getJSON()` unchanged except for the
// attribute defaults listed here, a body with anything else opens read-only, and the
// schema holds exactly the supported sets.

import { EditorContent, useEditor, type Editor } from "@tiptap/react";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { SchemaContentDto } from "@/api/schema";
import { TooltipProvider } from "@/components/ui/tooltip";
import { renderWithRouter, stubRoutes } from "@/test/render";
import { EditorScreen } from "./editor";
import {
  SUPPORTED_MARKS,
  SUPPORTED_NODES,
  checkHref,
  displayBody,
  editorExtensions,
  validateBody,
} from "./schema";
import everyNode from "./fixtures/every-node.json";
import seedSample from "./fixtures/seed-sample.json";
import unknownNode from "./fixtures/unknown-node.json";

/**
 * The only changes `getJSON()` may make to a stored body: TipTap fills in attribute
 * defaults the renderer ignores (pre-flight F32 lists what may appear). Never `target`
 * (links), never a heading level, never a node or a mark.
 */
const DOCUMENTED_DEFAULTS = [
  "image.width=null",
  "image.height=null",
  "tableHeader.colwidth=null",
  "tableCell.colwidth=null",
  // TipTap 3 cells carry a text alignment; the renderer ignores it (no `align` output).
  "tableHeader.align=null",
  "tableCell.align=null",
  "orderedList.type=null",
];

afterEach(() => {
  vi.unstubAllGlobals();
});

/** A mounted editor holding `content` (React, so node views render as in the app). */
function mount(content: unknown): Editor {
  const holder: { editor: Editor | null } = { editor: null };
  function Harness() {
    const editor = useEditor({
      extensions: editorExtensions({
        placeholder: "Write",
        resolveMedia: () => Promise.resolve("/media/x.png"),
      }),
      content: content as object,
      immediatelyRender: true,
      shouldRerenderOnTransaction: false,
      injectCSS: false,
      enableContentCheck: true,
      onContentError: ({ error }) => {
        throw error;
      },
    });
    holder.editor = editor;
    return <EditorContent editor={editor} />;
  }
  render(<Harness />);
  if (!holder.editor) throw new Error("no editor");
  return holder.editor;
}

/** What a save would send: `getJSON()` through JSON (as the request body is). */
const wire = (editor: Editor): unknown => JSON.parse(JSON.stringify(editor.getJSON())) as unknown;

type Json = Record<string, unknown>;
const isObj = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * Removes from `out` every attribute `input` does not have, recording each as
 * `type.attr=value`, so what remains can be compared with the input exactly.
 */
function stripAdded(out: unknown, input: unknown, added: string[]): unknown {
  if (Array.isArray(out) && Array.isArray(input)) {
    return out.map((o, i) => stripAdded(o, input[i], added));
  }
  if (!isObj(out) || !isObj(input)) return out;
  const result: Json = {};
  for (const [k, v] of Object.entries(out)) {
    if (k === "attrs" && isObj(v)) {
      const given = isObj(input.attrs) ? input.attrs : {};
      const kept: Json = {};
      for (const [ak, av] of Object.entries(v)) {
        if (ak in given) kept[ak] = av;
        else added.push(`${String(out.type)}.${ak}=${JSON.stringify(av)}`);
      }
      if (Object.keys(kept).length > 0 || "attrs" in input) result.attrs = kept;
    } else {
      result[k] = stripAdded(v, input[k], added);
    }
  }
  return result;
}

/** Every node type and mark type in a document, with the attribute names they carry. */
function inventory(doc: unknown): { nodes: Set<string>; marks: Set<string>; attrs: Set<string> } {
  const nodes = new Set<string>();
  const marks = new Set<string>();
  const attrs = new Set<string>();
  const walk = (v: unknown) => {
    if (Array.isArray(v)) return v.forEach(walk);
    if (!isObj(v)) return;
    nodes.add(String(v.type));
    if (isObj(v.attrs)) for (const a of Object.keys(v.attrs)) attrs.add(`${String(v.type)}.${a}`);
    if (Array.isArray(v.marks)) {
      for (const m of v.marks as Json[]) {
        marks.add(String(m.type));
        if (isObj(m.attrs))
          for (const a of Object.keys(m.attrs)) attrs.add(`${String(m.type)}.${a}`);
      }
    }
    walk(v.content);
  };
  walk(doc);
  return { nodes, marks, attrs };
}

describe("schema", () => {
  test("schema_is_exactly_the_renderer_set", () => {
    const editor = mount(everyNode);
    expect(Object.keys(editor.schema.nodes).sort()).toEqual([...SUPPORTED_NODES].sort());
    expect(Object.keys(editor.schema.marks).sort()).toEqual([...SUPPORTED_MARKS].sort());
  });

  test("roundtrip_every_renderer_node", () => {
    // The fixture covers every node, every mark and every attribute `render.rs` reads.
    const { nodes, marks, attrs } = inventory(everyNode);
    expect([...nodes].sort()).toEqual([...SUPPORTED_NODES].sort());
    expect([...marks].sort()).toEqual([...SUPPORTED_MARKS].sort());
    for (const read of [
      "heading.level",
      "orderedList.start",
      "codeBlock.language",
      "image.src",
      "image.alt",
      "image.title",
      "tableHeader.colspan",
      "tableHeader.rowspan",
      "tableCell.colspan",
      "tableCell.rowspan",
      "html.html",
      "link.href",
      "link.target",
    ]) {
      expect(attrs, read).toContain(read);
    }
    expect(validateBody(everyNode)).toEqual({ ok: true });

    const editor = mount(everyNode);
    const added: string[] = [];
    const stripped = stripAdded(wire(editor), everyNode, added);
    expect(stripped).toStrictEqual(everyNode);
    expect([...new Set(added)].sort()).toEqual([...DOCUMENTED_DEFAULTS].sort());
  });

  test("seed_sample_roundtrips", () => {
    // `seed demo`'s post body (`crates/ferrite/src/seed/demo.rs` `post_body`).
    expect(validateBody(seedSample)).toEqual({ ok: true });
    const editor = mount(seedSample);
    const added: string[] = [];
    expect(stripAdded(wire(editor), seedSample, added)).toStrictEqual(seedSample);
    for (const a of added) expect(DOCUMENTED_DEFAULTS).toContain(a);
  });

  test("a_stored_link_without_target_gets_none", () => {
    // F32: TipTap's link would add `target: "_blank"`; the site would then open it in a
    // new tab.
    const editor = mount(seedSample);
    expect(JSON.stringify(wire(editor))).not.toContain("target");
  });

  test("heading_1_is_unknown", () => {
    const h = (attrs?: object) => ({
      type: "doc",
      content: [
        { type: "heading", ...(attrs ? { attrs } : {}), content: [{ type: "text", text: "T" }] },
      ],
    });
    expect(validateBody(h({ level: 1 }))).toEqual({ ok: false, unknown: ["heading (level 1)"] });
    expect(validateBody(h())).toEqual({ ok: false, unknown: ["heading (level none)"] });
    expect(validateBody(h({ level: 7 }))).toEqual({ ok: false, unknown: ["heading (level 7)"] });
    expect(validateBody(h({ level: 2 }))).toEqual({ ok: true });
  });

  test("validate_body_reports_unknown_nodes_and_marks", () => {
    expect(validateBody(unknownNode)).toEqual({ ok: false, unknown: ["callout"] });
    expect(
      validateBody({
        type: "doc",
        content: [
          {
            type: "paragraph",
            content: [{ type: "text", text: "u", marks: [{ type: "underline" }] }],
          },
        ],
      }),
    ).toEqual({ ok: false, unknown: ["underline (mark)"] });
    expect(validateBody(null)).toEqual({ ok: true });
    expect(validateBody({})).toEqual({ ok: true });
    expect(validateBody({ type: "doc", content: [] })).toEqual({ ok: true });
  });

  test("display_body_keeps_unknown_text_visible", () => {
    const shown = displayBody(unknownNode);
    expect(validateBody(shown)).toEqual({ ok: true });
    expect(JSON.stringify(shown)).toContain("A note from another editor.");
  });

  test("link_href_policy", () => {
    for (const ok of [
      "https://a.example/x",
      "http://a.b",
      "mailto:a@b.c",
      "/about/",
      "#top",
      "page/2",
    ]) {
      expect(checkHref(ok), ok).toEqual({ ok: true, href: ok });
    }
    expect(checkHref("example.com/path")).toEqual({ ok: true, href: "https://example.com/path" });
    for (const bad of [
      "javascript:alert(1)",
      " JavaScript:alert(1)",
      "java\tscript:alert(1)",
      "data:text/html,x",
      "vbscript:x",
      "//evil.example",
      "/\\evil",
      "&#x6a;avascript:x",
      "",
      "   ",
    ]) {
      expect(checkHref(bad).ok, bad).toBe(false);
    }
  });
});

function itemWith(body: unknown, extra: Partial<SchemaContentDto> = {}): SchemaContentDto {
  return {
    id: "0190f1c0-0000-7000-8000-0000000000aa",
    type: "post",
    title: "Imported",
    slug: "imported",
    status: "draft",
    body,
    excerpt: "",
    fields: {},
    terms: [],
    menu_order: 0,
    permalink: "/imported/",
    author_id: "0190f1c0-0000-7000-8000-0000000000bb",
    author: { id: "0190f1c0-0000-7000-8000-0000000000bb", login: "me", display_name: "Me" },
    created_at: "2026-10-01T10:00:00Z",
    updated_at: "2026-10-01T10:00:00Z",
    version: 3,
    allowed: {
      edit: true,
      delete: false,
      publish: true,
      restore: false,
      schedule: true,
      submit: false,
      trash: true,
      trash_undoable: true,
      unpublish: false,
    },
    ...extra,
  };
}

async function openScreen(item: SchemaContentDto): Promise<void> {
  renderWithRouter(
    <TooltipProvider>
      <EditorScreen type="post" typeLabel="Posts" item={item} />
    </TooltipProvider>,
  );
  await screen.findByRole("textbox", { name: "Body" });
}

describe("editor screen", () => {
  test("unknown_node_opens_read_only", async () => {
    // Any request (a save) fails the test: the stub knows no route.
    const seen = stubRoutes({});
    await openScreen(itemWith(unknownNode));

    const body = screen.getByRole("textbox", { name: "Body" });
    expect(body).toHaveAttribute("contenteditable", "false");
    expect(screen.getByRole("note")).toHaveTextContent(
      "This item contains content this editor cannot edit safely: callout. It is shown read-only so nothing is lost.",
    );
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    // The unknown block's text is still shown.
    expect(body).toHaveTextContent("A note from another editor.");

    const user = userEvent.setup();
    await user.keyboard("{Control>}s{/Control}{Meta>}s{/Meta}");
    await act(() => Promise.resolve());
    expect(seen).toEqual([]);
  });

  test("forbidden_item_opens_read_only", async () => {
    stubRoutes({});
    const item = itemWith(seedSample);
    await openScreen({ ...item, allowed: { ...item.allowed, edit: false } });
    expect(screen.getByRole("textbox", { name: "Body" })).toHaveAttribute(
      "contenteditable",
      "false",
    );
    expect(screen.getByRole("note")).toHaveTextContent("You can read this item but not edit it.");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  test("supported_item_is_editable", async () => {
    stubRoutes({});
    await openScreen(itemWith(seedSample));
    expect(screen.getByRole("textbox", { name: "Body" })).toHaveAttribute(
      "contenteditable",
      "true",
    );
    expect(screen.queryByRole("note")).toBeNull();
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });
});
