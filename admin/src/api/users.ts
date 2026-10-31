import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { type UserFilters, qk } from "./keys";
import type {
  SchemaRoleDto,
  SchemaUserDto,
  SchemaUserInput,
  SchemaUserList,
  SchemaUserPatchInput,
  SchemaUserStatusDto,
} from "./schema";

export type UserDto = SchemaUserDto;
export type RoleDto = SchemaRoleDto;
export type UserStatus = SchemaUserStatusDto;

/** Users a page (constraints: exact values). */
export const USERS_PAGE = 50;
/** Longest `q` the API takes, in characters (after trimming). */
export const MAX_USER_QUERY = 100;

/**
 * `GET /users` (`manage_users`): one page by login. `q` is trimmed and a blank one left
 * out (the API takes a blank `q` as no search too).
 */
export function listUsers(
  query: UserFilters & { cursor?: string; limit?: number } = {},
  options: { signal?: AbortSignal } = {},
): Promise<SchemaUserList> {
  const { q, ...rest } = query;
  const trimmed = q?.trim() ?? "";
  return unwrap(
    api.GET("/api/v1/users", {
      params: { query: trimmed === "" ? rest : { ...rest, q: trimmed } },
      ...(options.signal ? { signal: options.signal } : {}),
    }),
  );
}

/** The first page of a filtered list (the screen pages on with `useInfiniteQuery`). */
export function usersQuery(filters: UserFilters) {
  return queryOptions({
    queryKey: qk.users(filters),
    queryFn: ({ signal }) => listUsers({ ...filters, limit: USERS_PAGE }, { signal }),
    staleTime: 30_000,
  });
}

/** `GET /users/{id}`: oneself, or anyone with `manage_users`. */
export function getUser(id: string): Promise<UserDto> {
  return unwrap(api.GET("/api/v1/users/{id}", { params: { path: { id } } }));
}

export function userQuery(id: string) {
  return queryOptions({
    queryKey: qk.user(id),
    queryFn: () => getUser(id),
    staleTime: 30_000,
  });
}

/** `POST /users`; without `password` the account cannot sign in until one is set. */
export function createUser(body: SchemaUserInput): Promise<UserDto> {
  return unwrap(api.POST("/api/v1/users", { body }));
}

/**
 * `PATCH /users/{id}`: last write wins (no `If-Match`). A user changing their own e-mail
 * sends `current_password`; role and status never change on oneself.
 */
export function patchUser(id: string, body: SchemaUserPatchInput): Promise<UserDto> {
  return unwrap(api.PATCH("/api/v1/users/{id}", { params: { path: { id } }, body }));
}

/**
 * `DELETE /users/{id}`; `reassign` names the active user who takes over the deleted
 * user's content (a `422` with field `reassign` says it is needed).
 */
export async function deleteUser(id: string, reassign?: string): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/users/{id}", {
      params: { path: { id }, ...(reassign ? { query: { reassign } } : {}) },
    }),
  );
}

/**
 * `POST /users/{id}/password` (another user, `manage_users`): sets `must_reset` and ends
 * their sessions. Oneself is `400`: use `changePassword`.
 */
export async function setUserPassword(id: string, password: string): Promise<void> {
  await unwrap(
    api.POST("/api/v1/users/{id}/password", { params: { path: { id } }, body: { password } }),
  );
}

/** `POST /users/{id}/totp/disable` (another user, `manage_users`; P43). */
export async function disableUserTotp(id: string): Promise<void> {
  await unwrap(api.POST("/api/v1/users/{id}/totp/disable", { params: { path: { id } } }));
}
