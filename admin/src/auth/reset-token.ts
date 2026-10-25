// The password-reset token travels in the link's fragment (`#token=…`, 07 §2), which no
// browser sends to a server or puts in a `Referer`. It is read once, before anything
// renders or sends a request, the fragment is dropped from the address bar, and the
// token lives only in memory until the reset page takes it (Review Focus 1).

/** The page the mailed link opens. */
export const RESET_PATH = "/admin/reset-password";

/**
 * Reads `token` from `loc.hash` and replaces the history entry with the same path and
 * query and no fragment, whether or not a token was there. `null` when there is none.
 */
export function takeResetToken(
  loc: Pick<Location, "hash" | "pathname" | "search">,
  history: Pick<History, "replaceState">,
): string | null {
  const params = new URLSearchParams(loc.hash.startsWith("#") ? loc.hash.slice(1) : loc.hash);
  const token = params.get("token");
  history.replaceState(null, "", loc.pathname + loc.search);
  return token === null || token.trim() === "" ? null : token;
}

let held: string | null = null;

/** Keeps the token taken in `main.tsx` for the reset page. */
export function holdResetToken(token: string | null): void {
  held = token;
}

/** The held token, once: every later call (a reload is a fresh module) returns `null`. */
export function consumeResetToken(): string | null {
  const token = held;
  held = null;
  return token;
}

/** `true` on the reset page (with or without a trailing slash). */
export function isResetPath(pathname: string): boolean {
  return pathname === RESET_PATH || pathname === `${RESET_PATH}/`;
}
