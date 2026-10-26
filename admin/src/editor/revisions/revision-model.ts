// What the revisions drawer (Task 17) works out from the API's revisions, free of React:
// the `restored from …` note, the word counts of a diff, the day groups of the list and
// the revision an Undo of a restore goes back to.

import type { SchemaRevisionSummaryDto } from "@/api/schema";
import { uiLocale } from "@/lib/locale";
import { i18n } from "@/i18n";
import { type DiffPart, diffRevision } from "../diff";
import { isObject, sameValue, valuesOf } from "../fields/fields-model";
import { blockText } from "../text";
import { wordCount } from "../word-count";

export type RevisionSummary = SchemaRevisionSummaryDto;
export type RevisionDiffs = ReturnType<typeof diffRevision>;

const RESTORED = /^restored from ([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/i;

/** The revision id a restore's note names (`restored from <rid>`), else `null`. */
export function restoredFrom(note: string | null | undefined): string | null {
  return note ? (RESTORED.exec(note.trim())?.[1]?.toLowerCase() ?? null) : null;
}

/** Words added and removed across the parts (whitespace-only changes count none). */
export function diffStats(parts: readonly DiffPart[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const p of parts) {
    if (p.kind === "added") added += wordCount(p.text);
    else if (p.kind === "removed") removed += wordCount(p.text);
  }
  return { added, removed };
}

export const changed = (parts: readonly DiffPart[]) => parts.some((p) => p.kind !== "same");

/** Whether anything of title, excerpt or body differs. */
export function anyChange(d: RevisionDiffs): boolean {
  return changed(d.title) || changed(d.excerpt) || changed(d.body);
}

/** The word counts of title, excerpt and body together. */
export function totalStats(d: RevisionDiffs): { added: number; removed: number } {
  const all = [d.title, d.excerpt, d.body].map(diffStats);
  return {
    added: all.reduce((n, s) => n + s.added, 0),
    removed: all.reduce((n, s) => n + s.removed, 0),
  };
}

/**
 * The revision an Undo of restoring `rid` restores: the list (newest first, read after
 * the restore) starts with the restore's own snapshot, and the one after it is what was
 * current before (progress ledger, T17). `null` when the list does not start with that
 * snapshot (someone saved in between): there is then nothing safe to undo to.
 */
export function undoTarget(items: readonly RevisionSummary[], rid: string): string | null {
  const [snapshot, before] = items;
  if (!snapshot || restoredFrom(snapshot.note) !== rid.toLowerCase()) return null;
  return before?.id ?? null;
}

export type DayKey = "today" | "yesterday" | "earlier";

export interface DayGroup {
  /** `YYYY-MM-DD` on the local clock: the group's React key. */
  key: string;
  day: DayKey;
  /** Midnight of that day (local), for the heading of an earlier day. */
  date: Date;
  items: RevisionSummary[];
}

const dayKey = (d: Date) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

/** The list (newest first) in runs of one local day each, in the same order. */
export function groupByDay(items: readonly RevisionSummary[], now: Date = new Date()): DayGroup[] {
  const today = dayKey(now);
  const y = new Date(now);
  y.setDate(y.getDate() - 1);
  const yesterday = dayKey(y);
  const groups: DayGroup[] = [];
  for (const item of items) {
    const at = new Date(item.created_at);
    const key = dayKey(at);
    let group = groups.at(-1);
    if (group?.key !== key) {
      group = {
        key,
        day: key === today ? "today" : key === yesterday ? "yesterday" : "earlier",
        date: new Date(at.getFullYear(), at.getMonth(), at.getDate()),
        items: [],
      };
      groups.push(group);
    }
    group.items.push(item);
  }
  return groups;
}

/** One or two capitals of a display name ("Ada Lovelace" → "AL"). */
export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  const first = parts[0]?.[0] ?? "";
  const last = parts.length > 1 ? (parts[parts.length - 1]?.[0] ?? "") : "";
  return (first + last).toUpperCase() || "?";
}

export const fmt = (options: Intl.DateTimeFormatOptions) =>
  new Intl.DateTimeFormat(uiLocale(), options);

/** "14:02" (the list's rows). */
export const timeOf = (iso: string) =>
  fmt({ hour: "numeric", minute: "2-digit" }).format(new Date(iso));

/** "26 Oct 2026, 14:02" (the selected revision, the `restored from` note, the toast). */
export const whenOf = (iso: string) =>
  fmt({
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(iso));

/** One custom field that differs; a missing side is a field the version does not have. */
export interface FieldChange {
  name: string;
  before?: unknown;
  after?: unknown;
}

/**
 * The top-level custom fields that differ between `before` and `after`, by name. `null`
 * and absent are different values (a cleared field against one never set).
 */
export function fieldChanges(before: unknown, after: unknown): FieldChange[] {
  const a = valuesOf(before);
  const b = valuesOf(after);
  const names = [...new Set([...Object.keys(a), ...Object.keys(b)])].sort();
  const out: FieldChange[] = [];
  for (const name of names) {
    const inA = Object.hasOwn(a, name);
    const inB = Object.hasOwn(b, name);
    if (inA && inB && sameValue(a[name], b[name])) continue;
    out.push({ name, ...(inA ? { before: a[name] } : {}), ...(inB ? { after: b[name] } : {}) });
  }
  return out;
}

/** Longest value text a change line shows. */
const VALUE_CHARS = 160;

/** A field value as one line of text: absent, empty, yes/no, a rich text's words, JSON. */
export function fieldText(v: unknown): string {
  const t = i18n.getFixedT(null, "editor");
  let text: string;
  if (v === undefined) return t("revisions.fieldValue.absent");
  if (v === null) return t("revisions.fieldValue.empty");
  if (typeof v === "boolean")
    return v ? t("revisions.fieldValue.yes") : t("revisions.fieldValue.no");
  if (typeof v === "string") text = v;
  else if (typeof v === "number") text = String(v);
  else if (isObject(v) && v.type === "doc") text = blockText(v).replace(/\s+/g, " ").trim();
  else if (Array.isArray(v)) {
    text = t("revisions.fieldValue.rows", { count: v.length, json: JSON.stringify(v) });
  } else text = JSON.stringify(v);
  return text.length > VALUE_CHARS ? `${text.slice(0, VALUE_CHARS)}…` : text;
}
