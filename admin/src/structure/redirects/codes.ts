// The response codes a redirect can have: the ones core accepts
// (`ferrite_domain::REDIRECT_CODES`; `ferrite-core/src/redirects.rs` `check_row`).

export const REDIRECT_CODES = [301, 302, 307, 308, 410] as const;
export type RedirectCode = (typeof REDIRECT_CODES)[number];

export const isRedirectCode = (code: number): code is RedirectCode =>
  (REDIRECT_CODES as readonly number[]).includes(code);
