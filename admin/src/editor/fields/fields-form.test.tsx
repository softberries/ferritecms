// The custom fields as part of the editor's draft (Review Focus 2): stored values the
// schema no longer has are listed and dropped from the save; a required field left empty
// stops the save before any request; a server `422` path lands on its input.

import { EditorContent } from "@tiptap/react";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { SchemaContentDto } from "@/api/schema";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { type EditorDocument, useEditorDocument } from "../editor-state";
import type { FieldDef } from "./fields-model";
import { FieldsSection } from "./fields-section";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";
const PATH = `/api/v1/content/book/${ID}`;

function item(fields: Record<string, unknown>, version = 3): SchemaContentDto {
  return {
    id: ID,
    type: "book",
    title: "A book",
    slug: "a-book",
    status: "draft",
    body: { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "x" }] }] },
    excerpt: "",
    fields,
    seo: { title: null, description: null, canonical: null, robots: [] },
    terms: [],
    menu_order: 0,
    permalink: "/book/a-book/",
    author_id: "0190f1c0-0000-7000-8000-0000000000bb",
    author: { id: "0190f1c0-0000-7000-8000-0000000000bb", login: "me", display_name: "Me" },
    created_at: "2026-10-01T10:00:00Z",
    updated_at: "2026-10-01T10:00:00Z",
    version,
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
  };
}

const def = (name: string, kind: FieldDef["kind"], extra: Partial<FieldDef> = {}): FieldDef => ({
  name,
  label: name[0]!.toUpperCase() + name.slice(1),
  kind,
  required: false,
  options: [],
  fields: [],
  ...extra,
});

let doc: EditorDocument | null = null;
const d = () => doc as EditorDocument;

function Harness({ start, defs }: { start: SchemaContentDto; defs: FieldDef[] }) {
  const document = useEditorDocument({
    type: "book",
    item: start,
    placeholder: "",
    bodyLabel: "Body",
    fieldDefs: defs,
  });
  useEffect(() => {
    doc = document;
  });
  return (
    <>
      <EditorContent editor={document.editor} />
      <FieldsSection doc={document} />
    </>
  );
}

/** Routes for the item: PATCH answers with `answer(body)`; the slot is empty. */
function routes(answer: (body: Record<string, unknown>) => Response) {
  const patches: Record<string, unknown>[] = [];
  const seen = stubRoutes({
    [PATH]: async (_url, req) => {
      expect(req.method).toBe("PATCH");
      const body = (await req.json()) as Record<string, unknown>;
      patches.push(body);
      return answer(body);
    },
    [`${PATH}/autosave`]: () => new Response(null, { status: 204 }),
    [`${PATH}/revisions`]: () => json({ items: [] }),
  });
  return { patches, seen };
}

afterEach(() => {
  doc = null;
  vi.unstubAllGlobals();
});

