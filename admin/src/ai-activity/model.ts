// The AI activity screens' pure helpers (M6-5, design 17 §11): set status, op verbs,
// a generic JSON diff by dotted path, and conflict texts.

import type { TFunction } from "i18next";
import type {
  ChangeDto,
  ChangeSetDto,
  ChangeSetStatus,
  RevertConflictDto,
} from "@/api/change-sets";

export const STATUSES: readonly ChangeSetStatus[] = [
  "open",
  "closed",
  "partly_reverted",
  "reverted",
] as const;

export function statusOf(set: ChangeSetDto): ChangeSetStatus {
  return (STATUSES as readonly string[]).includes(set.status)
    ? (set.status as ChangeSetStatus)
    : "closed";
}

/** The kinds whose diff is the revisions drawer's title/excerpt/body diff. */
export function isContentKind(kind: string): boolean {
  return kind === "content" || kind === "media";
}

/** Changes still revertable in a set: not reverted and with both images kept. */
export function revertableCount(changes: readonly ChangeDto[]): number {
  return changes.filter((c) => c.reverted_at == null && c.revertable).length;
}

/** One leaf that differs between two JSON values. */
export interface JsonChange {
  path: string;
  before?: unknown;
  after?: unknown;
}

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/**
 * The leaves that differ between `before` and `after`, by dotted path (`items.0.label`),
 * in path order. Objects and arrays are walked; a member on one side only is an added
 * or removed leaf. `undefined` means absent.
 */
export function jsonDiff(before: unknown, after: unknown, path = ""): JsonChange[] {
  if (same(before, after)) return [];
  const walk = (a: unknown, b: unknown) =>
    (isObject(a) && isObject(b)) || (Array.isArray(a) && Array.isArray(b));
  if (!walk(before, after)) {
    return [
      {
        path: path || "(value)",
        ...(before === undefined ? {} : { before }),
        ...(after === undefined ? {} : { after }),
      },
    ];
  }
  const a = before as Record<string, unknown>;
  const b = after as Record<string, unknown>;
  const keys = [...new Set([...Object.keys(a), ...Object.keys(b)])];
  if (!Array.isArray(before)) keys.sort();
  const out: JsonChange[] = [];
  for (const key of keys) {
    const sub = path ? `${path}.${key}` : key;
    const inA = Object.hasOwn(a, key);
    const inB = Object.hasOwn(b, key);
    if (inA && inB) out.push(...jsonDiff(a[key], b[key], sub));
    else if (inA) out.push({ path: sub, before: a[key] });
    else out.push({ path: sub, after: b[key] });
  }
  return out;
}

/** A JSON leaf as one short line of text. */
export function valueText(v: unknown, max = 160): string {
  const text = typeof v === "string" ? v : JSON.stringify(v);
  return text.length > max ? `${text.slice(0, max)}…` : text;
}

/** The members of a content image the field table shows (besides title/excerpt/body). */
export const CONTENT_FIELDS = [
  "status",
  "slug",
  "parent_id",
  "menu_order",
  "terms",
  "seo",
  "fields",
  "published_at",
  "scheduled_at",
  "alt",
  "locale",
] as const;

/** The members of `CONTENT_FIELDS` that differ, as a JSON diff. */
export function contentFieldChanges(before: unknown, after: unknown): JsonChange[] {
  const pick = (v: unknown) => {
    if (!isObject(v)) return undefined;
    const out: Record<string, unknown> = {};
    for (const k of CONTENT_FIELDS) if (Object.hasOwn(v, k)) out[k] = v[k];
    return out;
  };
  return jsonDiff(pick(before), pick(after));
}

/** The text of one revert conflict ("Edited by Ana 10 min ago"). */
export function conflictText(
  t: TFunction<"ai">,
  c: RevertConflictDto,
  names: (id: string) => string | undefined,
  ago: (at: string) => string,
): string {
  switch (c.reason) {
    case "changed_since": {
      const who = c.by ? names(c.by) : undefined;
      if (who && c.at) return t("activity.conflict.changedBy", { who, when: ago(c.at) });
      return t("activity.conflict.changed");
    }
    case "later_change":
      return t("activity.conflict.later", { seq: c.later ?? 0 });
    case "gone":
      return t("activity.conflict.gone");
    case "too_large":
      return t("activity.conflict.tooLarge");
    default:
      return c.reason;
  }
}

/** Whether `force` resolves every conflict (only `changed_since` ones are forceable). */
export function forceable(conflicts: readonly RevertConflictDto[]): boolean {
  return conflicts.length > 0 && conflicts.every((c) => c.reason === "changed_since");
}
