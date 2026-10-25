import { api, unwrap } from "./client";
import { setCsrfToken } from "./csrf";
import type {
  SchemaLoginRequest,
  SchemaLoginResponse,
  SchemaMeDto,
  SchemaPasswordChange,
  SchemaPasswordChanged,
  SchemaPasswordResetConfirm,
  SchemaPasswordResetRequest,
  SchemaTotpLoginRequest,
} from "./schema";

function keepToken(r: SchemaLoginResponse): SchemaLoginResponse {
  if (r.status === "session") setCsrfToken(r.csrf_token);
  return r;
}

/** Password login; a `session` answer stores the CSRF token, `totp_required` carries the challenge. */
export async function login(body: SchemaLoginRequest): Promise<SchemaLoginResponse> {
  return keepToken(await unwrap(api.POST("/api/v1/auth/login", { body })));
}

/** The second step of a TOTP login. */
export async function loginTotp(body: SchemaTotpLoginRequest): Promise<SchemaLoginResponse> {
  return keepToken(await unwrap(api.POST("/api/v1/auth/login/totp", { body })));
}

/** Ends the session; the CSRF token is dropped even when the request fails. */
export async function logout(): Promise<void> {
  try {
    await unwrap(api.POST("/api/v1/auth/logout"));
  } finally {
    setCsrfToken(null);
  }
}

/** The signed-in user; stores the session's CSRF token (`null` for a bearer token). */
export async function me(): Promise<SchemaMeDto> {
  const user = await unwrap(api.GET("/api/v1/auth/me"));
  setCsrfToken(user.csrf_token ?? null);
  return user;
}

/** Changes the password; the server rotates the session, so the new CSRF token is stored. */
export async function changePassword(body: SchemaPasswordChange): Promise<SchemaPasswordChanged> {
  const changed = await unwrap(api.POST("/api/v1/auth/password", { body }));
  setCsrfToken(changed.csrf_token ?? null);
  return changed;
}

/** Asks for a reset e-mail (`202` whatever the account). */
export function requestReset(body: SchemaPasswordResetRequest): Promise<void> {
  return unwrap(api.POST("/api/v1/auth/password-reset", { body }));
}

/** Sets a new password with the e-mailed token (`204`; ends every session of the account). */
export function confirmReset(body: SchemaPasswordResetConfirm): Promise<void> {
  return unwrap(api.POST("/api/v1/auth/password-reset/confirm", { body }));
}
