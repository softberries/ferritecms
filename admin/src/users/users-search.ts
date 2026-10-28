// The users screen's URL state (`/admin/users?q=…&role=…&status=…&id=…&new=true`): `q`
// the search (trimmed, at most `MAX_USER_QUERY` characters, the API's limit), `role` and
// `status` the filter chips, `id` the user open in the sheet, `new` the New user dialog
// (the palette's "New user"). Invalid members are dropped, defaults omitted.

import { MAX_USER_QUERY, type RoleDto, type UserStatus } from "@/api/users";

export const ROLES = ["admin", "editor", "author", "contributor"] as const satisfies RoleDto[];
export const STATUSES = ["active", "disabled"] as const satisfies UserStatus[];

export interface UsersSearch {
  q?: string;
  role?: RoleDto;
  status?: UserStatus;
  /** The user open in the sheet. */
  id?: string;
  /** The New user dialog is open. */
  new?: true;
}

export const isRole = (v: unknown): v is RoleDto =>
  typeof v === "string" && (ROLES as readonly string[]).includes(v);
const isStatus = (v: unknown): v is UserStatus =>
  typeof v === "string" && (STATUSES as readonly string[]).includes(v);

/** `validateSearch` of the route. */
export function parseUsersSearch(raw: Record<string, unknown>): UsersSearch {
  const out: UsersSearch = {};
  if (typeof raw.q === "string") {
    const q = Array.from(raw.q.trim()).slice(0, MAX_USER_QUERY).join("");
    if (q) out.q = q;
  }
  if (isRole(raw.role)) out.role = raw.role;
  if (isStatus(raw.status)) out.status = raw.status;
  if (typeof raw.id === "string" && raw.id) out.id = raw.id;
  if (raw.new === true || raw.new === "true") out.new = true;
  return out;
}
