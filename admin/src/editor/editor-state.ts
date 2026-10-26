// The editor's document state: the TipTap editor bound to the core block model, the
// title, the side panel's members (slug, excerpt, terms, parent, menu order), the
// server's copy of the item, dirty tracking, `save()` and the lifecycle transitions.
// Title, body and panel are one draft: `save()` PATCHes the members that changed, and a
// transition with unsaved changes saves them first and then transitions with the
// version the save returned (pre-flight F40). Every failed write goes through one place
// (`onConflict` for a 412, `onWriteError` otherwise) so Task 16's conflict dialog plugs
// in there. The writing surface (`editor.tsx`) and the side panel (`panel/`) render it;
// autosave and the leave guard (Task 16) and revisions (Task 17) build on it.
//
// Read-only (Review Focus 3): a body with a node, mark or heading level the editor
// cannot represent (`validateBody`), a body TipTap's content check refuses
// (`onContentError`), an item the server does not let this account edit
// (`allowed.edit`) and a trashed item all open with `editable: false`, and `save()` sends
// nothing. An unsupported body is shown through `displayBody`, a display-only copy;
// the stored body is never replaced by it. A transition that leaves the item not
// editable (`allowed.edit` false) turns the screen read-only from then on.

import type { JSONContent } from "@tiptap/core";
import { useEditor, type Editor } from "@tiptap/react";
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { NetworkError } from "@/api/client";
import { type LifecycleAction, createContent, lifecycle, patchContent } from "@/api/content";
import { qk } from "@/api/keys";
import { resolveMediaUrl } from "@/api/media";
import { type ApiError, isProblem } from "@/api/problem";
import type { SchemaContentDto, SchemaContentInput, SchemaContentPatch } from "@/api/schema";
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

/** The side panel's members of the draft (with title and body, one draft). */
export interface MetaDraft {
  slug: string;
  /** The excerpt as shown; `""` means "derive it from the body" (`excerpt: null`, F39). */
  excerpt: string;
  /** Term ids, in order. */
  terms: string[];
  parent_id: string | null;
  menu_order: number;
}

/** A transition the panel runs (`trash` goes through the list's undo logic instead). */
export type Transition = Exclude<LifecycleAction, "trash" | "restore">;

/** What a write was, for its error message. */
export type WriteOp = "save" | Transition;

/** The panel members as stored on `item` (a new item: empty). */
export function metaOf(item: ContentDto | null): MetaDraft {
  return item
    ? {
        slug: item.slug,
        excerpt: item.excerpt,
        terms: [...item.terms],
        parent_id: item.parent_id ?? null,
        menu_order: item.menu_order,
      }
    : { slug: "", excerpt: "", terms: [], parent_id: null, menu_order: 0 };
}

const sameTerms = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((id, i) => id === b[i]);

/**
 * The PATCH members of `draft` that differ from `base`. An empty slug is never sent
 * (the field puts the stored one back); an empty excerpt is sent as `null` (derive).
 */
export function metaChanges(draft: MetaDraft, base: MetaDraft): SchemaContentPatch {
  const out: SchemaContentPatch = {};
  const slug = draft.slug.trim();
  if (slug !== "" && slug !== base.slug) out.slug = slug;
  if (draft.excerpt !== base.excerpt) out.excerpt = draft.excerpt === "" ? null : draft.excerpt;
  if (!sameTerms(draft.terms, base.terms)) out.terms = [...draft.terms];
  if (draft.parent_id !== base.parent_id) out.parent_id = draft.parent_id;
  if (draft.menu_order !== base.menu_order) out.menu_order = draft.menu_order;
  return out;
}

/** The draft after a save: members unchanged since the request take the server's value
 * (the normalised slug, the derived excerpt); members edited meanwhile stay. */
function settleMeta(now: MetaDraft, sent: MetaDraft, stored: MetaDraft): MetaDraft {
  return {
    slug: now.slug === sent.slug ? stored.slug : now.slug,
    excerpt: now.excerpt === sent.excerpt ? stored.excerpt : now.excerpt,
    terms: sameTerms(now.terms, sent.terms) ? stored.terms : now.terms,
    parent_id: now.parent_id === sent.parent_id ? stored.parent_id : now.parent_id,
    menu_order: now.menu_order === sent.menu_order ? stored.menu_order : now.menu_order,
  };
}

