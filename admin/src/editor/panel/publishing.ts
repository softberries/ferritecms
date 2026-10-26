// The side panel's decisions that need no React: which actions the status section shows
// (only what the item's `allowed` names, Review Focus 4), the schedule time as the user
// picks it (a wall-clock date and time in a time zone) to and from RFC 3339 UTC, the
// permalink a slug edit will give, the parents an item may not take, and the term tree.

import type { SchemaAllowedActionsDto } from "@/api/schema";
import { uiLocale } from "@/lib/locale";

// --- Status actions --------------------------------------------------------------

/**
 * What the status section can offer. `publish` and `schedule` open the publish dialog
 * (in "now" or "schedule" mode); `publishNow` opens it for a scheduled item (now only);
 * `save` is Save draft / Save changes / Update by status; `unpublish` of a scheduled item
 * reads "Unschedule"; `preview` opens `/_preview/<id>`.
 */
export type PanelAction =
  "publish" | "schedule" | "publishNow" | "submit" | "save" | "unpublish" | "preview";

export interface PanelActions {
  /** The first is the filled button; the rest are outlined. */
  primary: PanelAction[];
  secondary: PanelAction[];
}

export interface ActionItem {
  status: string;
  allowed: SchemaAllowedActionsDto;
}

/**
 * The status section's buttons for `item` (`null`: a new item, not stored yet, so only
 * saving it as a draft). Read-only screens offer only Preview. The SPA never decides
 * on its own: a button appears only when `allowed` has its flag.
 */
export function statusActions(item: ActionItem | null, readOnly: boolean): PanelActions {
  if (item === null)
    return readOnly ? { primary: [], secondary: [] } : { primary: ["save"], secondary: [] };
  const preview: PanelAction[] = item.status === "trash" ? [] : ["preview"];
  if (readOnly) return { primary: [], secondary: preview };
  const a = item.allowed;
  const primary: PanelAction[] = [];
  const secondary: PanelAction[] = [];
  switch (item.status) {
    case "draft":
    case "pending":
      if (a.publish) primary.push("publish");
      if (a.schedule) primary.push("schedule");
      // Submit for review is for those who cannot publish (contributors).
      if (a.submit && !a.publish) primary.push("submit");
      if (a.edit) primary.push("save");
      break;
    case "scheduled":
      if (a.edit) primary.push("save");
      if (a.publish) primary.push("publishNow");
      if (a.unpublish) secondary.push("unpublish");
      break;
    case "published":
    case "private":
      if (a.edit) primary.push("save");
      if (a.unpublish) secondary.push("unpublish");
      break;
    default:
      break;
  }
  secondary.push(...preview);
  return { primary, secondary };
}

// --- Schedule time ---------------------------------------------------------------

/** A wall-clock time as the picker holds it: `YYYY-MM-DD` and `HH:MM` (24 h). */
export interface WallClock {
  date: string;
  time: string;
}

/** The browser's IANA time zone (`Europe/Warsaw`); `UTC` when it cannot be read. */
export function browserTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

const partsFormat = new Map<string, Intl.DateTimeFormat>();

