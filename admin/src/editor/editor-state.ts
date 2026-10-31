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
// Custom fields and SEO (M2b-2 Task 8) are panel members too: `meta.fields` (the item's
// `fields` object as edited, `featured_image` included) and `meta.seo` (title,
// description and canonical URL as typed, the robots directives sorted; M3-2 P33). With
// a field schema (`fieldDefs`), a save sends the fields without the keys the schema no
// longer has (`toSave`, P39) and is refused here, before any request, while a field is
// invalid (a required one empty: Review Focus 2); the server's `422` paths land in
// `fieldErrors` for the inputs to show.
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
import { typesQuery } from "@/api/types";
import type {
  SchemaAutosaveInput,
  SchemaContentDto,
  SchemaContentInput,
  SchemaContentPatch,
  SchemaRobotsDto,
  SchemaSeoPatchDto,
} from "@/api/schema";
import type { SaveStateValue } from "@/components/kit/save-state";
import { sameList } from "@/lib/same-list";
import { pushRecent } from "@/palette/recent";
import {
  AUTOSAVE_INTERVAL_MS,
  type AutosaveState,
  type Autosaver,
  type Draft,
  createAutosaver,
} from "./autosave";
import { sortedRobots } from "./conflict";
import {
  type FieldDef,
  type FieldValues,
  fieldIssues,
  isObject,
  sameValue,
  serverFieldErrors,
  toSave,
  valuesOf,
} from "./fields/fields-model";
import type { Handoff } from "./handoff";
import { type Backup, type BackupKey, clearBackup, writeBackup } from "./local-backup";
import { ImageUploads } from "./insert-image";
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
  /** The item's custom fields as edited (`featured_image` included). */
  fields: FieldValues;
  /** The SEO title, description and canonical URL as typed (`""`: not set), and the
   * robots directives. */
  seo: SeoDraft;
}

/** The SEO members of the draft (`""`: not set; the server trims what is sent). */
export interface SeoDraft {
  title: string;
  description: string;
  canonical: string;
  /** Sorted, as the server stores them; empty: none. */
  robots: SchemaRobotsDto[];
}

/** The server's limit for `seo.canonical`. */
export const MAX_CANONICAL = 2048;

/**
 * `text` trimmed the way the server trims SEO values (Rust `str::trim`: Unicode
 * White_Space). Unlike JS `trim()` it keeps U+FEFF (a BOM is not White_Space).
 */
export function trimSeo(text: string): string {
  return text.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
}

/**
 * `text` cut so the server's limit holds: at most `max` characters (code points, as the
 * server counts) once trimmed. Edge whitespace is kept (the server trims it away); an
 * over-long paste is cut at a code-point boundary.
 */
export function clampSeo(text: string, max: number): string {
  if (seoLength(text) <= max) return text;
  const lead = [...(/^\p{White_Space}*/u.exec(text)?.[0] ?? "")].length;
  return [...text].slice(0, lead + max).join("");
}

/** The length the server checks: code points of the trimmed text. */
export function seoLength(text: string): number {
  return [...trimSeo(text)].length;
}

/**
 * The value an edit from `prev` to `next` leaves under the limit `max`, as a native
 * `maxLength` would: an edit that fits is kept; one made while `prev` is already at the
 * limit is refused (`prev`, so typing inside full text does not push its end off); one
 * that jumps past it (a paste) is cut by {@link clampSeo}.
 */
export function limitSeoEdit(prev: string, next: string, max: number): string {
  if (seoLength(next) <= max) return next;
  if (seoLength(prev) >= max) return prev;
  return clampSeo(next, max);
}

/** The text an `insert*` input event puts in (a line break as `"\n"`). */
function insertedText(e: InputEvent): string {
  if (e.inputType === "insertLineBreak" || e.inputType === "insertParagraph") return "\n";
  return e.data ?? e.dataTransfer?.getData("text/plain") ?? "";
}

