// The editor's document state: the TipTap editor bound to the core block model, the
// title, the server's copy of the item, dirty tracking and `save()`. The writing surface
// (`editor.tsx`) renders it; the side panel and publishing (Task 15), autosave, the 412
// conflict dialog and the leave guard (Task 16) and revisions (Task 17) build on it.
//
// Read-only (Review Focus 3): a body with a node, mark or heading level the editor
// cannot represent (`validateBody`), a body TipTap's content check refuses
// (`onContentError`), an item the server does not let this account edit
// (`allowed.edit`) and a trashed item all open with `editable: false`, and `save()` sends
// nothing. An unsupported body is shown through `displayBody`, a display-only copy;
// the stored body is never replaced by it.

import type { JSONContent } from "@tiptap/core";
import { useEditor, type Editor } from "@tiptap/react";
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { NetworkError } from "@/api/client";
import { createContent, patchContent } from "@/api/content";
import { qk } from "@/api/keys";
import { resolveMediaUrl } from "@/api/media";
import { type ApiError, isProblem } from "@/api/problem";
import type { SchemaContentDto, SchemaContentPatch } from "@/api/schema";
import type { SaveStateValue } from "@/components/kit/save-state";
import { pushRecent } from "@/palette/recent";
import { displayBody, editorExtensions, validateBody } from "./schema";

export type ContentDto = SchemaContentDto;

export type ReadOnlyReason =
  /** The body holds what the editor cannot represent; `unknown` names it (may be empty
   * when TipTap's content check refused the structure). */
  | { kind: "unsupported"; unknown: string[] }
  /** TipTap's content check refused the body's structure: it is shown as plain text
   * (`storedBody`), never through the editor's schema, so nothing seems lost. */
  | { kind: "invalid" }
  /** The server says this account may not edit the item (`allowed.edit`). */
  | { kind: "forbidden" }
  | { kind: "trash" };

/** What a just-created item hands to its `/$id` screen (the route remounts the editor). */
export interface Handoff {
  id: string;
  title: string;
  body: JSONContent;
  selection: { from: number; to: number } | null;
  focus: "title" | "body" | null;
  /** What the first save sent (the dirty baseline of the `/$id` screen). */
  saved: { title: string; body: string };
}

let pendingHandoff: Handoff | null = null;

/** Stashes the live editor state of a just-created item for its `/$id` screen. */
export function handOff(h: Handoff): void {
  pendingHandoff = h;
}

/** The waiting hand-over for `id`, else `null` (the `/$id` loader then skips its fetch). */
export function peekHandoff(id: string): Handoff | null {
  return pendingHandoff?.id === id ? pendingHandoff : null;
}

/** Drops the hand-over for `id` once its screen has mounted. */
export function clearHandoff(id: string): void {
  if (pendingHandoff?.id === id) pendingHandoff = null;
}

export interface EditorDocumentOptions {
  type: string;
  /** The item as loaded; `null` for a new one (created on the first save). */
  item: ContentDto | null;
  /** From `peekHandoff` when the screen continues a just-created item. */
  handoff?: Handoff | null;
  placeholder: string;
  /** The body's accessible name. */
  bodyLabel: string;
  /** After the first save of a new item (the route navigates to its `/$id`). */
  onCreated?: (item: ContentDto, handoff: Handoff) => void;
  /**
   * A `412`: someone saved in between. Task 16's conflict dialog replaces the default
   * (the caller's toast); the user's text stays in the editor either way.
   */
  onConflict?: (error: ApiError) => void;
  /** Any other failed save (shown by the caller). */
  onSaveError?: (error: unknown) => void;
}

export interface EditorDocument {
  editor: Editor;
  /** The server's latest copy (`null` until a new item's first save). */
  item: ContentDto | null;
  title: string;
  setTitle: (title: string) => void;
  readOnly: ReadOnlyReason | null;
  /** The body as the server stored it (for the `invalid` plain-text view). */
  storedBody: unknown;
  /** Where the cursor goes when the screen opens (a hand-over keeps the user's place). */
  initialFocus: "title" | "body" | null;
  dirty: boolean;
  saveState: SaveStateValue;
  /** When the last save landed (or the item was last updated). */
  savedAt: Date | null;
  /**
   * Saves title and body: `POST` for a new item, else `PATCH` with `If-Match`. Resolves
   * to the saved item (its `version` is the new ETag), the current one when nothing
   * changed, or `null` when nothing was saved (read-only, conflict, failure). At most
   * one request is in flight; a second call joins it.
   */
  save: () => Promise<ContentDto | null>;
}

