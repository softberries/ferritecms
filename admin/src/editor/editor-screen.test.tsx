// The editor screen's bar and its `/new` publish gate (M2b-2 final review, parked):
// the truncating way back carries its label as a `title`; `mod+shift+p` on a new item
// saves it first and asks its `/$id` screen to open the publish dialog, which opens there
// only when the account may publish or schedule the item.

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { type ReactNode, createElement, useState } from "react";
import { ShellChromeContext } from "@/shell/chrome-context";
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
    locale: "en",
    translation_group: "0190f1c0-0000-7000-8000-0000000000aa",
    translations: [],
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
      needs_approval: false,
      stage: false,
      override: false,
      unpublish: false,
      ...allowed,
    },
  };
}

/** The shell's focus mode, as the real shell keeps it. */
function Chrome({ children }: { children: ReactNode }) {
  const [focusMode, setFocusMode] = useState(false);
  return createElement(
    ShellChromeContext.Provider,
    { value: { focusMode, setFocusMode } },
    children,
  );
}

function open(props: Partial<EditorScreenProps> & { item: SchemaContentDto | null }) {
  renderWithRouter(
    createElement(
      Chrome,
      null,
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

  test("the_live_preview_toggles_with_mod_alt_p_and_focus_mode_hides_it", async () => {
    // Below 1280 px (jsdom has no `matchMedia` match): the bar's Write | Preview control.
    stubRoutes({
      ...sideRoutes,
      [`/api/v1/content/post/${ID}/preview-links`]: () =>
        json(
          {
            id: "l1",
            token: `ferp_${"a".repeat(43)}`,
            url: `http://localhost/_preview/link/ferp_${"a".repeat(43)}`,
            target_kind: "item",
            target_id: ID,
            created_by: "u",
            created_at: "2026-10-01T10:00:00Z",
            expires_at: new Date(Date.now() + 3_600_000).toISOString(),
          },
          201,
        ),
    });
    await open({ item: itemWith({ publish: true, schedule: true }) });
    const user = userEvent.setup();
    const write = screen.getByRole("radio", { name: "Write" });
    expect(write).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{Control>}{Alt>}p{/Alt}{/Control}");
    const frame = await screen.findByTitle("Preview of Spring notes");
    expect(screen.getByRole("radio", { name: "Preview" })).toHaveAttribute("aria-checked", "true");
    // The writing surface stays mounted (the editor's view lives in it), hidden.
    expect(screen.getByRole("textbox", { name: "Body", hidden: true })).not.toBeVisible();
    await waitFor(() => expect(frame.getAttribute("src")).toMatch(/working=1/));
    // Focus mode hides the pane and shows the text again.
    await user.keyboard("{Control>}{Shift>}f{/Shift}{/Control}");
    expect(screen.queryByTitle("Preview of Spring notes")).toBeNull();
    await user.keyboard("{Escape}");
    await screen.findByTitle("Preview of Spring notes");
    await user.click(screen.getByRole("radio", { name: "Write" }));
    expect(screen.queryByTitle("Preview of Spring notes")).toBeNull();
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
