// The type screen's minors and relation target types (M2b-3 Task 12): the schema-change
// confirm says what each kind of change does (removed select options and narrowed
// relations included); a built-in type's delete says it cannot be done instead of
// offering it; a relation's target types are edited as checkboxes and saved with the
// schema; the server's refusal of them lands at the field; a type another relation
// points at is refused with that reason.

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { ContentTypeDto } from "@/api/types";
import type { FieldDef } from "@/editor/fields/fields-model";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { TypeForm } from "./type-form";

const typeOf = (
  name: string,
  fields: FieldDef[] | null,
  extra: Partial<ContentTypeDto> = {},
): ContentTypeDto => ({
  name,
  label: name[0]!.toUpperCase() + name.slice(1),
  built_in: false,
  hierarchical: false,
  has_archive: false,
  fields,
  ...extra,
});

const def = (name: string, kind: FieldDef["kind"], extra: Partial<FieldDef> = {}): FieldDef => ({
  name,
  label: name[0]!.toUpperCase() + name.slice(1),
  kind,
  required: false,
  options: [],
  fields: [],
  ...extra,
});

const ALL = [
  typeOf("post", null, { built_in: true, label: "Posts" }),
  typeOf("page", null, { built_in: true, label: "Pages" }),
  typeOf("media", null, { built_in: true, label: "Media" }),
];

function problem(status: number, detail: string, field?: string): Response {
  return new Response(
    JSON.stringify({
      type: "https://ferritecms.dev/problems/validation",
      title: "Validation failed",
      status,
      detail,
      ...(field ? { errors: [{ field, detail }] } : {}),
    }),
    { status, headers: { "Content-Type": "application/problem+json" } },
  );
}

/** The type's routes: list, re-read (stale guard) and `PATCH` (recorded). */
function routes(dto: ContentTypeDto, patch?: (body: unknown) => Response) {
  const patches: unknown[] = [];
  stubRoutes({
    "/api/v1/types": () => json({ items: [...ALL, dto] }),
    [`/api/v1/types/${dto.name}`]: async (_url, req) => {
      if (req.method === "PATCH") {
        const body: unknown = await req.json();
        patches.push(body);
        return patch ? patch(body) : json({ ...dto, ...(body as object) });
      }
      if (req.method === "DELETE") {
        return problem(422, 'type is a relation target of type "shelf"');
      }
      return json(dto);
    },
  });
  return patches;
}

async function mount(dto: ContentTypeDto) {
  renderWithRouter(<TypeForm initial={dto} />);
  await screen.findByRole("button", { name: "Save changes" });
}

const row = (name: string) =>
  document.querySelector<HTMLElement>(`[data-slot=builder-row][data-name="${name}"]`)!;
const openRow = async (user: ReturnType<typeof userEvent.setup>, name: string) => {
  await user.click(row(name).querySelector<HTMLElement>(":scope > button[aria-expanded]")!);
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("type form", () => {
  test("the_change_dialog_says_what_each_kind_does", async () => {
    const dto = typeOf("book", [
      def("genre", "select", { options: ["fiction", "essay", "poetry"] }),
      def("subtitle", "text"),
    ]);
    routes(dto);
    await mount(dto);
    const user = userEvent.setup();
    await openRow(user, "genre");
    await user.click(within(row("genre")).getByRole("button", { name: "Remove option 2" }));
    await openRow(user, "subtitle");
    await user.click(within(row("subtitle")).getByRole("switch", { name: /Required/ }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    const dialog = await screen.findByRole("alertdialog");
    const options = within(dialog).getByText("Options removed").closest("section")!;
    expect(options).toHaveTextContent("genre: essay");
    expect(options).toHaveTextContent(
      "Items holding a removed option cannot be saved until another is chosen.",
    );
    const required = dialog.querySelector("[data-change=newlyRequired]")!;
    expect(required).toHaveTextContent("subtitle");
    expect(required).toHaveTextContent("Items without a value cannot be saved until they get one.");
    // Only the kinds this change has, each with its own words.
    expect(dialog.querySelector("[data-change=removed]")).toBeNull();
    expect(dialog).not.toHaveTextContent("and are then removed");
  });

  test("a_built_in_type_says_it_cannot_be_deleted", async () => {
    const dto = typeOf("post", null, { built_in: true, label: "Posts" });
    routes(dto);
    await mount(dto);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Delete type" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Built-in types cannot be deleted.");
    expect(dialog).not.toHaveTextContent("removed for good");
    expect(within(dialog).queryByRole("button", { name: "Delete type" })).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  test("a_type_another_relation_points_at_is_refused_with_that_reason", async () => {
    const dto = typeOf("book", []);
    routes(dto);
    await mount(dto);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Delete type" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Delete type" }),
    );
    const refused = await waitFor(() => {
      const el = document.querySelector("[data-slot=delete-refused]");
      if (!el) throw new Error("not refused yet");
      return el;
    });
    expect(refused).toHaveTextContent(
      "A relation field of the type shelf points at it: remove it there first.",
    );
  });

  test("relation_types_are_edited_and_saved_with_the_schema", async () => {
    const dto = typeOf("book", [def("related", "relation")]);
    const patches = routes(dto);
    await mount(dto);
    const user = userEvent.setup();
    await openRow(user, "related");
    const targets = await within(row("related")).findByRole("group", { name: "Can point at" });
    // Every content type but media, this one included.
    expect(
      within(targets)
        .getAllByRole("checkbox")
        .map((c) => c.getAttribute("aria-label") ?? c.closest("li")?.textContent),
    ).toHaveLength(3);
    await user.click(within(targets).getByRole("checkbox", { name: /Pages/ }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    // Any type → pages only: stored values may point elsewhere, so it asks.
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.querySelector("[data-change=targetsNarrowed]")).toHaveTextContent("related");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(patches).toHaveLength(1));
    expect((patches[0] as { fields: FieldDef[] }).fields).toEqual([
      def("related", "relation", { types: ["page"] }),
    ]);
  });

  test("a_server_refusal_of_the_types_shows_at_the_field", async () => {
    // A type deleted elsewhere since the screen loaded: the server names it at the field.
    const dto = typeOf("book", [def("related", "relation")]);
    routes(dto, () => problem(422, 'fields.related: type "page" is not a content type', "fields"));
    await mount(dto);
    const user = userEvent.setup();
    await openRow(user, "related");
    const targets = await within(row("related")).findByRole("group", { name: "Can point at" });
    await user.click(within(targets).getByRole("checkbox", { name: /Pages/ }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save changes" }),
    );
    await waitFor(() =>
      expect(within(row("related")).getByText('type "page" is not a content type')).toBeVisible(),
    );
    expect(document.querySelector("[data-slot=save-failure]")).toBeNull();
  });
});
