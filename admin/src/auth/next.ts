// Where to go after signing in (Review Focus 5): `?next=` is accepted only for a
// same-origin path under `/admin/`, so it can never become an open redirect.

/** Where a refused or missing `next` goes: the dashboard. */
export const DEFAULT_NEXT = "/admin/";

const ADMIN = "/admin/";
const LOGIN = "/admin/login";

/** Percent-decodes until the text stops changing (catches `%252e%252e`); `null` if malformed. */
function fullyDecoded(raw: string): string | null {
  let text = raw;
  for (let i = 0; i < 5; i++) {
    let next: string;
    try {
      next = decodeURIComponent(text);
    } catch {
      return null;
    }
    if (next === text) return text;
    text = next;
  }
  return null;
}

function hasControl(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c < 0x20 || c === 0x7f) return true;
  }
  return false;
}

/**
 * `raw` when it is a same-origin path under `/admin/` (after URL parsing against
 * `location.origin`): no backslash, no `//` prefix, no `..` segment after decoding, and
 * not the login page itself. Anything else, `/admin/`.
 */
export function safeNext(raw: string | null | undefined): string {
  if (typeof raw !== "string" || raw === "") return DEFAULT_NEXT;
  // A path, not a scheme (`javascript:`, `https:`) or a protocol-relative URL.
  if (!raw.startsWith("/") || raw.startsWith("//")) return DEFAULT_NEXT;
  const decoded = fullyDecoded(raw);
  if (decoded === null) return DEFAULT_NEXT;
  // Browsers read `\` as `/` in special URLs (`/\evil` is `//evil`); control characters
  // are stripped by the URL parser, so refuse both before parsing.
  if (decoded.includes("\\") || decoded.startsWith("//") || hasControl(decoded)) {
    return DEFAULT_NEXT;
  }
  const path = decoded.split(/[?#]/, 1)[0] ?? "";
  // No empty segment anywhere (`/admin//evil.com` would become the router href
  // `//evil.com`), and no dot segments.
  if (path.includes("//")) return DEFAULT_NEXT;
  if (path.split("/").some((segment) => segment === ".." || segment === ".")) return DEFAULT_NEXT;

  const origin = globalThis.location.origin;
  let url: URL;
  try {
    url = new URL(raw, origin);
  } catch {
    return DEFAULT_NEXT;
  }
  if (url.origin !== origin) return DEFAULT_NEXT;
  if (!url.pathname.startsWith(ADMIN)) return DEFAULT_NEXT;
  // The router matches paths case-insensitively, so the login page in any case.
  const lower = url.pathname.toLowerCase();
  if (lower === LOGIN || lower.startsWith(`${LOGIN}/`)) return DEFAULT_NEXT;
  return raw;
}

/**
 * `next` (from `safeNext`) as a router `href`; the router's paths are relative to the
 * `/admin` basepath. Defence in depth: anything that would not be a single-slash path
 * (a protocol-relative `//host` or a backslash) becomes `/`.
 */
export function routerHref(next: string): string {
  const href = next.startsWith(ADMIN) ? next.slice(ADMIN.length - 1) : "/";
  if (!href.startsWith("/") || href.startsWith("//") || href.includes("\\")) return "/";
  return href;
}

/** The `?next=` search of the sign-in routes. */
export interface NextSearch {
  next?: string;
}

/** `?next=` as given (validated with `safeNext` only where it is used). */
export function nextSearch(search: Record<string, unknown>): NextSearch {
  return typeof search.next === "string" && search.next !== "" ? { next: search.next } : {};
}