/**
 * A `beforeinput` listener that keeps the field within `max` (as {@link seoLength}
 * counts) before anything lands, so the caret never jumps, as a native `maxLength`:
 * any insert (typing, Enter, a replacement, a paste, a drop) that would pass the limit
 * is refused; a paste or drop is cut to what fits (at a code-point boundary) and put in
 * at the caret. Composition (IME) cannot be cancelled; {@link limitSeoEdit} still holds
 * the limit then.
 */
export function refuseSeoOverflow(max: number): (e: Event) => void {
  return (e) => {
    const input = e as InputEvent;
    if (!input.inputType.startsWith("insert") || input.inputType === "insertCompositionText") {
      return;
    }
    const el = e.target as HTMLInputElement | HTMLTextAreaElement;
    const start = el.selectionStart ?? el.value.length;
    const end = el.selectionEnd ?? start;
    const before = el.value.slice(0, start);
    const rest = el.value.slice(end);
    const text = insertedText(input);
    if (seoLength(before + text + rest) <= max) return;
    e.preventDefault();
    if (input.inputType !== "insertFromPaste" && input.inputType !== "insertFromDrop") return;
    // The longest start of the pasted text (in code points, so never half a surrogate
    // pair) that still fits. The trimmed length never shrinks as the start grows, so
    // bisection finds it in O(log n) length checks, not one per pasted character.
    const chars = [...text];
    const fits = (n: number) => seoLength(before + chars.slice(0, n).join("") + rest) <= max;
    let fit = 0;
    for (let hi = chars.length - 1; fit < hi;) {
      const mid = Math.ceil((fit + hi) / 2);
      if (fits(mid)) fit = mid;
      else hi = mid - 1;
    }
    if (fit <= 0) return;
    el.setRangeText(chars.slice(0, fit).join(""), start, end, "end");
    el.dispatchEvent(new Event("input", { bubbles: true }));
  };
}

/**
 * Whether the server takes `text` as `seo.canonical` (core `check_canonical`): blank (it
 * deletes the value), or, trimmed, an absolute `http(s)` URL with a host, without
 * whitespace (Unicode White_Space) or control characters (Cc), at most
 * {@link MAX_CANONICAL} characters.
 */
export function canonicalAccepted(text: string): boolean {
  const s = trimSeo(text);
  if (s === "") return true;
  const bad = /[\p{White_Space}\p{Cc}]/u;
  if ([...s].length > MAX_CANONICAL || bad.test(s)) return false;
  try {
    const url = new URL(s);
    return (url.protocol === "http:" || url.protocol === "https:") && url.hostname !== "";
  } catch {
    return false;
  }
}

const sameRobots = (a: readonly SchemaRobotsDto[], b: readonly SchemaRobotsDto[]) =>
  sameList(sortedRobots(a), sortedRobots(b));

/** The SEO a reset leaves: nothing set (the page then uses its own values). */
export const EMPTY_SEO: SeoDraft = { title: "", description: "", canonical: "", robots: [] };

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
        fields: { ...valuesOf(item.fields) },
        seo: {
          title: item.seo?.title ?? "",
          description: item.seo?.description ?? "",
          canonical: item.seo?.canonical ?? "",
          robots: sortedRobots(item.seo?.robots ?? []),
        },
      }
    : {
        slug: "",
        excerpt: "",
        terms: [],
        parent_id: null,
        menu_order: 0,
        fields: {},
        seo: EMPTY_SEO,
      };
}

/** A typed SEO value as the API takes it: blank (as the server trims) is `null` (delete). */
const seoValue = (s: string): string | null => (trimSeo(s) === "" ? null : s);

/** The directives as the API takes them: none is `null` (delete). */
const robotsValue = (robots: readonly SchemaRobotsDto[]): SchemaRobotsDto[] | null =>
  robots.length === 0 ? null : sortedRobots(robots);

