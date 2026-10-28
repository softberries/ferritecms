// Users for the users screen's unit tests.

import type { UserDto } from "@/api/users";
import type { MeDto } from "@/auth/session";

export const ME: MeDto = {
  id: "11111111-1111-4111-8111-111111111111",
  login: "ada",
  email: "ada@example.test",
  display_name: "Ada Lovelace",
  role: "admin",
  must_reset: false,
  totp_enabled: false,
  mail_configured: false,
  capabilities: ["read_admin", "manage_users"],
  scopes: ["users:write"],
  csrf_token: "csrf",
};

export function user(patch: Partial<UserDto> = {}): UserDto {
  return {
    id: "22222222-2222-4222-8222-222222222222",
    login: "grace",
    email: "grace@example.test",
    display_name: "Grace Hopper",
    role: "editor",
    status: "active",
    must_reset: false,
    totp_enabled: false,
    created_at: "2026-01-02T03:04:05Z",
    last_login_at: "2026-10-27T09:00:00Z",
    ...patch,
  };
}

/** A `422 validation` problem; `field` names the field its one error is about. */
export function invalid(detail: string, field?: string): Response {
  return new Response(
    JSON.stringify({
      type: "https://ferritecms.dev/problems/validation",
      title: "Validation failed",
      status: 422,
      detail,
      ...(field ? { errors: [{ field, detail }] } : {}),
    }),
    { status: 422, headers: { "Content-Type": "application/problem+json" } },
  );
}

export function noContent(): Response {
  return new Response(null, { status: 204 });
}
