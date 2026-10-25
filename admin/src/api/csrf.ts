// The session's `X-CSRF-Token`, held in memory only (never in storage, a cookie
// or a URL). It comes from login, `GET /auth/me` and a password change (07 §2).

let token: string | null = null;

/** Replaces the token; `null` once the session is gone (logout, `401`). */
export function setCsrfToken(t: string | null): void {
  token = t;
}

export function getCsrfToken(): string | null {
  return token;
}
