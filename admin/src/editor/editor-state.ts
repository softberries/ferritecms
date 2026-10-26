// The editor's document state: the TipTap editor bound to the core block model, the
// title, the side panel's members (slug, excerpt, terms, parent, menu order), the
// server's copy of the item, dirty tracking, `save()` and the lifecycle transitions.
// Title, body and panel are one draft: `save()` PATCHes the members that changed, and a
// transition with unsaved changes saves them first and then transitions with the
// version the save returned (pre-flight F40). Every failed write goes through one place
// (`onConflict` for a 412, `onWriteError` otherwise) so Task 16's conflict dialog plugs
// in there. The writing surface (`editor.tsx`) and the side panel (`panel/`) render it;
// revisions (Task 17) build on it.
//
// Revisions (Task 17): `restoreRevision` writes an earlier revision back (`POST
// …/revisions/{rid}/restore` with `If-Match`), through the same failure path as a save
// (a `412` reaches the conflict dialog). The server deletes the user's autosave slot with
// it, so on success the autosaver is marked clean and stopped: the screen then reopens on
// the restored item, and nothing of the old working draft is written to the slot again.
//
// Autosave (Task 16): every change of an existing, editable item marks the autosaver
// (`autosave.ts`) dirty; it writes the working draft to the item's slot every 10 s. An
// explicit save holds it around the `PATCH` (no `PUT` can land after the `PATCH` that
// cleared the slot) and marks it clean after. The save indicator folds both together:
// "Saved" (the item holds it), "Autosaved" (the slot holds it), "Unsaved changes"
// (neither, yet). A restored draft (the slot, or one kept across a sign-in) opens as
// the working draft; one made from an older version saves with that version, so the
// server answers `412` and the conflict dialog decides. A `401` hands the draft to
// `onUnauthenticated` (it is kept for after the sign-in) instead of an error message.
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
import {
  type LifecycleAction,
  createContent,
  deleteAutosave,
  lifecycle,
  patchContent,
  putAutosave,
  restoreRevision,
} from "@/api/content";
import { qk } from "@/api/keys";
import { resolveMediaUrl } from "@/api/media";
import { type ApiError, isProblem } from "@/api/problem";
import type {
  SchemaAutosaveInput,
  SchemaContentDto,
  SchemaContentInput,
  SchemaContentPatch,
} from "@/api/schema";
import type { SaveStateValue } from "@/components/kit/save-state";
import { pushRecent } from "@/palette/recent";
import {
  AUTOSAVE_INTERVAL_MS,
  type AutosaveState,
  type Autosaver,
  type Draft,
  createAutosaver,
} from "./autosave";
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

/** What a `412` interrupted beyond the write itself: a transition, or a revision restore. */
export type Paused = Transition | "revision";

/** What a write was, for its error message. */
export type WriteOp = "save" | Paused;

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
  onConflict?: (error: ApiError, op: WriteOp, context: ConflictContext) => void;
  /** Any other failed write (shown by the caller, unless the transition handles it). */
  onWriteError?: (error: unknown, op: WriteOp) => void;
  /**
   * The session ended (`401`) or must change its password, during a save or an
   * autosave: `draft` is the working draft to keep for after the sign-in (F37).
   */
  onUnauthenticated?: (error: ApiError, draft: Draft) => void;
  /** A draft to open with instead of the stored item (the restore banner, Task 16). */
  restore?: RestoreSource | null;
  /**
   * The user's own version, kept in the slot after Take theirs. While it is there the
   * working draft is not autosaved (it would replace it); the tab asks before closing
   * instead, and an explicit save puts the kept copy back after the `PATCH` cleared it.
   */
  kept?: Draft | null;
}

/** A working draft the screen opens with: the autosave slot, or one kept over a sign-in. */
export interface RestoreSource {
  draft: Draft;
  /** When it was made (RFC 3339): the banner's "from …". */
  at: string;
  /**
   * The slot does not hold it yet (a draft kept over a sign-in): it is written there at
   * once, and `onStored` runs when that landed.
   */
  flush?: boolean;
  onStored?: () => void;
}

