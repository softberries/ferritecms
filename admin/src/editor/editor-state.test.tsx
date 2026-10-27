// The editor's document state around a revision restore: editing stops the moment the
// restore is asked for, also while a save in flight lands first, so nothing typed in
// that window is replaced by the restore without a word.

import { EditorContent } from "@tiptap/react";
import { act, waitFor } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { SchemaContentDto } from "@/api/schema";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { type EditorDocument, useEditorDocument } from "./editor-state";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";

function item(version: number, text: string): SchemaContentDto {
  return {
    id: ID,
    type: "post",
    title: "Restore me",
    slug: "restore-me",
    status: "draft",
    body: { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text }] }] },
    excerpt: "",
    fields: {},
    seo: { title: null, description: null },
    terms: [],
    menu_order: 0,
    permalink: "/restore-me/",
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

let doc: EditorDocument | null = null;

function Harness({ start }: { start: SchemaContentDto }) {
  const d = useEditorDocument({ type: "post", item: start, placeholder: "", bodyLabel: "Body" });
  useEffect(() => {
    doc = d;
  });
  return <EditorContent editor={d.editor} />;
}

afterEach(() => {
  doc = null;
  vi.unstubAllGlobals();
});

describe("restore_revision", () => {
  test("locks_editing_before_waiting_for_a_save_in_flight", async () => {
    let landSave: ((r: Response) => void) | null = null;
    const ifMatch: (string | null)[] = [];
    stubRoutes({
      [`/api/v1/content/post/${ID}`]: (_url, req) => {
        expect(req.method).toBe("PATCH");
        return new Promise<Response>((resolve) => {
          landSave = resolve;
        });
      },
      [`/api/v1/content/post/${ID}/revisions/r1/restore`]: (_url, req) => {
        ifMatch.push(req.headers.get("If-Match"));
        return json(item(5, "From revision one"));
      },
      [`/api/v1/content/post/${ID}/autosave`]: () => new Response(null, { status: 204 }),
      [`/api/v1/content/post/${ID}/revisions`]: () => json({ items: [] }),
    });
    renderWithRouter(<Harness start={item(3, "Stored text")} />);
    await waitFor(() => {
      expect(doc?.editor.isEditable).toBe(true);
    });
    const d = () => doc as EditorDocument;

    act(() => {
      d().editor.commands.insertContent(" edited");
    });
    let saving: Promise<unknown> = Promise.resolve();
    let restoring: Promise<unknown> = Promise.resolve();
    act(() => {
      saving = d().save();
    });
    act(() => {
      restoring = d().restoreRevision("r1");
    });
    // The PATCH has not landed, yet editing is already off (body and title).
    expect(d().editor.isEditable).toBe(false);
    expect(d().restoring).toBe(true);
    expect(ifMatch).toEqual([]);

    // Hold the PATCH until the restore has been asked for, then let it land.
    await waitFor(() => {
      expect(landSave).not.toBeNull();
    });
    expect(d().editor.isEditable).toBe(false);
    await act(async () => {
      landSave?.(json(item(4, "Stored text edited")));
      await saving;
      await restoring;
    });
    // The restore matched the version the save returned.
    expect(ifMatch).toEqual(['"4"']);
  });

  test("unlocks_when_there_is_nothing_to_restore_over", async () => {
    stubRoutes({
      [`/api/v1/content/post/${ID}/autosave`]: () => new Response(null, { status: 204 }),
    });
    const start = item(3, "Stored text");
    renderWithRouter(<Harness start={{ ...start, allowed: { ...start.allowed, edit: false } }} />);
    await waitFor(() => {
      expect(doc).not.toBeNull();
    });
    const onError = vi.fn();
    await act(async () => {
      await (doc as EditorDocument).restoreRevision("r1", { onError });
    });
    expect(onError).toHaveBeenCalledWith(null);
    expect((doc as EditorDocument).restoring).toBe(false);
  });
});

describe("autosave_slot", () => {
  const SLOT = `/api/v1/content/post/${ID}/autosave`;

  function slotLog() {
    const log: { method: string; title?: string }[] = [];
    stubRoutes({
      [SLOT]: async (_url, req) => {
        if (req.method === "PUT") {
          const body = (await req.json()) as { title: string };
          log.push({ method: "PUT", title: body.title });
          return json({ ...body, saved_at: "2026-10-01T10:06:00Z" });
        }
        log.push({ method: req.method });
        return new Response(null, { status: 204 });
      },
    });
    return log;
  }

  afterEach(() => {
    vi.useRealTimers();
  });

  test("a_draft_edited_back_to_the_item_clears_the_slot", async () => {
    // CI run 40404431115: invalid values sat long enough to be autosaved, were typed back
    // to what the item holds ("Saved"), and a reload within the next 10 s opened the old
    // invalid draft from the slot as a restored draft. Back at the item, the slot goes.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const log = slotLog();
    renderWithRouter(<Harness start={item(4, "Stored text")} />);
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => doc?.setTitle("Half-way title"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(log).toEqual([{ method: "PUT", title: "Half-way title" }]));

    act(() => doc?.setTitle("Restore me"));
    await act(() => vi.advanceTimersByTimeAsync(100));
    await waitFor(() => expect(log.map((r) => r.method)).toEqual(["PUT", "DELETE"]));
    expect(doc?.saveState).toBe("saved");
    // Nothing is written after it: the item holds the draft.
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(log.map((r) => r.method)).toEqual(["PUT", "DELETE"]);

    // Edited again: the slot follows as before.
    act(() => doc?.setTitle("Second edit"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(log.at(-1)).toEqual({ method: "PUT", title: "Second edit" }));
  });

  test("an_edit_undone_before_the_slot_took_it_writes_nothing", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const log = slotLog();
    renderWithRouter(<Harness start={item(4, "Stored text")} />);
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => doc?.setTitle("Typo"));
    act(() => doc?.setTitle("Restore me"));
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(log).toEqual([]);
  });
});