describe("fields_form_test", () => {
  test("stale_keys_are_listed_and_dropped", async () => {
    const defs = [def("subtitle", "text")];
    const { patches } = routes((body) => json(item(body.fields as Record<string, unknown>, 4)));
    renderWithRouter(
      <Harness
        start={item({ subtitle: "Kept", isbn: "978-3", featured_image: null })}
        defs={defs}
      />,
    );
    const notice = await screen.findByRole("note");
    expect(within(notice).getByText("isbn")).toBeInTheDocument();
    expect(notice).toHaveTextContent("Not in the type any more: isbn, removed when you save");
    // The notice alone changes nothing.
    expect(d().dirty).toBe(false);

    // Any save (here the title alone) sends the fields without the stale key.
    act(() => d().setTitle("A renamed book"));
    await act(async () => {
      await d().save();
    });
    expect(patches).toHaveLength(1);
    expect(patches[0]).toMatchObject({ title: "A renamed book" });
    expect(patches[0]?.fields).toEqual({ subtitle: "Kept", featured_image: null });
    await waitFor(() => {
      expect(screen.queryByRole("note")).toBeNull();
    });
  });

  test("stale_row_keys_are_listed_and_dropped", async () => {
    // A repeater lost a sub-field: its rows still hold it.
    const defs = [def("chapters", "repeater", { fields: [def("title", "text")] })];
    const { patches, seen } = routes((body) =>
      json(item(body.fields as Record<string, unknown>, 4)),
    );
    renderWithRouter(
      <Harness
        start={item({
          chapters: [
            { title: "One", pages: 12 },
            { title: "Two", pages: 3 },
          ],
        })}
        defs={defs}
      />,
    );
    const notice = await screen.findByRole("note");
    expect(notice).toHaveTextContent(
      "Not in the type any more: chapters[].pages, removed when you save",
    );
    act(() => d().setTitle("Renamed"));
    let result: unknown = null;
    await act(async () => {
      result = await d().save();
    });
    // Saved (nothing refused, nothing unexplained), without the stale sub-key.
    expect(result).not.toBeNull();
    expect(seen.filter((u) => u.pathname === PATH)).toHaveLength(1);
    expect(patches[0]?.fields).toEqual({ chapters: [{ title: "One" }, { title: "Two" }] });
  });

  test("required_blocks_save", async () => {
    const defs = [def("isbn", "text", { required: true }), def("pages", "number")];
    const { seen } = routes(() => json(item({}, 4)));
    renderWithRouter(<Harness start={item({})} defs={defs} />);
    const isbn = await screen.findByRole("textbox", { name: /Isbn/ });
    // Untouched, a required field says nothing yet.
    expect(isbn).not.toHaveAttribute("aria-invalid");

    act(() => d().setTitle("Edited"));
    let result: unknown = "pending";
    await act(async () => {
      result = await d().save();
    });
    // Nothing was sent; the field says why.
    expect(result).toBeNull();
    expect(seen.filter((u) => u.pathname === PATH)).toEqual([]);
    expect(isbn).toHaveAttribute("aria-invalid", "true");
    expect(isbn).toHaveAccessibleDescription(/Required/);

    // Filled in, the save goes through.
    await userEvent.setup().type(isbn, "978-3");
    expect(isbn).not.toHaveAttribute("aria-invalid");
    await act(async () => {
      result = await d().save();
    });
    expect(result).not.toBeNull();
  });

  test("server_422_path_shows_on_its_input", async () => {
    const defs = [def("price", "number"), def("subtitle", "text")];
    const problem = {
      type: "https://ferrite.dev/problems/validation",
      title: "Validation failed",
      status: 422,
      detail: "fields.price: must be a number",
      errors: [{ field: "fields", detail: "fields.price: must be a number" }],
    };
    routes(
      () =>
        new Response(JSON.stringify(problem), {
          status: 422,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    renderWithRouter(<Harness start={item({ price: 3 })} defs={defs} />);
    const price = await screen.findByRole("textbox", { name: /Price/ });
    const subtitle = screen.getByRole("textbox", { name: /Subtitle/ });
    act(() => d().setTitle("Edited"));
    await act(async () => {
      await d().save();
    });
    expect(price).toHaveAttribute("aria-invalid", "true");
    expect(price).toHaveAccessibleDescription(/Must be a number/);
    expect(subtitle).not.toHaveAttribute("aria-invalid");
    // Editing the input clears the server's message.
    await userEvent.setup().type(price, "4");
    expect(price).not.toHaveAttribute("aria-invalid");
  });

  test("a_relation_of_the_wrong_type_is_shown_at_its_field", async () => {
    // P52: core refuses a value whose item is of a type the field does not list, at the
    // value's path (`invalid_key`); the relation's trigger says why.
    const defs = [def("related", "relation", { types: ["page"] })];
    const POST_ID = "0190f1c0-0000-7000-8000-0000000000dd";
    const detail = 'fields.related: must be an item of type "page"';
    const problem = {
      type: "https://ferrite.dev/problems/validation",
      title: "Validation failed",
      status: 422,
      detail,
      errors: [{ field: "fields.related", detail }],
    };
    routes(
      () =>
        new Response(JSON.stringify(problem), {
          status: 422,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    renderWithRouter(<Harness start={item({ related: POST_ID })} defs={defs} />);
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => d().setTitle("Edited"));
    await act(async () => {
      await d().save();
    });
    const trigger = await waitFor(() => {
      const el = document.querySelector("[data-slot=relation-trigger]");
      if (!el) throw new Error("no relation trigger");
      return el;
    });
    await waitFor(() => expect(trigger).toHaveAttribute("aria-invalid", "true"));
    expect(trigger).toHaveAccessibleDescription(/must be an item of type "page"/i);
  });
});