/** What the restore banner says about the draft the screen opened with. */
export interface Restored {
  at: string;
  /** Made from an older version than the item's: saving goes through the conflict flow. */
  stale: boolean;
}

/** What a 412 interrupted beyond the write itself. */
export interface ConflictContext {
  /**
   * The transition that was waiting for this save (F40), or the one refused itself. It
   * does not run: the conflict dialog says so, and the user runs it again once decided.
   */
  then?: Paused;
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
  /** The transition in flight (its save included), else `null`. */
  pending: Transition | null;
  /** The body as the server stored it (for the `invalid` plain-text view). */
  storedBody: unknown;
  /** Where the cursor goes when the screen opens (a hand-over keeps the user's place). */
  initialFocus: "title" | "body" | null;
  dirty: boolean;
  /** The explicit save's state (`saving` only while a `PATCH`/`POST` runs). */
  saveState: SaveStateValue;
  /** When the last save landed (or the item was last updated). */
  savedAt: Date | null;
  /** The save indicator: the explicit save and the autosave slot together. */
  indicator: SaveStateValue;
  /** When the indicator's state happened (the last save or autosave), else `null`. */
  indicatorAt: Date | null;
  /**
   * Changes that neither the item nor the slot holds yet: only then does closing or
   * reloading the tab ask first (F38).
   */
  unprotected: boolean;
  /** The screen opened with a restored draft that differs from the item, else `null`. */
  restored: Restored | null;
  /** The working draft as the slot would take it now. */
  draft: () => Draft;
  /** Whether anything differs from the item right now (not waiting for a render). */
  isDirtyNow: () => boolean;
  /**
   * The conflict dialog's Keep mine: takes `theirs` as the new base (its version is the
   * `If-Match`) and saves the draft over it. Resolves like `save()`.
   */
  keepMine: (theirs: ContentDto) => Promise<ContentDto | null>;
  /** Takes `theirs` as the new base without saving (the draft already equals it). */
  rebase: (theirs: ContentDto) => void;
  /**
   * Writes `draft` to the slot now (Take theirs keeps mine there). `asKept`: it is the
   * kept copy of this screen again (the kept Discard's Undo): autosave stops.
   */
  keepInSlot: (draft: Draft, options?: { asKept?: boolean }) => Promise<void>;
  /**
   * Drops the working draft's changes from the slot for good (the leave guard's and the
   * restore banner's Discard): nothing is written to it afterwards from this screen.
   */
  discard: () => Promise<void>;
  /** The kept copy in the slot (after Take theirs), else `null`. */
  kept: Draft | null;
  /** The kept copy is only in memory (writing it back to the slot failed). */
  keptUnstored: boolean;
  /**
   * Writes a kept copy that is only on this page to the slot again. `true` when the slot
   * holds it (or there is none to store), `false` when it still failed.
   */
  storeKept: () => Promise<boolean>;
  /** Deletes the kept copy; the working draft is autosaved from then on. */
  discardKept: () => Promise<void>;
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
  /**
   * Restores revision `rid` over the item (after a save or transition in flight). On
   * success the slot is gone and the autosaver stopped: the caller reopens the screen on
   * the item it resolves to (a kept copy is written back first). A `412` goes to
   * `onConflict` (op `revision`), a `401` to `onUnauthenticated`, anything else to
   * `onError`; then it resolves to `null` and the screen goes on as it was.
   */
  restoreRevision: (
    rid: string,
    options?: { onError?: (e: unknown) => void },
  ) => Promise<ContentDto | null>;
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

/**
 * What the editor opens `body` with: the stored document, or `undefined` (TipTap's own
 * empty document) for an empty one (`content: []`, what the API stores for an item
 * created without a body; not valid for the schema's `block+`).
 */
function editorContentOf(body: unknown): JSONContent | undefined {
  const b = body as JSONContent | null | undefined;
  return b &&
    typeof b === "object" &&
    b.type === "doc" &&
    Array.isArray(b.content) &&
    b.content.length > 0
    ? b
    : undefined;
}

/**
 * `body` as the editor would hold it after loading it (JSON text, attribute defaults
 * filled in): the dirty baseline of a body the editor does not show (an item under a
 * restored draft, the server's copy after a conflict). `""` when the schema refuses it,
 * so any draft counts as changed against it.
 */
function normalizedBody(editor: Editor, body: unknown): string {
  try {
    const content = editorContentOf(body);
    const node = content
      ? editor.schema.nodeFromJSON(content)
      : editor.schema.topNodeType.createAndFill();
    return node ? JSON.stringify(node.toJSON()) : "";
  } catch {
    return "";
  }
}

/** The panel members of a restored draft over `item` (`null` members: as stored). */
function metaOfDraft(draft: Draft, item: ContentDto | null): MetaDraft {
  const stored = metaOf(item);
  return {
    slug: draft.slug ?? stored.slug,
    // `null`: derive from the body, which the field shows as empty (F39).
    excerpt: draft.excerpt ?? "",
    terms: draft.terms ? [...draft.terms] : stored.terms,
    parent_id: draft.parent_id,
    menu_order: draft.menu_order ?? stored.menu_order,
  };
}

/** A draft as `PUT …/autosave` takes it. */
export function autosaveInput(d: Draft): SchemaAutosaveInput {
  return {
    base_version: d.base_version,
    title: d.title,
    excerpt: d.excerpt,
    body: d.body as SchemaAutosaveInput["body"],
    fields: (d.fields ?? {}) as SchemaAutosaveInput["fields"],
    slug: d.slug,
    parent_id: d.parent_id,
    menu_order: d.menu_order,
    terms: d.terms,
  };
}

/** A failure that sends the user to sign in (or to change the password) first. */
const needsSignIn = (e: unknown): e is ApiError =>
  isProblem(e, "unauthenticated") || isProblem(e, "password-reset-required");

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

