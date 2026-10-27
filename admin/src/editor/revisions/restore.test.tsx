// Restoring a revision from the editor screen (Task 17 fix round 1): the editor is
// locked while the restore runs and a progress toast says so; after the restore, the
// Undo goes away as soon as the item is saved again, and an Undo that still runs sends
// the restore's own version as its `If-Match` (a later save answers 412).

import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { Toaster, toast } from "sonner";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { SchemaContentDto, SchemaRevisionDto } from "@/api/schema";
import { TooltipProvider } from "@/components/ui/tooltip";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { EditorScreen, type EditorScreenProps } from "../editor";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";
const R0 = "0190f1c2-6d2e-7a51-9a3b-1c2d3e4f5a60";
const R1 = "0190f1c2-6d2e-7a51-9a3b-1c2d3e4f5a61";
const R2 = "0190f1c2-6d2e-7a51-9a3b-1c2d3e4f5a62";
const AUTHOR = { id: "0190f1c0-0000-7000-8000-0000000000bb", login: "me", display_name: "Me" };
const BASE = `/api/v1/content/post/${ID}`;

const para = (text: string) => ({
  type: "doc",
  content: [{ type: "paragraph", content: [{ type: "text", text }] }],
});

function itemWith(extra: Partial<SchemaContentDto> = {}): SchemaContentDto {
  return {
    id: ID,
    type: "post",
    title: "Spring notes",
    slug: "spring-notes",
    status: "draft",
    body: para("The newer paragraph."),
    excerpt: "An excerpt",
    fields: {},
    seo: { title: null, description: null },
    terms: [],
    menu_order: 0,
    permalink: "/spring-notes/",
    author_id: AUTHOR.id,
    author: AUTHOR,
    created_at: "2026-10-01T10:00:00Z",
    updated_at: "2026-10-01T10:00:00Z",
    version: 4,
    allowed: {
      edit: true,
      delete: false,
      publish: true,
      restore: false,
      schedule: false,
      submit: false,
      trash: true,
      trash_undoable: true,
      unpublish: false,
    },
    ...extra,
  };
}

function summary(id: string, at: string, note: string | null = null) {
  return { id, created_at: at, note, title: "Spring notes", author_id: AUTHOR.id, author: AUTHOR };
}

function revision(id: string, text: string): SchemaRevisionDto {
  return {
    ...summary(id, "2026-10-01T09:00:00Z"),
    body: para(text),
    excerpt: "An excerpt",
    fields: {},
  };
}

