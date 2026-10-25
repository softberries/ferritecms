// The session layer: who is signed in (`GET /auth/me`), the login guard of the `_app`
// layout, and logout. The CSRF token is held by `@/api/csrf` (memory only); `me()`,
// `login()` and `changePassword()` in `@/api/auth` store it.

import { type QueryClient, queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { type ParsedLocation, redirect, useNavigate } from "@tanstack/react-router";
import { useCallback } from "react";
import { logout as apiLogout, me } from "@/api/auth";
import { getCsrfToken, setCsrfToken } from "@/api/csrf";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import type { SchemaMeDto } from "@/api/schema";
import type { RouterContext } from "@/lib/router-context";
import { setRecentViewer } from "@/palette/recent";

export type MeDto = SchemaMeDto;

/** How long a confirmed session is trusted before the guard asks the server again. */
const ME_FRESH_MS = 60_000;

/**
 * The signed-in user as a query. It never retries: a `401` is an answer, not a
 * transient failure. `queryFn` stores the CSRF token (through `me()`).
 */
export const meQuery = queryOptions({
  queryKey: qk.me,
  queryFn: me,
  staleTime: ME_FRESH_MS,
  retry: false,
});

/** The signed-in user, once known (the `_app` guard has always loaded it). */
export function useMe(): MeDto | undefined {
  return useQuery(meQuery).data;
}

/** The admin path and query of a router location (never the fragment). */
export function adminPath(location: Pick<ParsedLocation, "pathname" | "searchStr">): string {
  const path = location.pathname;
  const prefixed = path === "/admin" || path.startsWith("/admin/") ? path : `/admin${path}`;
  return `${prefixed}${location.searchStr}`;
}

export interface GuardContext {
  context: RouterContext;
  location: Pick<ParsedLocation, "pathname" | "searchStr">;
}

/**
 * The signed-in user, or a redirect: no session → `/login?next=<this page>`, a session
 * that must change its password → `/change-password`. It resolves only once the CSRF
 * token is held, so a write made after it carries one.
 */
export async function ensureSession(ctx: GuardContext): Promise<MeDto> {
  const user = await loadSession(ctx);
  if (user.must_reset) throw redirect({ to: "/change-password", replace: true });
  return user;
}

/**
 * The `_app` guard: `ensureSession`, then the palette's Recent viewer, set here
 * (synchronously, before any route under `_app` renders) so a child that pushes a
 * Recent entry on mount always writes to the signed-in user's list.
 */
export async function enterShell(ctx: GuardContext): Promise<MeDto> {
  const user = await ensureSession(ctx);
  setRecentViewer(user.id);
  return user;
}

/**
 * The signed-in user (a `must_reset` one included), or a redirect to `/login` with
 * `next`. It calls `me()` directly rather than through the query cache, so the global
 * error handler does not also redirect on the `401`; the result is put into the cache.
 */
export async function loadSession({ context, location }: GuardContext): Promise<MeDto> {
  const { queryClient } = context;
  const cached = freshUser(queryClient);
  if (cached) return cached;
  let user: MeDto;
  try {
    user = await me();
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      setCsrfToken(null);
      setRecentViewer(null);
      queryClient.removeQueries({ queryKey: qk.me });
      throw redirect({ to: "/login", search: { next: adminPath(location) }, replace: true });
    }
    throw e;
  }
  queryClient.setQueryData(qk.me, user);
  return user;
}

/** The cached user while it is fresh and its CSRF token is held, else `undefined`. */
function freshUser(queryClient: QueryClient): MeDto | undefined {
  const state = queryClient.getQueryState<MeDto>(qk.me);
  const user = state?.data;
  if (!user || state.isInvalidated) return undefined;
  if (Date.now() - state.dataUpdatedAt > ME_FRESH_MS) return undefined;
  if (user.csrf_token != null && getCsrfToken() === null) return undefined;
  return user;
}

/** What `logout` needs from the app (the router and its query client). */
export interface LogoutDeps {
  queryClient: QueryClient;
  navigate: (opts: {
    to: "/login";
    replace: boolean;
    state: { authNotice: "signed-out" };
  }) => unknown;
}

/**
 * Ends the session: posts the logout (a failure still signs out locally), clears the
 * query cache, the CSRF token and the Recent viewer, and navigates to `/login`.
 */
export async function logout({ queryClient, navigate }: LogoutDeps): Promise<void> {
  try {
    await apiLogout();
  } catch {
    // The server session may already be gone (or unreachable); the local one ends anyway.
  }
  setCsrfToken(null);
  // Nobody is signed in: Recent reads nothing and writes nothing until the next guard.
  setRecentViewer(null);
  queryClient.clear();
  await navigate({ to: "/login", replace: true, state: { authNotice: "signed-out" } });
}

/** `logout` bound to the app's query client and router. */
export function useLogout(): () => Promise<void> {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  return useCallback(() => logout({ queryClient, navigate }), [queryClient, navigate]);
}
