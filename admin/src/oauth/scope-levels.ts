// The consent screen's permissions as one level per scope area (12 §3.3): `none`,
// `read` or `write` (`<area>:write` implies `<area>:read`, as core reads scopes). What
// the user may choose is capped by the request's `grantable`, never above it.

import { SCOPE_AREAS, type ScopeArea } from "@/api/tokens";

export type Level = "none" | "read" | "write";
export type Levels = Partial<Record<ScopeArea, Level>>;

const RANK: Record<Level, number> = { none: 0, read: 1, write: 2 };

/** The strongest level each area holds in `scopes` (unknown names are ignored). */
export function levelsOf(scopes: readonly string[]): Levels {
  const out: Levels = {};
  for (const scope of scopes) {
    const [area, access] = scope.split(":");
    if (!(SCOPE_AREAS as readonly string[]).includes(area ?? "")) continue;
    if (access !== "read" && access !== "write") continue;
    const key = area as ScopeArea;
    if (RANK[access] > RANK[out[key] ?? "none"]) out[key] = access;
  }
  return out;
}

/** The areas in `levels` above `none`, in `SCOPE_AREAS` order. */
export function areasOf(levels: Levels): ScopeArea[] {
  return SCOPE_AREAS.filter((area) => (levels[area] ?? "none") !== "none");
}

/** One scope per area that is not `none`, in `SCOPE_AREAS` order. */
export function scopesOf(levels: Levels): string[] {
  return areasOf(levels).map((area) => `${area}:${levels[area] ?? "none"}`);
}

/** Every area of `max` at no more than `read` (the "Read only" shortcut). */
export function readOnly(max: Levels): Levels {
  const out: Levels = {};
  for (const area of areasOf(max)) out[area] = "read";
  return out;
}

/** Whether any area of `levels` is `write`. */
export function anyWrite(levels: Levels): boolean {
  return SCOPE_AREAS.some((area) => levels[area] === "write");
}

/** The areas `requested` asks for that the user cannot grant at all (their role). */
export function beyondRole(requested: Levels, grantable: Levels): ScopeArea[] {
  return areasOf(requested).filter((area) => (grantable[area] ?? "none") === "none");
}