function readOnlyFor(item: ContentDto | null, body: unknown): ReadOnlyReason | null {
  const check = validateBody(body);
  if (!check.ok) return { kind: "unsupported", unknown: check.unknown };
  if (item?.status === "trash") return { kind: "trash" };
  if (item && !item.allowed.edit) return { kind: "forbidden" };
  return null;
}

const asBody = (doc: JSONContent) => doc as unknown as NonNullable<SchemaContentPatch["body"]>;

export function useEditorDocument(opts: EditorDocumentOptions): EditorDocument {
  const { type, handoff } = opts;
  const queryClient = useQueryClient();
  const [item, setItem] = useState<ContentDto | null>(opts.item);
  const itemRef = useRef(item);
  const latest = useRef(opts);
  useEffect(() => {
    latest.current = opts;
  });

  const storedBody: unknown = opts.item?.body;
  // Decided once per screen: what the item was when it opened.
  const [initialReadOnly] = useState(() => readOnlyFor(opts.item, storedBody));
  const [contentError, setContentError] = useState(false);
  const readOnly: ReadOnlyReason | null =
    initialReadOnly ?? (contentError ? { kind: "invalid" } : null);
  const readOnlyRef = useRef(readOnly);
  useEffect(() => {
    readOnlyRef.current = readOnly;
  });

  const [initialContent] = useState<JSONContent | undefined>(() => {
    if (handoff) return handoff.body;
    if (initialReadOnly?.kind === "unsupported") return displayBody(storedBody) as JSONContent;
    const body = storedBody as JSONContent | null | undefined;
    return body && typeof body === "object" && body.type === "doc" ? body : undefined;
  });

  // Built once: the editor is created once per screen.
  const [extensions] = useState(() =>
    editorExtensions({ placeholder: opts.placeholder, resolveMedia: resolveMediaUrl }),
  );

  const [title, setTitleState] = useState(handoff?.title ?? opts.item?.title ?? "");
  const titleRef = useRef(title);
  const [dirty, setDirty] = useState(false);
  const [saveState, setSaveState] = useState<SaveStateValue>("saved");
  const [savedAt, setSavedAt] = useState<Date | null>(
    opts.item ? new Date(opts.item.updated_at) : null,
  );

  const editor = useEditor({
    extensions,
    ...(initialContent ? { content: initialContent } : {}),
    editable: initialReadOnly === null,
    immediatelyRender: true,
    shouldRerenderOnTransaction: false,
    injectCSS: false,
    enableContentCheck: true,
    onContentError: ({ editor: e }) => {
      // TipTap refused the structure: never let it be saved over (Review Focus 3).
      readOnlyRef.current = readOnlyRef.current ?? { kind: "invalid" };
      e.setEditable(false, false);
      queueMicrotask(() => setContentError(true));
    },
    editorProps: {
      attributes: {
        class: "editor-body",
        spellcheck: "true",
        role: "textbox",
        "aria-multiline": "true",
        "aria-label": opts.bodyLabel,
        ...(initialReadOnly === null ? {} : { "aria-readonly": "true" }),
      },
    },
  });

  // The baseline (what the server holds, as JSON text): the body as TipTap holds it
  // after load, its attribute defaults included, so opening and saving untouched is not
  // "dirty"; after a hand-over, what the first save sent.
  const [baseline] = useState(
    () =>
      handoff?.saved ?? { title: opts.item?.title ?? "", body: JSON.stringify(editor.getJSON()) },
  );
  const saved = useRef(baseline);

  const recompute = useCallback(() => {
    const base = saved.current;
    const isDirty =
      titleRef.current !== base.title || JSON.stringify(editor.getJSON()) !== base.body;
    setDirty(isDirty);
    setSaveState((s) => (s === "saving" ? s : isDirty ? "dirty" : "saved"));
  }, [editor]);

  useEffect(() => {
    const onUpdate = () => {
      setDirty(true);
      setSaveState((s) => (s === "saving" ? s : "dirty"));
    };
    editor.on("update", onUpdate);
    return () => {
      editor.off("update", onUpdate);
    };
  }, [editor]);

  // A hand-over continues where the user was: same selection and focus, and dirty when
  // they typed while the first save was in flight.
  useEffect(() => {
    if (!handoff) return;
    if (handoff.selection) {
      const size = editor.state.doc.content.size;
      const from = Math.min(handoff.selection.from, size);
      const to = Math.min(handoff.selection.to, size);
      editor.commands.setTextSelection({ from, to });
    }
    if (handoff.focus === "body") editor.commands.focus();
    recompute();
  }, [editor, handoff, recompute]);

  // The palette's Recent group: this item, with its title as it is now.
  const id = item?.id;
  const status = item?.status;
  const recentTitle = item?.title;
  useEffect(() => {
    if (id && status !== undefined && recentTitle !== undefined) {
      pushRecent({ kind: "item", type, id, title: recentTitle, status });
    }
  }, [type, id, status, recentTitle]);

  const setTitle = useCallback(
    (next: string) => {
      titleRef.current = next;
      setTitleState(next);
      if (next !== saved.current.title) {
        setDirty(true);
        setSaveState((s) => (s === "saving" ? s : "dirty"));
      } else recompute();
    },
    [recompute],
  );

  const inflight = useRef<Promise<ContentDto | null> | null>(null);

  const save = useCallback((): Promise<ContentDto | null> => {
    if (readOnlyRef.current) return Promise.resolve(null);
    if (inflight.current) return inflight.current;
    const current = itemRef.current;
    const sentTitle = titleRef.current;
    const doc = editor.getJSON();
    const sentBody = JSON.stringify(doc);
    const base = saved.current;
    if (current && base.title === sentTitle && base.body === sentBody) {
      setDirty(false);
      setSaveState("saved");
      return Promise.resolve(current);
    }
    setSaveState("saving");
    const run = async (): Promise<ContentDto | null> => {
      try {
        const next = current
          ? await patchContent(type, current.id, current.version, {
              title: sentTitle,
              body: asBody(doc),
            })
          : await createContent(type, { title: sentTitle, body: asBody(doc) });
        itemRef.current = next;
        saved.current = { title: sentTitle, body: sentBody };
        setItem(next);
        queryClient.setQueryData(qk.content.item(type, next.id), next);
        void queryClient.invalidateQueries({ queryKey: qk.content.lists(type) });
        void queryClient.invalidateQueries({ queryKey: qk.content.counts(type) });
        setSavedAt(new Date());
        const still =
          titleRef.current === sentTitle && JSON.stringify(editor.getJSON()) === sentBody;
        setDirty(!still);
        setSaveState(still ? "saved" : "dirty");
        if (!current) {
          const { from, to } = editor.state.selection;
          const active = document.activeElement;
          latest.current.onCreated?.(next, {
            id: next.id,
            title: titleRef.current,
            body: editor.getJSON(),
            selection: { from, to },
            focus: editor.isFocused
              ? "body"
              : active instanceof HTMLTextAreaElement && active.dataset.slot === "editor-title"
                ? "title"
                : null,
            saved: { title: sentTitle, body: sentBody },
          });
        }
        return next;
      } catch (e) {
        setSaveState(e instanceof NetworkError ? "offline" : "error");
        if (isProblem(e, "precondition-failed")) latest.current.onConflict?.(e);
        else latest.current.onSaveError?.(e);
        return null;
      } finally {
        inflight.current = null;
      }
    };
    const p = run();
    inflight.current = p;
    return p;
  }, [editor, queryClient, type]);

  const initialFocus = handoff ? handoff.focus : opts.item === null ? "title" : null;
  return {
    editor,
    item,
    title,
    setTitle,
    readOnly,
    storedBody,
    initialFocus,
    dirty,
    saveState,
    savedAt,
    save,
  };
}
