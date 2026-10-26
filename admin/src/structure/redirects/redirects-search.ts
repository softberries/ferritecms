// The redirects screen's URL state (`/admin/structure/redirects?q=…&new=true`): `q` the
// search (trimmed, at most `MAX_REDIRECT_QUERY` characters, the API's limit; free text,
// so the router keeps it a string, `lib/search-params.ts`), `new` the New redirect
// dialog. Invalid members are dropped, defaults omitted.

import { MAX_REDIRECT_QUERY } from "@/api/redirects";
import type { RedirectsSearch } from "./redirects-table";

/** `validateSearch` of the route. */
export function parseRedirectsSearch(raw: Record<string, unknown>): RedirectsSearch {
  const out: RedirectsSearch = {};
  if (typeof raw.q === "string") {
    const q = Array.from(raw.q.trim()).slice(0, MAX_REDIRECT_QUERY).join("");
    if (q) out.q = q;
  }
  if (raw.new === true || raw.new === "true") out.new = true;
  return out;
}