/** The SEO members as the slot and the conflict dialog take them. */
export const seoOfDraft = (seo: SeoDraft) => ({
  title: seoValue(seo.title),
  description: seoValue(seo.description),
  canonical: seoValue(seo.canonical),
  robots: robotsValue(seo.robots),
});

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
  if (!sameValue(draft.fields, base.fields))
    out.fields = draft.fields as SchemaContentPatch["fields"];
  const seo: SchemaSeoPatchDto = {};
  if (draft.seo.title !== base.seo.title) seo.title = seoValue(draft.seo.title);
  if (draft.seo.description !== base.seo.description) {
    seo.description = seoValue(draft.seo.description);
  }
  if (draft.seo.canonical !== base.seo.canonical) seo.canonical = seoValue(draft.seo.canonical);
  if (!sameRobots(draft.seo.robots, base.seo.robots)) seo.robots = robotsValue(draft.seo.robots);
  if (Object.keys(seo).length > 0) out.seo = seo;
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
    fields: now.fields === sent.fields ? stored.fields : now.fields,
    seo: {
      title: now.seo.title === sent.seo.title ? stored.seo.title : now.seo.title,
      description:
        now.seo.description === sent.seo.description ? stored.seo.description : now.seo.description,
      canonical:
        now.seo.canonical === sent.seo.canonical ? stored.seo.canonical : now.seo.canonical,
      robots: sameRobots(now.seo.robots, sent.seo.robots) ? stored.seo.robots : now.seo.robots,
    },
  };
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
  /**
   * Whose screen it is (`me.id`): the key of the local backup (`local-backup.ts`) that
   * holds new edits while a kept copy occupies the slot. `null`: no backup.
   */
  backupUser?: string | null;
  /**
   * This browser's backup of the item as found when the screen opened: offered (as
   * `backup`) when it differs from both the item and the slot, dropped when it does not.
   */
  backup?: Backup | null;
  /**
   * The type's field schema (`null`: none, the fields are kept as stored). Read when a
   * save starts, so a refreshed schema applies from the next save on.
   */
  fieldDefs?: FieldDef[] | null;
  /**
   * A save was refused because of field values: by the check here (`client`, nothing was
   * sent) or by the server's `422` (`server`, which `onWriteError` reports too).
   */
  onInvalid?: (source: "client" | "server") => void;
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
  /**
   * This browser's backup, opened under a kept copy (`kept`): it is in neither the slot
   * nor the item, so it is no "restored" draft (the kept banner shows) and the local
   * backup goes on holding it.
   */
  local?: boolean;
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

export interface RestoreRevisionOptions {
  /** A failure that is not a `412` or a `401` (also: the account may no longer edit). */
  onError?: (e: unknown) => void;
  /**
   * The `If-Match` to send instead of the item's version: an Undo sends the version the
   * restore produced, so any save after it answers `412` (the conflict flow) instead of
   * being reverted unseen.
   */
  version?: number;
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
  /** Sets one top-level custom field (`featured_image` included) in the draft. */
  setField: (name: string, value: unknown) => void;
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
  /** This browser's backup when it differs from the item and the slot (offered), else `null`. */
  backup: Backup | null;
  /** Deletes the local backup and stops offering it. */
  dropBackup: () => void;
  /** The backup on offer right now (a reopened screen offers it again), else `null`. */
  backupNow: () => Backup | null;
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
  restoreRevision: (rid: string, options?: RestoreRevisionOptions) => Promise<ContentDto | null>;
  /**
   * A revision restore is running: the body, the title and the panel are locked, and no
   * save or transition starts (what is typed now would be replaced unseen).
   */
  restoring: boolean;
  /** The type's field schema, as passed (`null`: none). */
  fieldDefs: FieldDef[] | null;
  /** The server's last `422` messages by input path (`fields.price`, `seo.title`). */
  fieldErrors: Record<string, string>;
  /** Drops the server's message of `path` (its input changed). */
  clearFieldError: (path: string) => void;
  /** A save was refused for field values since the last successful save. */
  saveAttempted: boolean;
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
    fields: isObject(draft.fields) ? { ...draft.fields } : stored.fields,
    seo: draft.seo
      ? {
          title: draft.seo.title === undefined ? stored.seo.title : (draft.seo.title ?? ""),
          description:
            draft.seo.description === undefined
              ? stored.seo.description
              : (draft.seo.description ?? ""),
          // A slot from before M3-2 has neither: the stored ones stay.
          canonical:
            draft.seo.canonical === undefined ? stored.seo.canonical : (draft.seo.canonical ?? ""),
          robots:
            draft.seo.robots === undefined
              ? stored.seo.robots
              : sortedRobots(draft.seo.robots ?? []),
        }
      : stored.seo,
  };
}