  // A restored draft opens only on an editable screen, and only with a body this editor
  // can represent (it came from this editor; anything else stays in the slot untouched).
  const [restoring] = useState<RestoreSource | null>(() => {
    const r = opts.restore ?? null;
    if (!r || handoff || initialReadOnly !== null) return null;
    return validateBody(r.draft.body).ok ? r : null;
  });

  const [initialContent] = useState<JSONContent | undefined>(() => {
    if (handoff) return handoff.body;
    if (restoring) return editorContentOf(restoring.draft.body);
    if (initialBodyReadOnly?.kind === "unsupported") {
      return displayBody(storedBody) as JSONContent;
    }
    // TipTap's own empty document stands in for an empty one; the baseline below is
    // taken from that, so it is not "dirty".
    return editorContentOf(storedBody);
  });

  // Built once: the editor is created once per screen.
  const [extensions] = useState(() =>
    editorExtensions({ placeholder: opts.placeholder, resolveMedia: resolveMediaUrl }),
  );

  const [title, setTitleState] = useState(
    handoff?.title ?? restoring?.draft.title ?? opts.item?.title ?? "",
  );
  const titleRef = useRef(title);
  const [meta, setMetaState] = useState<MetaDraft>(
    () =>
      handoff?.meta ?? (restoring ? metaOfDraft(restoring.draft, opts.item) : metaOf(opts.item)),
  );
  const metaRef = useRef(meta);
  const [savedMeta, setSavedMeta] = useState<MetaDraft>(() => metaOf(opts.item));
  const savedMetaRef = useRef(savedMeta);
  const [savedAt, setSavedAt] = useState<Date | null>(
    opts.item ? new Date(opts.item.updated_at) : null,
  );
  // The `If-Match` of the next save: the item's version, or the older one a restored
  // draft was made from (so the server answers 412 and the conflict dialog decides).
  const baseVersion = useRef(
    restoring && opts.item && restoring.draft.base_version < opts.item.version
      ? restoring.draft.base_version
      : (opts.item?.version ?? 0),
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
  // "dirty"; after a hand-over, what the first save sent; under a restored draft, the
  // item's body as TipTap would hold it.
  const [baseline] = useState(() => {
    if (handoff) return handoff.saved;
    const itemTitle = opts.item?.title ?? "";
    if (restoring) return { title: itemTitle, body: normalizedBody(editor, storedBody) };
    return { title: itemTitle, body: JSON.stringify(editor.getJSON()) };
  });
  const saved = useRef(baseline);

  // A hand-over is dirty when the user typed while the first save was in flight; a
  // restored draft when it differs from the item.
  const [dirty, setDirty] = useState(() =>
    handoff || restoring
      ? title !== baseline.title ||
        JSON.stringify(editor.getJSON()) !== baseline.body ||
        Object.keys(metaChanges(meta, savedMeta)).length > 0
      : false,
  );
  const [saveState, setSaveState] = useState<SaveStateValue>(() => (dirty ? "dirty" : "saved"));
  const [restored, setRestored] = useState<Restored | null>(() =>
    restoring && dirty
      ? {
          at: restoring.at,
          stale: opts.item !== null && restoring.draft.base_version < opts.item.version,
        }
      : null,
  );

  const isDirtyNow = useCallback(() => {
    const base = saved.current;
    return (
      titleRef.current !== base.title ||
      JSON.stringify(editor.getJSON()) !== base.body ||
      Object.keys(metaChanges(metaRef.current, savedMetaRef.current)).length > 0
    );
  }, [editor]);

  /** The working draft, as the slot (and the conflict dialog) take it. */
  const draftNow = useCallback(
    (): Draft => ({
      base_version: baseVersion.current,
      title: titleRef.current,
      excerpt: metaRef.current.excerpt === "" ? null : metaRef.current.excerpt,
      body: editor.getJSON(),
      fields: itemRef.current?.fields ?? {},
      slug: metaRef.current.slug.trim() === "" ? null : metaRef.current.slug.trim(),
      parent_id: metaRef.current.parent_id,
      menu_order: metaRef.current.menu_order,
      terms: [...metaRef.current.terms],
    }),
    [editor],
  );

  // --- Autosave ------------------------------------------------------------------

  const autosaver = useRef<Autosaver | null>(null);
  // The kept copy occupies the slot: the working draft is not written over it.
  const [kept, setKeptState] = useState<Draft | null>(opts.kept ?? null);
  const keptRef = useRef(kept);
  // Whether the slot really holds the kept copy (a failed re-PUT leaves it in memory only).
  const [keptStored, setKeptStored] = useState(true);
  const setKept = useCallback((d: Draft | null) => {
    keptRef.current = d;
    setKeptState(d);
  }, []);
  const [autoState, setAutoState] = useState<AutosaveState>("saved");
  // A draft restored from the slot is already there.
  const [autoSavedAt, setAutoSavedAt] = useState<Date | null>(() =>
    restoring && !restoring.flush ? new Date(restoring.at) : null,
  );
  const itemId = opts.item?.id ?? null;
  const autosaves = itemId !== null && initialReadOnly === null;
  // A kept draft that differs from the item goes to the slot at once; one that does not
  // is done with, and a slot that equals the item is removed quietly (no banner).
  const [restoreFlush] = useState(() => (restoring?.flush && restored ? restoring : null));
  const [restoreDone] = useState(() => (restoring && !restored ? restoring : null));
  useEffect(() => {
    if (!restoreDone) return;
    if (restoreDone.flush) restoreDone.onStored?.();
    else if (itemId !== null && autosaves) void deleteAutosave(type, itemId).catch(() => undefined);
  }, [restoreDone, itemId, autosaves, type]);

  const fail = useCallback(
    (e: unknown, op: WriteOp, onError?: (e: unknown) => void, context: ConflictContext = {}) => {
      if (needsSignIn(e)) {
        // Not an error to show: the draft is kept and the user signs in again (F37). The
        // slot cannot be written without a session: the autosaver stops.
        autosaver.current?.dispose();
        latest.current.onUnauthenticated?.(e, draftNow());
      } else if (isProblem(e, "precondition-failed")) {
        latest.current.onConflict?.(e, op, context);
      } else if (onError) onError(e);
      else latest.current.onWriteError?.(e, op);
    },
    [draftNow],
  );

  useEffect(() => {
    if (!autosaves || itemId === null) return;
    const a = createAutosaver({
      intervalMs: AUTOSAVE_INTERVAL_MS,
      save: async (d) => {
        await putAutosave(type, itemId, autosaveInput(d));
      },
      onError: (e) => {
        if (!needsSignIn(e)) return;
        a.dispose();
        latest.current.onUnauthenticated?.(e, draftNow());
      },
    });
    autosaver.current = a;
    const off = a.state.subscribe((s) => {
      setAutoState(s);
      const at = a.savedAt();
      if (s === "saved" && at !== null) setAutoSavedAt(new Date(at));
    });
    // A draft kept over a sign-in goes to the slot at once (then the banner is true).
    if (restoreFlush) {
      a.markDirty(draftNow);
      void a.flush().then(() => {
        if (a.state.get() === "saved") restoreFlush.onStored?.();
      });
    }
    return () => {
      off();
      if (autosaver.current === a) autosaver.current = null;
      // Leaving the screen writes what the slot does not hold yet (a lazy draft is read
      // now, while the editor still exists), then stops.
      const s = a.state.get();
      if (s !== "saved" && s !== "error") {
        try {
          a.markDirty(draftNow());
        } catch {
          // The editor is gone: nothing newer than the last write to keep.
        }
      }
      void a.flush().finally(() => a.dispose());
    };
  }, [autosaves, itemId, type, draftNow, restoreFlush]);

  /** A change of the draft: the slot follows it (never on a read-only screen). */
  const touch = useCallback(() => {
    if (readOnlyRef.current === null && keptRef.current === null) {
      autosaver.current?.markDirty(draftNow);
    }
  }, [draftNow]);

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
      touch();
    };
    editor.on("update", onUpdate);
    return () => {
      editor.off("update", onUpdate);
    };
  }, [editor, touch]);

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
      touch();
    },
    [recompute, touch],
  );

  const setMeta = useCallback(
    (patch: Partial<MetaDraft>) => {
      metaRef.current = { ...metaRef.current, ...patch };
      setMetaState(metaRef.current);
      recompute();
      touch();
    },
    [recompute, touch],
  );

  /** The server's copy of the item, everywhere (screen, cache, lists, counts, parents). */
  const adopt = useCallback(
    (next: ContentDto) => {
      // In step with the item (no older restored base waiting for the conflict flow):
      // the next save goes with this version.
      if (baseVersion.current === itemRef.current?.version) baseVersion.current = next.version;
      itemRef.current = next;
      setItem(next);
      queryClient.setQueryData(qk.content.item(type, next.id), next);
      // Children show this item as their parent by title (Task 15 carry): keep it fresh.
      queryClient.setQueryData(qk.content.parentRef(type, next.id), {
        title: next.title,
        slug: next.slug,
      });
      // Every save adds a revision (the panel's "Revisions (n)", the drawer's list).
      // `exact`: a revision itself never changes.
      void queryClient.invalidateQueries({
        queryKey: qk.content.revisions(type, next.id),
        exact: true,
      });
      void queryClient.invalidateQueries({ queryKey: qk.content.lists(type) });
      void queryClient.invalidateQueries({ queryKey: qk.content.counts(type) });
    },
    [queryClient, type],
  );

  const inflight = useRef<Promise<ContentDto | null> | null>(null);
  // One write at a time: a transition (its save and its lifecycle call) and a save never
  // overlap, so no request goes out with a version another one is about to replace.
  const transitioning = useRef<Promise<ContentDto | null> | null>(null);
  // The transition waiting for the save in flight, also when it joined a save that was
  // already running (its 412 must still name it, Task 15 carry).
  const waiting = useRef<Transition | null>(null);
  const [pending, setPending] = useState<Transition | null>(null);

  const saveNow = useCallback(
    (then?: Transition): Promise<ContentDto | null> => {
      if (readOnlyRef.current) return Promise.resolve(null);
      if (inflight.current) return inflight.current;
      const current = itemRef.current;
      if (current && !isDirtyNow()) {
        setDirty(false);
        setSaveState("saved");
        return Promise.resolve(current);
      }
      setSaveState("saving");
      const run = async (): Promise<ContentDto | null> => {
        // A running autosave lands first and none starts until this save is done: no
        // `PUT` can recreate the slot after the `PATCH` cleared it (Task 3 carry).
        const release = (await autosaver.current?.hold()) ?? (() => undefined);
        // What is sent: the draft as it is now (keystrokes during the wait included).
        const sentTitle = titleRef.current;
        const doc = editor.getJSON();
        const sentBody = JSON.stringify(doc);
        const sentMeta = metaRef.current;
        const base = saved.current;
        const metaPatch = metaChanges(sentMeta, savedMetaRef.current);
        try {
          let next: ContentDto;
          if (current) {
            // The changed members only.
            const patch: SchemaContentPatch = { ...metaPatch };
            if (base.title !== sentTitle) patch.title = sentTitle;
            if (base.body !== sentBody) patch.body = asBody(doc);
            // Undone while the autosave landed: nothing to write after all.
            if (Object.keys(patch).length === 0) {
              setDirty(false);
              setSaveState("saved");
              return current;
            }
            next = await patchContent(type, current.id, baseVersion.current, patch);
          } else {
            const input: SchemaContentInput = { title: sentTitle, body: asBody(doc) };
            if (metaPatch.slug !== undefined) input.slug = metaPatch.slug;
            if (metaPatch.excerpt) input.excerpt = metaPatch.excerpt;
            if (metaPatch.terms && metaPatch.terms.length > 0) input.terms = metaPatch.terms;
            if (metaPatch.parent_id) input.parent_id = metaPatch.parent_id;
            if (metaPatch.menu_order !== undefined) input.menu_order = metaPatch.menu_order;
            next = await createContent(type, input);
          }
          baseVersion.current = next.version;
          saved.current = { title: sentTitle, body: sentBody };
          const stored = metaOf(next);
          savedMetaRef.current = stored;
          setSavedMeta(stored);
          metaRef.current = settleMeta(metaRef.current, sentMeta, stored);
          setMetaState(metaRef.current);
          adopt(next);
          setSavedAt(new Date());
          setRestored(null);
          const still = !isDirtyNow();
          setDirty(!still);
          setSaveState(still ? "saved" : "dirty");
          // The server deleted the slot: nothing to write, except edits made meanwhile
          // (now relative to the new version). A kept copy goes back at once (still
          // held, so nothing can come between).
          autosaver.current?.markClean();
          const keptCopy = keptRef.current;
          if (keptCopy && current) {
            try {
              await putAutosave(type, current.id, autosaveInput(keptCopy));
              setKeptStored(true);
            } catch (e) {
              // Still in memory: the banner says it is only on this page, and leaving or
              // closing the tab asks first.
              setKeptStored(false);
              if (needsSignIn(e)) {
                autosaver.current?.dispose();
                latest.current.onUnauthenticated?.(e, keptCopy);
              }
            }
          }
          if (!still) touch();
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
          // A conflict is a decision to make, not a failure: the dialog says so.
          setSaveState(
            e instanceof NetworkError
              ? "offline"
              : isProblem(e, "precondition-failed")
                ? "dirty"
                : "error",
          );
          const waitingFor = then ?? waiting.current ?? undefined;
          fail(e, "save", undefined, waitingFor ? { then: waitingFor } : {});
          return null;
        } finally {
          release();
          inflight.current = null;
        }
      };
      const p = run();
      inflight.current = p;
      return p;
    },
    [editor, type, adopt, fail, isDirtyNow, touch],
  );

  /** A save waits for a transition in flight, then saves what is still unsaved. */
  const save = useCallback((): Promise<ContentDto | null> => {
    const running = transitioning.current;
    return running ? running.then(() => saveNow()) : saveNow();
  }, [saveNow]);

  const transition = useCallback(
    (action: Transition, options: TransitionOptions = {}): Promise<ContentDto | null> => {
      if (readOnlyRef.current) return Promise.resolve(null);
      // A second transition while one runs (a shortcut) does nothing.
      if (transitioning.current) return Promise.resolve(null);
      const run = async (): Promise<ContentDto | null> => {
        // Unsaved changes first (F40); a save that joined one already in flight may
        // not hold the latest keystrokes, so save again while anything is unsaved.
        waiting.current = action;
        let current: ContentDto | null;
        try {
          current = await saveNow(action);
          if (current && isDirtyNow()) current = await saveNow(action);
        } finally {
          waiting.current = null;
        }
        if (!current || itemRef.current === null) return null;
        try {
          const next =
            action === "schedule"
              ? await lifecycle(type, current.id, "schedule", current.version, {
                  at: options.at ?? "",
                })
              : await lifecycle(type, current.id, action, current.version);
          baseVersion.current = next.version;
          adopt(next);
          return next;
        } catch (e) {
          fail(e, action, options.onError, { then: action });
          return null;
        }
      };
      setPending(action);
      const p = run().finally(() => {
        transitioning.current = null;
        setPending(null);
      });
      transitioning.current = p;
      return p;
    },
    [saveNow, isDirtyNow, type, adopt, fail],
  );

  /** `theirs` becomes what the draft is compared with and saved over. */
  const rebase = useCallback(
    (theirs: ContentDto) => {
      baseVersion.current = theirs.version;
      saved.current = { title: theirs.title, body: normalizedBody(editor, theirs.body) };
      const stored = metaOf(theirs);
      savedMetaRef.current = stored;
      setSavedMeta(stored);
      adopt(theirs);
      setSavedAt(new Date(theirs.updated_at));
      setRestored(null);
      recompute();
    },
    [editor, adopt, recompute],
  );

  const keepMine = useCallback(
    (theirs: ContentDto): Promise<ContentDto | null> => {
      rebase(theirs);
      return saveNow();
    },
    [rebase, saveNow],
  );

  const keepInSlot = useCallback(
    async (d: Draft, options: { asKept?: boolean } = {}): Promise<void> => {
      if (itemId === null) return;
      const a = autosaver.current;
      const release = (await a?.hold()) ?? (() => undefined);
      try {
        await putAutosave(type, itemId, autosaveInput(d));
        // The slot holds this draft: nothing left to write when the screen goes.
        a?.markClean();
        if (options.asKept) {
          setKept(d);
          setKeptStored(true);
        }
      } catch (e) {
        if (needsSignIn(e)) {
          a?.dispose();
          latest.current.onUnauthenticated?.(e, d);
        }
        throw e;
      } finally {
        release();
      }
    },
    [itemId, type, setKept],
  );

  const discard = useCallback(async (): Promise<void> => {
    const a = autosaver.current;
    if (itemId === null || keptRef.current !== null) {
      // No slot (a new item), or the slot holds the kept copy, not these edits: it
      // stays (only its own Discard removes it). The edits are dropped with the screen.
      a?.markClean();
      a?.dispose();
      return;
    }
    // A running PUT lands first; none starts while the DELETE runs. Only once the DELETE
    // succeeded is the autosaver done; a failed one leaves it as it was (still dirty and
    // guarded), so nothing the user types next goes unsaved.
    const release = (await a?.hold()) ?? (() => undefined);
    try {
      await deleteAutosave(type, itemId);
      a?.markClean();
      a?.dispose();
    } finally {
      release();
    }
  }, [itemId, type]);

  const storeKept = useCallback(async (): Promise<boolean> => {
    const copy = keptRef.current;
    if (copy === null || keptStored || itemId === null) return true;
    const a = autosaver.current;
    const release = (await a?.hold()) ?? (() => undefined);
    try {
      await putAutosave(type, itemId, autosaveInput(copy));
      setKeptStored(true);
      return true;
    } catch (e) {
      if (needsSignIn(e)) {
        a?.dispose();
        latest.current.onUnauthenticated?.(e, copy);
      }
      return false;
    } finally {
      release();
    }
  }, [keptStored, itemId, type]);

  const discardKept = useCallback(async (): Promise<void> => {
    if (itemId === null) return;
    const a = autosaver.current;
    const release = (await a?.hold()) ?? (() => undefined);
    try {
      await deleteAutosave(type, itemId);
      setKept(null);
    } finally {
      release();
    }
    // The slot is free: the working draft goes there from now on.
    if (isDirtyNow()) touch();
  }, [itemId, type, setKept, isDirtyNow, touch]);

  const restoreRevisionNow = useCallback(
    async (rid: string, options: { onError?: (e: unknown) => void } = {}) => {
      // A save or transition in flight lands first (its version is the one to match).
      await transitioning.current;
      await inflight.current;
      const current = itemRef.current;
      if (!current || !current.allowed.edit) return null;
      const a = autosaver.current;
      // A running autosave lands first and none starts while the restore runs.
      const release = (await a?.hold()) ?? (() => undefined);
      try {
        const next = await restoreRevision(type, current.id, rid, current.version);
        // The server deleted this user's slot with the restore: the old working draft
        // must not go back there (not by the timer, not when the screen goes).
        a?.markClean();
        a?.dispose();
        baseVersion.current = next.version;
        const keptCopy = keptRef.current;
        if (keptCopy) {
          // The kept copy (after Take theirs) was in that slot: it goes back, as after a save.
          try {
            await putAutosave(type, current.id, autosaveInput(keptCopy));
          } catch (e) {
            setKeptStored(false);
            if (needsSignIn(e)) latest.current.onUnauthenticated?.(e, keptCopy);
          }
        }
        adopt(next);
        return next;
      } catch (e) {
        fail(e, "revision", options.onError, { then: "revision" });
        return null;
      } finally {
        release();
      }
    },
    [type, adopt, fail],
  );

  // The indicator: an explicit save in flight or failed comes first; then, with
  // changes the item does not hold, what the slot holds.
  let indicator: SaveStateValue;
  let indicatorAt: Date | null = null;
  if (saveState === "saving" || saveState === "error" || saveState === "offline") {
    indicator = saveState;
  } else if (!dirty) {
    indicator = "saved";
    indicatorAt = savedAt;
  } else if (!autosaves || kept) {
    indicator = "dirty";
  } else if (autoState === "saved") {
    indicator = autoSavedAt ? "autosaved" : "dirty";
    indicatorAt = autoSavedAt;
  } else {
    indicator = autoState;
  }
  const keptUnstored = kept !== null && !keptStored;
  const unprotected =
    keptUnstored ||
    (dirty &&
      readOnly === null &&
      !(autosaves && kept === null && autoState === "saved" && autoSavedAt !== null));

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
    pending,
    storedBody,
    initialFocus,
    dirty,
    saveState,
    savedAt,
    indicator,
    indicatorAt,
    unprotected,
    restored,
    draft: draftNow,
    isDirtyNow,
    keepMine,
    rebase,
    keepInSlot,
    discard,
    kept,
    keptUnstored,
    storeKept,
    discardKept,
    save,
    transition,
    adopt,
    restoreRevision: restoreRevisionNow,
  };
}
