import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaSessionDto, SchemaSessionsRevoked } from "./schema";

export type SessionDto = SchemaSessionDto;

/** `GET /auth/sessions`: the signed-in user's live sessions, newest first. */
export async function listSessions(): Promise<SessionDto[]> {
  const list = await unwrap(api.GET("/api/v1/auth/sessions"));
  return list.items;
}

export const sessionsQuery = queryOptions({
  queryKey: qk.sessions,
  queryFn: listSessions,
  staleTime: 30_000,
});

/** `DELETE /auth/sessions/{id}`: ends one of one's own other sessions. */
export async function revokeSession(id: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/auth/sessions/{id}", { params: { path: { id } } }));
}

/** `POST /auth/sessions/revoke-others`: ends every session but this one. */
export function revokeOtherSessions(): Promise<SchemaSessionsRevoked> {
  return unwrap(api.POST("/api/v1/auth/sessions/revoke-others"));
}