/**
 * A draft as `PUT …/autosave` takes it. The slot checks `seo` as a save does, so a
 * canonical URL the server would refuse (one half typed) is left out, "unchanged": a
 * `422` would refuse the whole draft. The field still holds it and says what is wrong.
 */
export function autosaveInput(d: Draft): SchemaAutosaveInput {
  let seo = d.seo;
  if (seo && typeof seo.canonical === "string" && !canonicalAccepted(seo.canonical)) {
    seo = { ...seo };
    delete seo.canonical;
  }
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
    ...(seo ? { seo } : {}),
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
  const [extensions] = useState(() => [
    ...editorExtensions({ placeholder: opts.placeholder, resolveMedia: resolveMediaUrl }),
    ImageUploads,
  ]);

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
    restoring && dirty && !restoring.local
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
      fields: metaRef.current.fields,
      seo: seoOfDraft(metaRef.current.seo),
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
  // Read through the ref: a caller holding an older render (the leave dialog's Save and
  // leave, right after `save()`'s own re-PUT) must not write it a second time.
  const [keptStored, setKeptStoredState] = useState(true);
  const keptStoredRef = useRef(keptStored);
  const setKeptStored = useCallback((stored: boolean) => {
    keptStoredRef.current = stored;
    setKeptStoredState(stored);
  }, []);
  const setKept = useCallback((d: Draft | null) => {
    keptRef.current = d;
    setKeptState(d);
  }, []);
  const [autoState, setAutoState] = useState<AutosaveState>("saved");
  // A draft restored from the slot is already there.
  const [autoSavedAt, setAutoSavedAt] = useState<Date | null>(() =>
    restoring && !restoring.flush && !restoring.local ? new Date(restoring.at) : null,
  );
  const itemId = opts.item?.id ?? null;
  const autosaves = itemId !== null && initialReadOnly === null;
  // A kept draft that differs from the item goes to the slot at once; one that does not
  // is done with, and a slot that equals the item is removed quietly (no banner).
  const [restoreFlush] = useState(() => (restoring?.flush && restored ? restoring : null));
  const [restoreDone] = useState(() =>
    restoring && !restored && !restoring.local ? restoring : null,
  );
  useEffect(() => {
    if (!restoreDone) return;
    if (restoreDone.flush) restoreDone.onStored?.();
    else if (itemId !== null && autosaves) void deleteAutosave(type, itemId).catch(() => undefined);
  }, [restoreDone, itemId, autosaves, type]);

  // Whether the slot may hold a draft other than the item: a restored one, or one the
  // autosaver (or `keepInSlot`) wrote since. A save, a discard or a restore empties it.
  const slotHolds = useRef(restored !== null);

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
        slotHolds.current = true;
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

  // --- Local backup (R13, P40) ----------------------------------------------------

  const backupUser = opts.backupUser ?? null;
  const backupKey = useMemo<BackupKey | null>(
    () => (itemId !== null && backupUser !== null ? { item: itemId, user: backupUser } : null),
    [itemId, backupUser],
  );
  // The backup found on opening: offered only when it is neither the item nor the slot
  // (what the screen restored, or the kept copy) and this screen may edit it.
  const [backupCheck] = useState(() => {
    const b = opts.backup ?? null;
    if (!b || handoff || initialReadOnly !== null || !validateBody(b.draft.body).ok) {
      return { offered: null, redundant: false };
    }
    const shape = (d: Draft) => ({
      title: d.title,
      body: normalizedBody(editor, d.body),
      meta: metaOfDraft(d, opts.item),
    });
    const mine = shape(b.draft);
    const same = (other: { title: string; body: string; meta: MetaDraft }) =>
      other.title === mine.title &&
      other.body === mine.body &&
      Object.keys(metaChanges(other.meta, mine.meta)).length === 0 &&
      Object.keys(metaChanges(mine.meta, other.meta)).length === 0;
    const slots = [opts.restore?.draft, opts.kept].flatMap((d) => (d ? [shape(d)] : []));
    const item = {
      title: opts.item?.title ?? "",
      body: normalizedBody(editor, storedBody),
      meta: metaOf(opts.item),
    };
    const redundant = same(item) || slots.some(same);
    return { offered: redundant ? null : b, redundant };
  });
  const [backup, setBackupState] = useState<Backup | null>(backupCheck.offered);
  const backupRef = useRef(backup);
  const setBackup = useCallback((b: Backup | null) => {
    backupRef.current = b;
    setBackupState(b);
  }, []);
  useEffect(() => {
    // Nothing in it that the item or the slot does not hold already.
    if (backupCheck.redundant && backupKey) clearBackup(backupKey);
  }, [backupCheck, backupKey]);
  const dropBackup = useCallback(() => {
    if (backupKey) clearBackup(backupKey);
    setBackup(null);
  }, [backupKey, setBackup]);
  const backupNow = useCallback(() => backupRef.current, []);

  /**
   * The draft is back at what the item holds: nothing is left to autosave, and a draft
   * the slot took earlier goes, or a reload would open that older draft as a restored
   * one under a "Saved" indicator (CI run 40404431115). A running write lands first and
   * none starts while the `DELETE` runs; an edit meanwhile is written after it.
   */
  const clearing = useRef(false);
  const clearSlot = useCallback(() => {
    const a = autosaver.current;
    if (!a || a.disposed()) return;
    // A write running now may be about to put an older draft in the slot: it is waited
    // for below, like one that already landed.
    const writing = a.state.get() === "saving";
    a.markClean();
    if ((!slotHolds.current && !writing) || clearing.current || itemId === null) return;
    clearing.current = true;
    void (async () => {
      const release = await a.hold();
      try {
        // Stopped meanwhile (the waited write met a lost session): no further request.
        if (!a.disposed() && slotHolds.current && !isDirtyNow()) {
          await deleteAutosave(type, itemId);
          slotHolds.current = false;
        }
      } catch (e) {
        if (needsSignIn(e)) {
          // The session ended: the draft is kept for after the sign-in (F37).
          a.dispose();
          latest.current.onUnauthenticated?.(e, draftNow());
        }
        // Otherwise it stays: the next edit writes over it, the next save deletes it.
      } finally {
        clearing.current = false;
        release();
      }
    })();
  }, [itemId, type, isDirtyNow, draftNow]);

  /**
   * A change of the draft: the slot follows it (never on a read-only screen); while a
   * kept copy holds the slot, the local backup does (cleared when nothing is unsaved).
   * `known`: whether the draft differs from the item, when the caller has just checked.
   */
  const touch = useCallback(
    (known?: boolean) => {
      if (readOnlyRef.current !== null) return;
      if (keptRef.current === null) {
        if (known ?? isDirtyNow()) autosaver.current?.markDirty(draftNow);
        else clearSlot();
      }
      // A backup still on offer is not written over: the user has not chosen yet.
      else if (backupKey && backupRef.current === null) {
        if (known ?? isDirtyNow()) writeBackup(backupKey, draftNow());
        else clearBackup(backupKey);
      }
    },
    [draftNow, backupKey, isDirtyNow, clearSlot],
  );

  /** Whether the draft differs from the item now, into `dirty` and the save state. */
  const recompute = useCallback((): boolean => {
    const isDirty = isDirtyNow();
    setDirty(isDirty);
    setSaveState((s) => (s === "saving" ? s : isDirty ? "dirty" : "saved"));
    return isDirty;
  }, [isDirtyNow]);

  // A transition can take the right to edit away (or the item into the trash).
  useEffect(() => {
    if (readOnly === null || editor.isDestroyed || !editor.isEditable) return;
    editor.setEditable(false, false);
    editor.view.dom.setAttribute("aria-readonly", "true");
  }, [editor, readOnly]);

  useEffect(() => {
    // A body edit undone back to the item is "Saved" again (e1cd3a5 re-review).
    const onUpdate = () => touch(recompute());
    editor.on("update", onUpdate);
    return () => {
      editor.off("update", onUpdate);
    };
  }, [editor, touch, recompute]);

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
      touch(recompute());
    },
    [recompute, touch],
  );

  const setField = useCallback(
    (name: string, value: unknown) => {
      setMeta({ fields: { ...metaRef.current.fields, [name]: value } });
    },
    [setMeta],
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

  // The server's `422` messages by input path, and whether a save was refused for field
  // values (required fields then show their message even before they are touched).
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [saveAttempted, setSaveAttempted] = useState(false);
  const clearFieldError = useCallback((path: string) => {
    setFieldErrors((errors) => {
      if (!(path in errors)) return errors;
      const next = { ...errors };
      delete next[path];
      return next;
    });
  }, []);

  // A revision restore runs (`restoreRevision`): nothing else writes meanwhile.
  const restoringRef = useRef(false);
  const [revisionBusy, setRevisionBusy] = useState(false);
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
      if (readOnlyRef.current || restoringRef.current) return Promise.resolve(null);
      if (inflight.current) return inflight.current;
      const current = itemRef.current;
      if (current && !isDirtyNow()) {
        setDirty(false);
        setSaveState("saved");
        return Promise.resolve(current);
      }
      // Field values core would refuse: nothing is sent, the inputs say why (a required
      // field left empty included, Review Focus 2).
      const defs = latest.current.fieldDefs ?? null;
      if (defs && Object.keys(fieldIssues(defs, toSave(defs, metaRef.current.fields))).length > 0) {
        setSaveAttempted(true);
        setSaveState("error");
        latest.current.onInvalid?.("client");
        return Promise.resolve(null);
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
        if (defs) {
          // Keys the schema no longer has are dropped (P39): every write is checked
          // against the schema, so they go with the next save, whatever it changes.
          const clean = toSave(defs, sentMeta.fields);
          if (sameValue(clean, savedMetaRef.current.fields)) delete metaPatch.fields;
          else metaPatch.fields = clean as SchemaContentPatch["fields"];
        }
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
            if (metaPatch.fields) input.fields = metaPatch.fields;
            if (metaPatch.seo) input.seo = metaPatch.seo;
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
          setFieldErrors({});
          setSaveAttempted(false);
          const still = !isDirtyNow();
          setDirty(!still);
          setSaveState(still ? "saved" : "dirty");
          // The server deleted the slot: nothing to write, except edits made meanwhile
          // (now relative to the new version). A kept copy goes back at once (still
          // held, so nothing can come between). The working draft's local backup is
          // saved now too (one still on offer is another draft: it stays).
          autosaver.current?.markClean();
          slotHolds.current = false;
          if (backupKey && backupRef.current === null) clearBackup(backupKey);
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
          if (!still) touch(true);
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
              // The screen sets it when the publish dialog was asked for meanwhile.
              openPublishDialog: false,
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
          const errors = serverFieldErrors(e);
          if (Object.keys(errors).length > 0) {
            setFieldErrors(errors);
            setSaveAttempted(true);
            latest.current.onInvalid?.("server");
            // The schema may have changed since the screen read it.
            if (Object.keys(errors).some((p) => p.startsWith("fields"))) {
              void queryClient.invalidateQueries({ queryKey: typesQuery.queryKey });
            }
          }
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
    [editor, type, adopt, fail, isDirtyNow, touch, queryClient, backupKey, setKeptStored],
  );

  /** A save waits for a transition in flight, then saves what is still unsaved. */
  const save = useCallback((): Promise<ContentDto | null> => {
    const running = transitioning.current;
    return running ? running.then(() => saveNow()) : saveNow();
  }, [saveNow]);

  const transition = useCallback(
    (action: Transition, options: TransitionOptions = {}): Promise<ContentDto | null> => {
      if (readOnlyRef.current || restoringRef.current) return Promise.resolve(null);
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
        slotHolds.current = true;
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
    [itemId, type, setKept, setKeptStored],
  );

  const discard = useCallback(async (): Promise<void> => {
    const a = autosaver.current;
    // The edits go: so does their local backup (unless it is still on offer, a choice of
    // its own with its own Discard: then it stays for the next visit).
    if (backupKey && backupRef.current === null) clearBackup(backupKey);
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
      slotHolds.current = false;
      a?.markClean();
      a?.dispose();
    } finally {
      release();
    }
  }, [itemId, type, backupKey]);

  const storeKept = useCallback(async (): Promise<boolean> => {
    const copy = keptRef.current;
    if (copy === null || keptStoredRef.current || itemId === null) return true;
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
  }, [itemId, type, setKeptStored]);

  const discardKept = useCallback(async (): Promise<void> => {
    if (itemId === null) return;
    const a = autosaver.current;
    const release = (await a?.hold()) ?? (() => undefined);
    try {
      await deleteAutosave(type, itemId);
      slotHolds.current = false;
      setKept(null);
    } finally {
      release();
    }
    // The slot is free: the working draft goes there from now on (not to the backup).
    if (backupKey) clearBackup(backupKey);
    if (isDirtyNow()) touch(true);
  }, [itemId, type, setKept, isDirtyNow, touch, backupKey]);

  /** The editing lock of a restore: the body and title are read-only while it is on. */
  const lockEditing = useCallback(
    (on: boolean) => {
      setRevisionBusy(on);
      if (editor.isDestroyed) return;
      if (on) editor.setEditable(false, false);
      else if (readOnlyRef.current === null) editor.setEditable(true, false);
    },
    [editor],
  );
  const lock = useCallback(
    (on: boolean) => {
      restoringRef.current = on;
      lockEditing(on);
    },
    [lockEditing],
  );
  // A restore waiting for a write in flight (editing already locked, writes not yet).
  const restoreWaiting = useRef(false);

  const restoreRevisionNow = useCallback(
    async (rid: string, options: RestoreRevisionOptions = {}) => {
      if (restoringRef.current || restoreWaiting.current) return null;
      // Editing stops now, not after the wait below: text typed while a save lands would
      // be replaced by the restore without a word. Writes stay open (`restoringRef` is
      // not set yet), so a transition in flight still saves and runs to its end.
      restoreWaiting.current = true;
      lockEditing(true);
      try {
        // A save or transition in flight lands first (its version is the one to match).
        await transitioning.current;
        await inflight.current;
      } finally {
        restoreWaiting.current = false;
      }
      const current = itemRef.current;
      if (!current || !current.allowed.edit) {
        lockEditing(false);
        // Nothing to restore over (the account may no longer edit it): said, not silent.
        options.onError?.(null);
        return null;
      }
      lock(true);
      const a = autosaver.current;
      // A running autosave lands first and none starts while the restore runs.
      const release = (await a?.hold()) ?? (() => undefined);
      try {
        const next = await restoreRevision(
          type,
          current.id,
          rid,
          options.version ?? current.version,
        );
        // The server deleted this user's slot with the restore: the old working draft
        // must not go back there (not by the timer, not when the screen goes).
        slotHolds.current = false;
        a?.markClean();
        a?.dispose();
        // The working draft (and so its local backup) is replaced by the revision.
        if (backupKey && backupRef.current === null) clearBackup(backupKey);
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
        // Stays locked: the screen reopens on `next`.
        return next;
      } catch (e) {
        lock(false);
        fail(e, "revision", options.onError, { then: "revision" });
        return null;
      } finally {
        release();
      }
    },
    [type, adopt, fail, lock, lockEditing, backupKey, setKeptStored],
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
    setField,
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
    backup,
    dropBackup,
    backupNow,
    save,
    transition,
    adopt,
    restoreRevision: restoreRevisionNow,
    restoring: revisionBusy,
    fieldDefs: opts.fieldDefs ?? null,
    fieldErrors,
    clearFieldError,
    saveAttempted,
  };
}
