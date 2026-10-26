// A `412 precondition-failed` on a save (07 §1): someone saved the item after this screen
// loaded it. The problem carries the server's copy (`current`, a `ContentDto`), so the
// conflict dialog can put the two versions side by side and the user decides; the
// user's text is never dropped (Review Focus 3).

import type { ApiError } from "@/api/problem";
import type { SchemaContentDto } from "@/api/schema";
import type { Draft } from "./autosave";
import { sameValue } from "./fields/fields-model";

export type ConflictField =
  "title" | "excerpt" | "body" | "slug" | "terms" | "parent" | "menu_order" | "fields" | "seo";

export interface ConflictState {
  /** The server's copy (`problem.current`). */
  theirs: SchemaContentDto;
  /** The working draft at the time of the conflict. */
  mine: Draft;
  /** The members that differ, in display order. */
  fields: ConflictField[];
}

/** `problem.current` as the item, when the problem carries one. */
export function currentOf(problem: ApiError): SchemaContentDto | null {
  const current: unknown = (problem.problem as { current?: unknown }).current;
  if (typeof current !== "object" || current === null) return null;
  const c = current as Partial<SchemaContentDto>;
  return typeof c.id === "string" && typeof c.version === "number" && typeof c.title === "string"
    ? (current as SchemaContentDto)
    : null;
}

/**
 * A body as comparable text: keys sorted, `null` attributes and empty `attrs` dropped
 * (TipTap fills in null defaults the stored copy may not have, pre-flight F32), so the
 * same document compares equal whichever editor wrote it.
 */
export function canonicalBody(body: unknown): string {
  const norm = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(norm);
    if (typeof v !== "object" || v === null) return v;
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(v).sort()) {
      const value = (v as Record<string, unknown>)[key];
      if (value === null || value === undefined) continue;
      const n = norm(value);
      if (key === "attrs" && typeof n === "object" && n !== null && Object.keys(n).length === 0) {
        continue;
      }
      out[key] = n;
    }
    return out;
  };
  return JSON.stringify(norm(body));
}

const sameList = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((x, i) => x === b[i]);

/** An SEO member as stored: trimmed, blank is `null` (what the server makes of it). */
export const seoText = (v: string | null | undefined): string | null => {
  const s = (v ?? "").trim();
  return s === "" ? null : s;
};

/** Whether the draft's SEO differs from the item's (a draft without SEO leaves it). */
function seoDiffers(theirs: SchemaContentDto, mine: Draft): boolean {
  if (!mine.seo) return false;
  const stored = theirs.seo;
  return (
    (mine.seo.title !== undefined && seoText(mine.seo.title) !== seoText(stored.title)) ||
    (mine.seo.description !== undefined &&
      seoText(mine.seo.description) !== seoText(stored.description))
  );
}

/**
 * The members where `mine` and `theirs` differ. A member the draft leaves unchanged
 * (`null` slug, terms or menu order) or derived (`null` excerpt) never counts.
 */
export function differingFields(theirs: SchemaContentDto, mine: Draft): ConflictField[] {
  const out: ConflictField[] = [];
  if (mine.title !== theirs.title) out.push("title");
  if (mine.excerpt !== null && mine.excerpt !== theirs.excerpt) out.push("excerpt");
  if (canonicalBody(mine.body) !== canonicalBody(theirs.body)) out.push("body");
  if (mine.slug !== null && mine.slug !== theirs.slug) out.push("slug");
  if (mine.terms !== null && !sameList(mine.terms, theirs.terms)) out.push("terms");
  if (mine.parent_id !== (theirs.parent_id ?? null)) out.push("parent");
  if (mine.menu_order !== null && mine.menu_order !== theirs.menu_order) out.push("menu_order");
  // `null` and absent differ (a cleared field is a value).
  if (!sameValue(mine.fields ?? {}, theirs.fields ?? {})) out.push("fields");
  if (seoDiffers(theirs, mine)) out.push("seo");
  return out;
}

/**
 * The conflict a `412` describes: the server's copy, the draft, and what differs.
 * `null` when the problem carries no usable `current` (the caller then says so plainly).
 */
export function onPreconditionFailed(problem: ApiError, mine: Draft): ConflictState | null {
  const theirs = currentOf(problem);
  if (!theirs) return null;
  return { theirs, mine, fields: differingFields(theirs, mine) };
}