function openScreen(item: SchemaContentDto, extra: Partial<EditorScreenProps> = {}) {
  renderWithRouter(
    createElement(
      TooltipProvider,
      null,
      createElement(EditorScreen, { type: "post", typeLabel: "Posts", item, ...extra }),
      createElement(Toaster),
    ),
  );
  return screen.findByRole("textbox", { name: "Body" });
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("revision restore", () => {
  test("restore_locks_the_editor_until_it_lands", async () => {
    const loading = vi.spyOn(toast, "loading");
    let land: (r: Response) => void = () => undefined;
    const posts: (string | null)[] = [];
    stubRoutes({
      [`${BASE}/revisions`]: () =>
        json({ items: [summary(R1, "2026-10-01T10:00:00Z"), summary(R0, "2026-10-01T09:00:00Z")] }),
      [`${BASE}/revisions/${R0}`]: () => json(revision(R0, "The older paragraph.")),
      [`${BASE}/revisions/${R0}/restore`]: (_url, req) => {
        posts.push(req.headers.get("If-Match"));
        return new Promise<Response>((resolve) => {
          land = resolve;
        });
      },
    });
    const onReopen = vi.fn();
    const body = await openScreen(itemWith(), { onReopen });
    const user = userEvent.setup();
    // (jsdom has no layout for a click into ProseMirror: the shortcut from the page.)
    expect(body).toBeInTheDocument();
    await user.keyboard("{Control>}{Alt>}r{/Alt}{/Control}");
    // (Two: the header's and the phone footer's; jsdom applies no media queries.)
    const [restore] = await screen.findAllByRole("button", { name: "Restore this version" });
    if (!restore) throw new Error("no Restore button");
    await waitFor(() => expect(restore).toBeEnabled());
    await user.click(restore);

    // In flight: the progress toast, and nothing can be typed or saved.
    await waitFor(() => expect(posts).toEqual(['"4"']));
    expect(loading).toHaveBeenCalledWith(
      "Restoring…",
      expect.objectContaining({ id: "editor-revision" }),
    );
    expect(document.querySelector("[data-slot=editor-title]")).toHaveAttribute("readonly");
    expect(document.querySelector(".editor-body")).toHaveAttribute("contenteditable", "false");

    land(json(itemWith({ version: 5, body: para("The older paragraph.") })));
    await waitFor(() => expect(onReopen).toHaveBeenCalledOnce());
    expect(onReopen.mock.calls[0]?.[0]).toMatchObject({
      item: { version: 5 },
      revision: { kind: "restored", rid: R0, version: 5 },
    });
  });

  test("undo_goes_once_the_item_is_saved_again", async () => {
    const success = vi.spyOn(toast, "success");
    const dismiss = vi.spyOn(toast, "dismiss");
    const undoPosts: (string | null)[] = [];
    stubRoutes({
      [`${BASE}/revisions`]: () =>
        json({
          items: [
            summary(R2, "2026-10-01T10:05:00Z", `restored from ${R0}`),
            summary(R1, "2026-10-01T10:00:00Z"),
          ],
        }),
      [BASE]: async (_url, req) => {
        const patch = (await req.json()) as { title?: string };
        return json(itemWith({ version: 6, title: patch.title ?? "Spring notes" }));
      },
      [`${BASE}/revisions/${R1}/restore`]: (_url, req) => {
        undoPosts.push(req.headers.get("If-Match"));
        return new Response(
          JSON.stringify({
            type: "https://ferritecms.dev/problems/precondition-failed",
            title: "Precondition failed",
            status: 412,
            current: itemWith({ version: 6, title: "Saved after the restore" }),
          }),
          { status: 412, headers: { "Content-Type": "application/problem+json" } },
        );
      },
    });
    await openScreen(itemWith({ version: 5 }), {
      revision: { kind: "restored", rid: R0, at: "2026-10-01T09:00:00Z", version: 5 },
      onReopen: vi.fn(),
    });
    // The restored screen offers Undo (to the revision after the restore's snapshot).
    await waitFor(() =>
      expect(success).toHaveBeenCalledWith(
        "Version restored",
        expect.objectContaining({
          action: expect.objectContaining({ label: "Undo" }) as unknown,
        }),
      ),
    );
    const options = success.mock.calls.at(-1)?.[1] as {
      action: { onClick: (e: unknown) => void };
    };

    // A save after the restore: the offer ends.
    dismiss.mockClear();
    const user = userEvent.setup();
    const title = screen.getByRole("textbox", { name: "Title" });
    await user.type(title, " tweaked");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(dismiss).toHaveBeenCalledWith("editor-revision"));

    // An Undo that still ran (a click racing the save) goes with the restore's version:
    // the server refuses it, nothing saved since is reverted.
    options.action.onClick({ preventDefault: () => undefined });
    await waitFor(() => expect(undoPosts).toEqual(['"5"']));
  });

  test("undo_keeps_its_toast_for_the_progress", async () => {
    // CI run 40404431115: sonner removes a toast 200 ms after its action button is
    // clicked unless the click's default is prevented. The Undo's progress ("Restoring…")
    // and its result ("Restore undone", after the remount) reuse the toast's id, so they
    // landed in the toast on its way out and went with it.
    const undoPosts: (string | null)[] = [];
    stubRoutes({
      [`${BASE}/revisions`]: () =>
        json({
          items: [
            summary(R2, "2026-10-01T10:05:00Z", `restored from ${R0}`),
            summary(R1, "2026-10-01T10:00:00Z"),
          ],
        }),
      [`${BASE}/revisions/${R1}/restore`]: (_url, req) => {
        undoPosts.push(req.headers.get("If-Match"));
        return new Promise<Response>(() => undefined);
      },
    });
    await openScreen(itemWith({ version: 5 }), {
      revision: { kind: "restored", rid: R0, at: "2026-10-01T09:00:00Z", version: 5 },
      onReopen: vi.fn(),
    });
    const toastEl = () => document.querySelector("[data-sonner-toast]");
    await waitFor(() => expect(toastEl()).toHaveTextContent("Version restored"));
    // (A plain click: sonner's swipe handler wants pointer capture, which jsdom lacks.)
    fireEvent.click(await screen.findByRole("button", { name: "Undo" }));
    await waitFor(() => expect(undoPosts).toEqual(['"5"']));
    // Past sonner's 200 ms exit: the same toast now says what runs, without the old
    // description or a second Undo.
    await new Promise((r) => setTimeout(r, 300));
    await waitFor(() => expect(toastEl()).toHaveTextContent("Restoring…"));
    expect(toastEl()).not.toHaveAttribute("data-removed", "true");
    expect(toastEl()).not.toHaveTextContent("is back");
    expect(screen.queryByRole("button", { name: "Undo" })).toBeNull();
  });
});