/** What a just-created item hands to its `/$id` screen (the route remounts the editor). */
export interface Handoff {
  id: string;
  title: string;
  body: JSONContent;
  selection: { from: number; to: number } | null;
  focus: "title" | "body" | null;
  /** What the first save sent (the dirty baseline of the `/$id` screen). */
  saved: { title: string; body: string };
  /** The panel members as they were (edits made while the first save was in flight). */
  meta: MetaDraft;
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
   * A `412` on a save or a transition: someone saved in between. Task 16's conflict
   * dialog replaces the default (the caller's toast); the user's text stays either way.
   */
  onConflict?: (error: ApiError, op: WriteOp) => void;
  /** Any other failed write (shown by the caller, unless the transition handles it). */
  onWriteError?: (error: unknown, op: WriteOp) => void;
}

export interface TransitionOptions {
  /** `schedule`: when, RFC 3339. */
  at?: string;
  /** Handles a failure of the transition itself (not a 412, not the save before it)
   * instead of `onWriteError`; the publish dialog shows it in place. */
  onError?: (error: unknown) => void;
}

export interface EditorDocument {
  editor: Editor;
  /** The server's latest copy (`null` until a new item's first save). */
  item: ContentDto | null;
  title: string;
  setTitle: (title: string) => void;
  readOnly: ReadOnlyReason | null;
  /** The panel members as edited. */
  meta: MetaDraft;
  /** The panel members as the server holds them. */
  savedMeta: MetaDraft;
  setMeta: (patch: Partial<MetaDraft>) => void;
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
  /**
   * Saves what changed (F40), then runs `action` with the version the save returned.
   * Resolves to the item after it, or `null` when the save or the transition failed.
   */
  transition: (action: Transition, options?: TransitionOptions) => Promise<ContentDto | null>;
  /** Takes the server's copy after a write made elsewhere (the trash in the panel). */
  adopt: (item: ContentDto) => void;
}

function bodyReadOnly(item: ContentDto | null, body: unknown): ReadOnlyReason | null {
  if (item === null) return null;
  const check = validateBody(body);
  return check.ok ? null : { kind: "unsupported", unknown: check.unknown };
}

/** What the item's status and `allowed` say now (they change with transitions). */
function itemReadOnly(item: ContentDto | null): ReadOnlyReason | null {
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
  // The body is judged once per screen (what it was when it opened); status and
  // `allowed` follow the item.
  const [initialBodyReadOnly] = useState(() => bodyReadOnly(opts.item, storedBody));
  const [initialReadOnly] = useState(() => initialBodyReadOnly ?? itemReadOnly(opts.item));
  const [contentError, setContentError] = useState(false);
  const readOnly: ReadOnlyReason | null = useMemo(
    () => initialBodyReadOnly ?? (contentError ? { kind: "invalid" } : null) ?? itemReadOnly(item),
    [initialBodyReadOnly, contentError, item],
  );
  const readOnlyRef = useRef(readOnly);
  useEffect(() => {
    readOnlyRef.current = readOnly;
  });

  const [initialContent] = useState<JSONContent | undefined>(() => {
    if (handoff) return handoff.body;
    if (initialBodyReadOnly?.kind === "unsupported") {
      return displayBody(storedBody) as JSONContent;
    }
    const body = storedBody as JSONContent | null | undefined;
    // An empty document (`content: []`, what the API stores for an item created without
    // a body) is not valid for the schema (`block+`); TipTap's own empty document stands
    // in for it, and the baseline below is taken from that, so it is not "dirty".
    return body &&
      typeof body === "object" &&
      body.type === "doc" &&
      Array.isArray(body.content) &&
      body.content.length > 0
      ? body
      : undefined;
  });

  // Built once: the editor is created once per screen.
  const [extensions] = useState(() =>
    editorExtensions({ placeholder: opts.placeholder, resolveMedia: resolveMediaUrl }),
  );

  const [title, setTitleState] = useState(handoff?.title ?? opts.item?.title ?? "");
  const titleRef = useRef(title);
  const [meta, setMetaState] = useState<MetaDraft>(() => handoff?.meta ?? metaOf(opts.item));
  const metaRef = useRef(meta);
  const [savedMeta, setSavedMeta] = useState<MetaDraft>(() => metaOf(opts.item));
  const savedMetaRef = useRef(savedMeta);
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

  // A hand-over is dirty when the user typed while the first save was in flight.
  const [dirty, setDirty] = useState(() =>
    handoff
      ? handoff.title !== handoff.saved.title ||
        JSON.stringify(editor.getJSON()) !== handoff.saved.body ||
        Object.keys(metaChanges(handoff.meta, metaOf(opts.item))).length > 0
      : false,
  );
  const [saveState, setSaveState] = useState<SaveStateValue>(() => (dirty ? "dirty" : "saved"));

  const isDirtyNow = useCallback(() => {
    const base = saved.current;
    return (
      titleRef.current !== base.title ||
      JSON.stringify(editor.getJSON()) !== base.body ||
      Object.keys(metaChanges(metaRef.current, savedMetaRef.current)).length > 0
    );
  }, [editor]);

  const recompute = useCallback(() => {
    const isDirty = isDirtyNow();
    setDirty(isDirty);
    setSaveState((s) => (s === "saving" ? s : isDirty ? "dirty" : "saved"));
  }, [isDirtyNow]);

  // A transition can take the right to edit away (or the item into the trash).
  useEffect(() => {
    if (readOnly === null || editor.isDestroyed || !editor.isEditable) return;
    editor.setEditable(false, false);
    editor.view.dom.setAttribute("aria-readonly", "true");
  }, [editor, readOnly]);

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

  // A hand-over continues where the user was: same selection and focus.
  useEffect(() => {
    if (!handoff) return;
    if (handoff.selection) {
      const size = editor.state.doc.content.size;
      const from = Math.min(handoff.selection.from, size);
      const to = Math.min(handoff.selection.to, size);
      editor.commands.setTextSelection({ from, to });
    }
    if (handoff.focus === "body") editor.commands.focus();
  }, [editor, handoff]);

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

  const setMeta = useCallback(
    (patch: Partial<MetaDraft>) => {
      metaRef.current = { ...metaRef.current, ...patch };
      setMetaState(metaRef.current);
      recompute();
    },
    [recompute],
  );

  /** The server's copy of the item, everywhere (screen, cache, lists, counts). */
  const adopt = useCallback(
    (next: ContentDto) => {
      itemRef.current = next;
      setItem(next);
      queryClient.setQueryData(qk.content.item(type, next.id), next);
      void queryClient.invalidateQueries({ queryKey: qk.content.lists(type) });
      void queryClient.invalidateQueries({ queryKey: qk.content.counts(type) });
    },
    [queryClient, type],
  );

  /** The one place a failed write goes: a 412 to the conflict handler (Task 16). */
  const fail = useCallback((e: unknown, op: WriteOp, onError?: (e: unknown) => void) => {
    if (isProblem(e, "precondition-failed")) latest.current.onConflict?.(e, op);
    else if (onError) onError(e);
    else latest.current.onWriteError?.(e, op);
  }, []);

  const inflight = useRef<Promise<ContentDto | null> | null>(null);

  const save = useCallback((): Promise<ContentDto | null> => {
    if (readOnlyRef.current) return Promise.resolve(null);
    if (inflight.current) return inflight.current;
    const current = itemRef.current;
    const sentTitle = titleRef.current;
    const doc = editor.getJSON();
    const sentBody = JSON.stringify(doc);
    const sentMeta = metaRef.current;
    const base = saved.current;
    const metaPatch = metaChanges(sentMeta, savedMetaRef.current);
    const titleChanged = base.title !== sentTitle;
    const bodyChanged = base.body !== sentBody;
    if (current && !titleChanged && !bodyChanged && Object.keys(metaPatch).length === 0) {
      setDirty(false);
      setSaveState("saved");
      return Promise.resolve(current);
    }
    setSaveState("saving");
    const run = async (): Promise<ContentDto | null> => {
      try {
        let next: ContentDto;
        if (current) {
          // The changed members only.
          const patch: SchemaContentPatch = { ...metaPatch };
          if (titleChanged) patch.title = sentTitle;
          if (bodyChanged) patch.body = asBody(doc);
          next = await patchContent(type, current.id, current.version, patch);
        } else {
          const input: SchemaContentInput = { title: sentTitle, body: asBody(doc) };
          if (metaPatch.slug !== undefined) input.slug = metaPatch.slug;
          if (metaPatch.excerpt) input.excerpt = metaPatch.excerpt;
          if (metaPatch.terms && metaPatch.terms.length > 0) input.terms = metaPatch.terms;
          if (metaPatch.parent_id) input.parent_id = metaPatch.parent_id;
          if (metaPatch.menu_order !== undefined) input.menu_order = metaPatch.menu_order;
          next = await createContent(type, input);
        }
        saved.current = { title: sentTitle, body: sentBody };
        const stored = metaOf(next);
        savedMetaRef.current = stored;
        setSavedMeta(stored);
        metaRef.current = settleMeta(metaRef.current, sentMeta, stored);
        setMetaState(metaRef.current);
        adopt(next);
        setSavedAt(new Date());
        const still = !isDirtyNow();
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
            meta: metaRef.current,
          });
        }
        return next;
      } catch (e) {
        setSaveState(e instanceof NetworkError ? "offline" : "error");
        fail(e, "save");
        return null;
      } finally {
        inflight.current = null;
      }
    };
    const p = run();
    inflight.current = p;
    return p;
  }, [editor, type, adopt, fail, isDirtyNow]);

  const transition = useCallback(
    async (action: Transition, options: TransitionOptions = {}): Promise<ContentDto | null> => {
      if (readOnlyRef.current) return null;
      // Unsaved changes first (F40); a save that joined one already in flight may not
      // hold the latest keystrokes, so save again while anything is still unsaved.
      let current = await save();
      if (current && isDirtyNow()) current = await save();
      if (!current || itemRef.current === null) return null;
      try {
        const next =
          action === "schedule"
            ? await lifecycle(type, current.id, "schedule", current.version, {
                at: options.at ?? "",
              })
            : await lifecycle(type, current.id, action, current.version);
        adopt(next);
        return next;
      } catch (e) {
        fail(e, action, options.onError);
        return null;
      }
    },
    [save, isDirtyNow, type, adopt, fail],
  );

  const initialFocus = handoff ? handoff.focus : opts.item === null ? "title" : null;
  return {
    editor,
    item,
    title,
    setTitle,
    readOnly,
    meta,
    savedMeta,
    setMeta,
    storedBody,
    initialFocus,
    dirty,
    saveState,
    savedAt,
    save,
    transition,
    adopt,
  };
}
