// The editor screen's bar and its `/new` publish gate (M2b-2 final review, parked):
// the truncating way back carries its label as a `title`; `mod+shift+p` on a new item
// saves it first and asks its `/$id` screen to open the publish dialog, which opens there
// only when the account may publish or schedule the item.

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { SchemaContentDto } from "@/api/schema";
import { TooltipProvider } from "@/components/ui/tooltip";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { newBodyUploadQueue } from "./body-uploads";
import { EditorScreen, type EditorScreenProps } from "./editor";
import { metaOf } from "./editor-state";
import type { Handoff } from "./handoff";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";
const BODY = {
  type: "doc",
  content: [{ type: "paragraph", content: [{ type: "text", text: "Hello" }] }],
};

function itemWith(allowed: { publish: boolean; schedule: boolean }): SchemaContentDto {
  return {
    id: ID,
    type: "post",
    title: "Spring notes",
    slug: "spring-notes",
    status: "draft",
    body: BODY,
    excerpt: "",
    fields: {},
    seo: { title: null, description: null, canonical: null, robots: [] },
    terms: [],
    menu_order: 0,
    permalink: "/spring-notes/",
    author_id: "0190f1c0-0000-7000-8000-0000000000bb",
    author: { id: "0190f1c0-0000-7000-8000-0000000000bb", login: "me", display_name: "Me" },
    created_at: "2026-10-01T10:00:00Z",
    updated_at: "2026-10-01T10:00:00Z",
    version: 1,
    allowed: {
      edit: true,
      delete: false,
      restore: false,
      submit: false,
      trash: true,
      trash_undoable: true,
      unpublish: false,
      ...allowed,
    },
  };
}

function open(props: Partial<EditorScreenProps> & { item: SchemaContentDto | null }) {
  renderWithRouter(
    createElement(
      TooltipProvider,
      null,
      createElement(EditorScreen, {
        uploads: newBodyUploadQueue(),
        type: "post",
        typeLabel: "Posts",
        ...props,
      }),
    ),
  );
  // (`hidden`: a dialog open at once hides the screen from the accessibility tree.)
  return screen.findByRole("textbox", { name: "Body", hidden: true });
}

/** The hand-over of a just-created item whose `/new` screen asked for the dialog. */
function handoffOf(item: SchemaContentDto): Handoff {
  return {
    id: item.id,
    title: item.title,
    body: BODY,
    selection: null,
    focus: null,
    saved: { title: item.title, body: JSON.stringify(BODY) },
    meta: metaOf(item),
    openPublishDialog: true,
  };
}

const sideRoutes = {
  "/api/v1/meta": () => json({ site_url: "http://localhost:8080" }),
  "/api/v1/taxonomies": () => json({ items: [] }),
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("editor screen", () => {
  test("the_way_back_has_its_label_as_title", async () => {
    stubRoutes({});
    await open({ item: null });
    const back = screen.getByRole("link", { name: "Posts" });
    expect(back).toHaveAttribute("title", "Posts");
  });

  test("publish_shortcut_on_new_saves_then_asks_for_the_dialog", async () => {
    const created = itemWith({ publish: true, schedule: true });
    const posts: unknown[] = [];
    stubRoutes({
      "/api/v1/content/post": async (_url, req) => {
        posts.push(await req.json());
        return json(created, 201);
      },
    });
    const onCreated = vi.fn();
    await open({ item: null, onCreated });
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox", { name: "Title" }), "Spring notes");
    await user.keyboard("{Control>}{Shift>}p{/Shift}{/Control}");
    await waitFor(() => expect(onCreated).toHaveBeenCalledOnce());
    expect(posts).toHaveLength(1);
    expect(onCreated.mock.calls[0]?.[1]).toMatchObject({ openPublishDialog: true });
  });

  test("a_plain_first_save_does_not_ask_for_the_dialog", async () => {
    stubRoutes({
      "/api/v1/content/post": () => json(itemWith({ publish: true, schedule: true }), 201),
    });
    const onCreated = vi.fn();
    await open({ item: null, onCreated });
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox", { name: "Title" }), "Spring notes");
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(onCreated).toHaveBeenCalledOnce());
    expect(onCreated.mock.calls[0]?.[1]).toMatchObject({ openPublishDialog: false });
  });

  test("the_handed_over_dialog_opens_when_the_item_may_be_published", async () => {
    stubRoutes(sideRoutes);
    const item = itemWith({ publish: true, schedule: false });
    await open({ item, handoff: handoffOf(item) });
    await waitFor(() =>
      expect(document.querySelector("[data-slot=publish-dialog]")).not.toBeNull(),
    );
  });

  test("the_handed_over_dialog_stays_shut_without_publish_or_schedule", async () => {
    stubRoutes(sideRoutes);
    const item = itemWith({ publish: false, schedule: false });
    await open({ item, handoff: handoffOf(item) });
    // The dialog's open state is the screen's initial state (no effect, no request), so
    // it would be there once the screen has rendered.
    expect(document.querySelector("[data-slot=publish-dialog]")).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