function formatIn(timeZone: string): Intl.DateTimeFormat {
  let f = partsFormat.get(timeZone);
  if (!f) {
    f = new Intl.DateTimeFormat("en-US", {
      timeZone,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
    partsFormat.set(timeZone, f);
  }
  return f;
}

function partsAt(ms: number, timeZone: string) {
  const out: Record<string, number> = {};
  for (const p of formatIn(timeZone).formatToParts(new Date(ms))) {
    if (p.type !== "literal") out[p.type] = Number(p.value);
  }
  return {
    year: out.year ?? 1970,
    month: out.month ?? 1,
    day: out.day ?? 1,
    hour: (out.hour ?? 0) % 24,
    minute: out.minute ?? 0,
    second: out.second ?? 0,
  };
}

/** How far `timeZone`'s wall clock is ahead of UTC at the instant `ms`, in ms. */
function offsetAt(ms: number, timeZone: string): number {
  const p = partsAt(ms, timeZone);
  const asUtc = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second);
  return asUtc - Math.floor(ms / 1000) * 1000;
}

const DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/;
const TIME_RE = /^(\d{2}):(\d{2})$/;

/**
 * The instant `wall` names in `timeZone`, as RFC 3339 UTC (`2026-11-03T08:30:00Z`):
 * the time the user chose stays the time on their clock, whatever the offset that day.
 * `null` when the date or time is not well formed. A time a daylight-saving change
 * skips moves forward by the gap (02:30 → 03:30); a time that happens twice takes the
 * later (standard-time) instant.
 */
export function wallClockToUtc(wall: WallClock, timeZone: string): string | null {
  const d = DATE_RE.exec(wall.date);
  const t = TIME_RE.exec(wall.time);
  if (!d || !t) return null;
  const [y, mo, da, h, mi] = [d[1], d[2], d[3], t[1], t[2]].map(Number) as [
    number,
    number,
    number,
    number,
    number,
  ];
  if (mo < 1 || mo > 12 || da < 1 || da > 31 || h > 23 || mi > 59) return null;
  const guess = Date.UTC(y, mo - 1, da, h, mi);
  let ms = guess - offsetAt(guess, timeZone);
  const again = offsetAt(ms, timeZone);
  if (again !== guess - ms) ms = guess - again;
  return new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z");
}

const pad = (n: number) => String(n).padStart(2, "0");

/** The wall clock in `timeZone` at the instant `iso` (the reverse of `wallClockToUtc`). */
export function utcToWallClock(iso: string, timeZone: string): WallClock | null {
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return null;
  const p = partsAt(ms, timeZone);
  return {
    date: `${p.year}-${pad(p.month)}-${pad(p.day)}`,
    time: `${pad(p.hour)}:${pad(p.minute)}`,
  };
}

/** `GMT+01:00` for `timeZone` at the instant `ms` (`GMT` for UTC itself). */
export function zoneOffsetLabel(timeZone: string, ms: number): string {
  const minutes = Math.round(offsetAt(ms, timeZone) / 60_000);
  if (minutes === 0) return "GMT";
  const sign = minutes > 0 ? "+" : "−";
  const abs = Math.abs(minutes);
  return `GMT${sign}${pad(Math.floor(abs / 60))}:${pad(abs % 60)}`;
}

/** "3 Nov 2026, 09:30 CET": an instant on the browser's clock, naming its zone. */
export function formatWhen(iso: string): string {
  // `dateStyle`/`timeStyle` cannot be combined with `timeZoneName`.
  return new Intl.DateTimeFormat(uiLocale(), {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    timeZoneName: "short",
  }).format(new Date(iso));
}

/** Whether `iso` is not in the future (the API refuses such a schedule as well). */
export function isPast(iso: string, now: number = Date.now()): boolean {
  return Date.parse(iso) <= now;
}

/** The picker's starting value: tomorrow at 09:00 on the user's clock. */
export function defaultScheduleWall(timeZone: string, now: number = Date.now()): WallClock {
  const today = utcToWallClock(new Date(now).toISOString(), timeZone);
  const [y, m, d] = (today?.date ?? "1970-01-01").split("-").map(Number) as [
    number,
    number,
    number,
  ];
  const next = new Date(Date.UTC(y, m - 1, d + 1));
  return {
    date: `${next.getUTCFullYear()}-${pad(next.getUTCMonth() + 1)}-${pad(next.getUTCDate())}`,
    time: "09:00",
  };
}

/** `YYYY-MM-DD` of a calendar `Date` (its local fields, as react-day-picker hands it over). */
export function dateKey(day: Date): string {
  return `${day.getFullYear()}-${pad(day.getMonth() + 1)}-${pad(day.getDate())}`;
}

/** A calendar `Date` (local midnight) for `YYYY-MM-DD`. */
export function dateFromKey(key: string): Date | undefined {
  const m = DATE_RE.exec(key);
  if (!m) return undefined;
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

// --- Permalink -------------------------------------------------------------------

/**
 * The permalink a slug edit will give: the stored `permalink` with its last segment
 * equal to the stored slug replaced by the new one (trimmed and lower-cased, as the
 * server stores it). `null` when the stored permalink does not contain the slug (a
 * structure without `%slug%`): the address is known only after the save.
 */
export function predictPermalink(
  permalink: string,
  savedSlug: string,
  draftSlug: string,
): string | null {
  const next = draftSlug.trim().toLowerCase();
  if (next === "" || next === savedSlug) return permalink;
  const segments = permalink.split("/");
  for (let i = segments.length - 1; i >= 0; i--) {
    if (segments[i] === savedSlug) {
      segments[i] = next;
      return segments.join("/");
    }
  }
  return null;
}

// --- Parent ----------------------------------------------------------------------

/**
 * The item and its descendants among `items` (as far as the loaded list shows; the
 * server refuses a cycle anyway): none of them can be the item's parent.
 */
export function notAParent(
  selfId: string | null,
  items: readonly { id: string; parent_id?: string | null }[],
): Set<string> {
  const out = new Set<string>();
  if (selfId === null) return out;
  out.add(selfId);
  const parentOf = new Map(items.map((i) => [i.id, i.parent_id ?? null]));
  for (const item of items) {
    const seen = new Set<string>();
    let at: string | null = item.parent_id ?? null;
    while (at !== null && !seen.has(at)) {
      if (at === selfId) {
        out.add(item.id);
        break;
      }
      seen.add(at);
      at = parentOf.get(at) ?? null;
    }
  }
  return out;
}

// --- Terms -----------------------------------------------------------------------

export interface TermLike {
  id: string;
  name: string;
  parent_id?: string | null;
}

export interface TermNode<T extends TermLike = TermLike> {
  term: T;
  depth: number;
}

/**
 * A hierarchical taxonomy's terms in tree order (each parent followed by its children,
 * siblings by name), with their depth. A term whose parent is not in the list sits at
 * the top level; a cycle is cut where it closes.
 */
export function termTree<T extends TermLike>(terms: readonly T[], locale?: string): TermNode<T>[] {
  const ids = new Set(terms.map((t) => t.id));
  const children = new Map<string | null, T[]>();
  for (const term of terms) {
    const parent = term.parent_id && ids.has(term.parent_id) ? term.parent_id : null;
    const list = children.get(parent) ?? [];
    list.push(term);
    children.set(parent, list);
  }
  const collator = new Intl.Collator(locale, { sensitivity: "base" });
  for (const list of children.values()) list.sort((a, b) => collator.compare(a.name, b.name));
  const out: TermNode<T>[] = [];
  const placed = new Set<string>();
  const walk = (parent: string | null, depth: number) => {
    for (const term of children.get(parent) ?? []) {
      if (placed.has(term.id)) continue;
      placed.add(term.id);
      out.push({ term, depth });
      walk(term.id, depth + 1);
    }
  };
  walk(null, 0);
  // Terms only reachable through a cycle: shown at the top level rather than lost.
  for (const term of terms) {
    if (!placed.has(term.id)) {
      placed.add(term.id);
      out.push({ term, depth: 0 });
      walk(term.id, 1);
    }
  }
  return out;
}

/** `selected` with `id` added at the end, or removed when it is there. */
export function toggleTerm(selected: readonly string[], id: string): string[] {
  return selected.includes(id) ? selected.filter((t) => t !== id) : [...selected, id];
}
