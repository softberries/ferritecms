// The router's search-string codec: TanStack Router's default (each value JSON-decoded,
// so `mine=true` is a boolean) except for free-text keys, which stay the exact text
// typed. The default would read `?q=2024` as the number 2024, `?q=531038e6` as
// 531038000000 and `?q=1e3` as 1000, and write a numeric-looking string back quoted
// (`?q=%222024%22`); every list search (content, media, redirects) would then lose what
// was typed on a reload or a shared link. One codec for the whole app, so no list
// schema has to undo it.

import { defaultParseSearch, defaultStringifySearch } from "@tanstack/react-router";

/** Search keys holding free text: never JSON-decoded, written as plain text. */
export const FREE_TEXT_KEYS: readonly string[] = ["q"];

/** `parseSearch` of the router. */
export function parseSearch(searchStr: string): Record<string, unknown> {
  const out: Record<string, unknown> = defaultParseSearch(searchStr);
  const raw = new URLSearchParams(searchStr.startsWith("?") ? searchStr.slice(1) : searchStr);
  for (const key of FREE_TEXT_KEYS) {
    const value = raw.get(key);
    if (value !== null) out[key] = value;
  }
  return out;
}

/** `stringifySearch` of the router. */
export function stringifySearch(search: object): string {
  const rest: Record<string, unknown> = {};
  const free: [string, string][] = [];
  for (const [key, value] of Object.entries(search)) {
    if (FREE_TEXT_KEYS.includes(key) && typeof value === "string") free.push([key, value]);
    else rest[key] = value;
  }
  const encoded = defaultStringifySearch(rest);
  if (free.length === 0) return encoded;
  const text = new URLSearchParams(free).toString();
  return encoded ? `${encoded}&${text}` : `?${text}`;
}
