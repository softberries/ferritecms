import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaCreatedTokenDto, SchemaTokenDto, SchemaTokenInput } from "./schema";

export type TokenDto = SchemaTokenDto;
/** Carries the bearer `token`: hold it in component state only, never in the cache. */
export type CreatedTokenDto = SchemaCreatedTokenDto;

/** The scope areas (`Area::ALL`; no endpoint lists them), each `read` or `write`. */
export const SCOPE_AREAS = [
  "content",
  "media",
  "terms",
  "menus",
  "comments",
  "users",
  "settings",
  "redirects",
  "extensions",
  "import",
  "system",
] as const;
export type ScopeArea = (typeof SCOPE_AREAS)[number];

/** Tokens a user may hold (the server's `422` past it). */
export const MAX_TOKENS = 50;

/** `GET /users/{id}/tokens`: every token of the user, oldest first (one page). */
export async function listTokens(userId: string): Promise<TokenDto[]> {
  const list = await unwrap(
    api.GET("/api/v1/users/{id}/tokens", { params: { path: { id: userId } } }),
  );
  return list.items;
}

export function tokensQuery(userId: string) {
  return queryOptions({
    queryKey: qk.tokens(userId),
    queryFn: () => listTokens(userId),
    staleTime: 30_000,
  });
}

/**
 * `POST /users/{id}/tokens`: the answer's `token` is shown once. Not a mutation whose
 * result is cached: the caller keeps it in component state until its dialog closes.
 */
export function createToken(userId: string, body: SchemaTokenInput): Promise<CreatedTokenDto> {
  return unwrap(api.POST("/api/v1/users/{id}/tokens", { params: { path: { id: userId } }, body }));
}

export async function revokeToken(userId: string, tokenId: string): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/users/{id}/tokens/{tid}", {
      params: { path: { id: userId, tid: tokenId } },
    }),
  );
}
