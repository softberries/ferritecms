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

function Harness({
  start,
  onUnauthenticated,
}: {
  start: SchemaContentDto;
  onUnauthenticated?: (e: unknown, draft: unknown) => void;
}) {
  const d = useEditorDocument({
    type: "post",
    item: start,
    placeholder: "",
    bodyLabel: "Body",
    ...(onUnauthenticated ? { onUnauthenticated } : {}),
  });
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

  test("a_slot_delete_401_hands_the_draft_to_sign_in", async () => {
    // e1cd3a5 re-review: the DELETE of a slot the draft no longer needs swallowed a lost
    // session; it goes to `onUnauthenticated` like every other slot write.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const methods: string[] = [];
    stubRoutes({
      [SLOT]: async (_url, req) => {
        methods.push(req.method);
        if (req.method === "PUT") {
          return json({ ...((await req.json()) as object), saved_at: "2026-10-01T10:06:00Z" });
        }
        return new Response(
          JSON.stringify({
            type: "https://ferritecms.dev/problems/unauthenticated",
            title: "Unauthenticated",
            status: 401,
            detail: "sign in again",
          }),
          { status: 401, headers: { "Content-Type": "application/problem+json" } },
        );
      },
    });
    const onUnauthenticated = vi.fn();
    renderWithRouter(
      <Harness start={item(4, "Stored text")} onUnauthenticated={onUnauthenticated} />,
    );
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => doc?.setTitle("Half-way title"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(methods).toEqual(["PUT"]));
    act(() => doc?.setTitle("Restore me"));
    await waitFor(() => expect(onUnauthenticated).toHaveBeenCalledOnce());
    expect(methods).toEqual(["PUT", "DELETE"]);
    expect(onUnauthenticated.mock.calls[0]?.[1]).toMatchObject({ title: "Restore me" });
  });

  test("a_body_edit_undone_to_the_item_is_saved_again", async () => {
    // e1cd3a5 re-review: a body edit set "Unsaved changes" for good, even once undone.
    stubRoutes({ [SLOT]: () => new Response(null, { status: 204 }) });
    renderWithRouter(<Harness start={item(4, "Stored text")} />);
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => {
      doc?.editor.commands.insertContent(" more");
    });
    await waitFor(() => expect(doc?.dirty).toBe(true));
    act(() => {
      doc?.editor.commands.undo();
    });
    await waitFor(() => expect(doc?.dirty).toBe(false));
    expect(doc?.saveState).toBe("saved");
    expect(doc?.indicator).toBe("saved");
  });

  test("an_edit_back_during_a_running_put_deletes_the_slot_after_it", async () => {
    // The PUT of "Half-way" runs; the draft goes back to the item meanwhile: the PUT
    // lands first (it may hold a draft), then the slot is deleted, nothing after it.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const log: string[] = [];
    let landPut: () => void = () => undefined;
    stubRoutes({
      [SLOT]: async (_url, req) => {
        if (req.method === "PUT") {
          const body = (await req.json()) as { title: string };
          log.push(`PUT ${body.title}`);
          await new Promise<void>((r) => {
            landPut = r;
          });
          log.push("PUT landed");
          return json({ ...body, saved_at: "2026-10-01T10:06:00Z" });
        }
        log.push(req.method);
        return new Response(null, { status: 204 });
      },
    });
    renderWithRouter(<Harness start={item(4, "Stored text")} />);
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => doc?.setTitle("Half-way"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(log).toEqual(["PUT Half-way"]));
    act(() => doc?.setTitle("Restore me"));
    await act(() => vi.advanceTimersByTimeAsync(100));
    expect(log).toEqual(["PUT Half-way"]);
    act(() => landPut());
    await waitFor(() => expect(log).toEqual(["PUT Half-way", "PUT landed", "DELETE"]));
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(log).toEqual(["PUT Half-way", "PUT landed", "DELETE"]);
    expect(doc?.indicator).toBe("saved");
  });

  test("an_edit_during_a_running_delete_is_written_after_it", async () => {
    // The draft went back to the item and the slot's DELETE runs; a new edit meanwhile is
    // not lost: it is written once the DELETE is done.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const log: string[] = [];
    let landDelete: () => void = () => undefined;
    stubRoutes({
      [SLOT]: async (_url, req) => {
        if (req.method === "PUT") {
          const body = (await req.json()) as { title: string };
          log.push(`PUT ${body.title}`);
          return json({ ...body, saved_at: "2026-10-01T10:06:00Z" });
        }
        log.push("DELETE");
        await new Promise<void>((r) => {
          landDelete = r;
        });
        log.push("DELETE landed");
        return new Response(null, { status: 204 });
      },
    });
    renderWithRouter(<Harness start={item(4, "Stored text")} />);
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => doc?.setTitle("Half-way"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(log).toEqual(["PUT Half-way"]));
    act(() => doc?.setTitle("Restore me"));
    await waitFor(() => expect(log).toEqual(["PUT Half-way", "DELETE"]));
    act(() => doc?.setTitle("Typed during the delete"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    // Nothing is written while the DELETE runs...
    expect(log).toEqual(["PUT Half-way", "DELETE"]);
    act(() => landDelete());
    // ...and the edit right after it.
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() =>
      expect(log).toEqual([
        "PUT Half-way",
        "DELETE",
        "DELETE landed",
        "PUT Typed during the delete",
      ]),
    );
  });

  test("a_401_on_the_waited_put_ends_the_slot_work_once", async () => {
    // Review minor: the draft went back to the item while a PUT ran; that PUT answered
    // 401 (the autosaver stopped, the draft went to sign in). No DELETE follows it, and
    // the sign-in is asked for once.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const methods: string[] = [];
    let second: (r: Response) => void = () => undefined;
    stubRoutes({
      [SLOT]: async (_url, req) => {
        methods.push(req.method);
        if (req.method === "PUT" && methods.length === 1) {
          return json({ ...((await req.json()) as object), saved_at: "2026-10-01T10:06:00Z" });
        }
        if (req.method === "PUT") return new Promise<Response>((r) => (second = r));
        return new Response(null, { status: 204 });
      },
    });
    const onUnauthenticated = vi.fn();
    renderWithRouter(
      <Harness start={item(4, "Stored text")} onUnauthenticated={onUnauthenticated} />,
    );
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => doc?.setTitle("First"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(methods).toEqual(["PUT"]));
    act(() => doc?.setTitle("Second"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(methods).toEqual(["PUT", "PUT"]));
    act(() => doc?.setTitle("Restore me"));
    act(() =>
      second(
        new Response(
          JSON.stringify({
            type: "https://ferritecms.dev/problems/unauthenticated",
            title: "Unauthenticated",
            status: 401,
            detail: "sign in again",
          }),
          { status: 401, headers: { "Content-Type": "application/problem+json" } },
        ),
      ),
    );
    await waitFor(() => expect(onUnauthenticated).toHaveBeenCalledOnce());
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(methods).toEqual(["PUT", "PUT"]);
    expect(onUnauthenticated).toHaveBeenCalledOnce();
  });

  test("typing_while_the_slot_waits_on_a_running_put_keeps_the_edit", async () => {
    // The draft went back to the item while a PUT ran (the slot waits for it), then the
    // user typed again before it landed: no DELETE, and the new edit is written after.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const log: string[] = [];
    let landPut: () => void = () => undefined;
    stubRoutes({
      [SLOT]: async (_url, req) => {
        if (req.method !== "PUT") {
          log.push(req.method);
          return new Response(null, { status: 204 });
        }
        const body = (await req.json()) as { title: string };
        log.push(`PUT ${body.title}`);
        if (body.title === "Half-way") {
          await new Promise<void>((r) => {
            landPut = r;
          });
        }
        return json({ ...body, saved_at: "2026-10-01T10:06:00Z" });
      },
    });
    renderWithRouter(<Harness start={item(4, "Stored text")} />);
    await waitFor(() => expect(doc).not.toBeNull());
    act(() => doc?.setTitle("Half-way"));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(log).toEqual(["PUT Half-way"]));
    act(() => doc?.setTitle("Restore me"));
    act(() => doc?.setTitle("Typed while waiting"));
    act(() => landPut());
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(log).toEqual(["PUT Half-way", "PUT Typed while waiting"]));
    expect(doc?.dirty).toBe(true);
  });
});
