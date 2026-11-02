// Connecting AI assistants (12 §3.2, §8): the consent screen's pending request, its
// approval or refusal (a signed-in session only; each answers where the browser goes
// next), and a user's connected apps (OAuth grants) with Disconnect.

import { queryOptions } from "@tanstack/react-query";
import { INLINE_ERRORS, api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaConsentDto, SchemaGrantDto, SchemaOAuthClientDto } from "./schema";

export type ConsentDto = SchemaConsentDto;
export type GrantDto = SchemaGrantDto;
export type OAuthClientDto = SchemaOAuthClientDto;

/** `GET /oauth/requests/{id}`: `404` for an unknown, expired or used request. */
export function getConsent(id: string): Promise<ConsentDto> {
  return unwrap(api.GET("/api/v1/oauth/requests/{id}", { params: { path: { id } } }));
}

/**
 * The request as the consent screen reads it: never retried (a `404` is an answer) and
 * never refetched behind the user's back (the request is used up by a decision). Its
 * failures show on the screen, not as a toast.
 */
export function consentQuery(id: string) {
  return queryOptions({
    queryKey: qk.oauthRequest(id),
    queryFn: () => getConsent(id),
    retry: false,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    meta: INLINE_ERRORS,
  });
}

/** `POST …/approve`: the chosen scopes (each within `grantable`); answers `redirect_to`. */
export async function approveConsent(id: string, scopes: string[]): Promise<string> {
  const r = await unwrap(
    api.POST("/api/v1/oauth/requests/{id}/approve", {
      params: { path: { id } },
      body: { scopes },
    }),
  );
  return r.redirect_to;
}

/** `POST …/deny`: answers `redirect_to` (the client's URI with `error=access_denied`). */
export async function denyConsent(id: string): Promise<string> {
  const r = await unwrap(
    api.POST("/api/v1/oauth/requests/{id}/deny", { params: { path: { id } } }),
  );
  return r.redirect_to;
}

/**
 * `GET /oauth/grants`: the connected apps of the signed-in user, or (`other`, with
 * `manage_users`) of another user.
 */
export async function listGrants(other?: string): Promise<GrantDto[]> {
  const list = await unwrap(
    api.GET("/api/v1/oauth/grants", other ? { params: { query: { user: other } } } : {}),
  );
  return list.items;
}

/** `userId`'s connected apps; `own` when it is the signed-in user (no `?user=`). */
export function grantsQuery(userId: string, own: boolean) {
  return queryOptions({
    queryKey: qk.grants(userId),
    queryFn: () => listGrants(own ? undefined : userId),
    staleTime: 30_000,
  });
}

/** `DELETE /oauth/grants/{id}`: the grant and its access tokens are gone. */
export async function disconnectGrant(id: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/oauth/grants/{id}", { params: { path: { id } } }));
}

/** The MCP endpoint of a site: `site.url` without its trailing slash, then `/mcp`. */
export function mcpUrl(siteUrl: string): string {
  return `${siteUrl.replace(/\/+$/, "")}/mcp`;
}
