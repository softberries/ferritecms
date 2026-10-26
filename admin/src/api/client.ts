import { MutationCache, QueryCache, QueryClient } from "@tanstack/react-query";
import createClient, { type Middleware } from "openapi-fetch";
import { i18n } from "@/i18n";
import { getCsrfToken, setCsrfToken } from "./csrf";
import { ApiError } from "./problem";
import type { paths } from "./schema";

const SAFE_METHODS = new Set(["GET", "HEAD"]);

/**
 * `fetch` itself failed: the request got no response (offline, DNS, connection
 * reset). Only these and `503` are retried; any other error in a `queryFn`
 * (a `TypeError` from a bug included) surfaces at once.
 */
export class NetworkError extends Error {
  constructor(cause: unknown) {
    super("network request failed", { cause });
    this.name = "NetworkError";
  }
}

/**
 * Sets `Accept` on every request and `X-CSRF-Token` on unsafe ones, and turns a
 * problem response (`application/problem+json`) into a thrown `ApiError`.
 */
export const requestMiddleware: Middleware = {
  onRequest({ request }) {
    request.headers.set("Accept", "application/json");
    const token = getCsrfToken();
    if (token !== null && !SAFE_METHODS.has(request.method.toUpperCase())) {
      request.headers.set("X-CSRF-Token", token);
    }
    return request;
  },
  async onResponse({ response }) {
    const type = response.headers.get("Content-Type") ?? "";
    if (type.toLowerCase().startsWith("application/problem+json")) {
      throw await ApiError.fromProblemResponse(response);
    }
    return undefined;
  },
  onError({ error }) {
    // An abort is the caller's own choice (TanStack Query cancels this way): not wrapped.
    if (error instanceof DOMException && error.name === "AbortError") return undefined;
    return new NetworkError(error);
  },
};

/**
 * The typed API client. The generated paths already start with `/api/v1`, so the
 * base is the page's own origin (absolute, because `Request` outside a browser
 * cannot resolve a relative URL). `fetch` is looked up per call, so tests can stub it.
 */
export const api = createClient<paths>({
  baseUrl: globalThis.location.origin,
  credentials: "same-origin",
  // openapi-fetch passes a second argument (`requestInitExt`, undici-only options
  // from `createClient`); none is configured here, so it is not forwarded.
  fetch: (request: Request) => globalThis.fetch(request),
});
api.use(requestMiddleware);

/**
 * The result of an openapi-fetch call as data: a failed response without a problem
 * document (the middleware already threw for those with one) becomes an `ApiError`.
 */
export async function unwrap<T>(
  call: Promise<{ data?: T; error?: unknown; response: Response }>,
): Promise<T> {
  const { data, response } = await call;
  if (!response.ok) throw ApiError.fromResponse(response);
  return data as T;
}

// --- Global error handling (07 §3) -------------------------------------------

/** What the global error handler needs from the app; the router in `main.tsx`. */
export interface ErrorRouting {
  /**
   * Navigates within the admin to `href` (relative to `/admin`, e.g. `/login?next=…`)
   * through the router, ignoring navigation blockers (F37: never `location.assign`).
   */
  navigate: (href: string) => unknown;
  /** The current URL's path and query, e.g. `/admin/content/post?status=draft`. */
  currentPath: () => string;
  /** Shows a transient warning (a toast); `id` replaces an earlier one of the same kind. */
  notify: (id: string, message: string) => void;
}

/** Pages a `401` does not leave: signing in is what they are for. */
const SIGN_IN_PAGES = ["/admin/login", "/admin/forgot-password", "/admin/reset-password"];
const CHANGE_PASSWORD_PAGE = "/admin/change-password";

function isOn(path: string, page: string): boolean {
  const pathname = path.split(/[?#]/, 1)[0] ?? "";
  return pathname === page || pathname.startsWith(`${page}/`);
}

/** The screens outside the shell (sign-in, reset, forced change): no toasts there. */
function onAuthScreen(path: string): boolean {
  return [...SIGN_IN_PAGES, CHANGE_PASSWORD_PAGE].some((page) => isOn(path, page));
}

/** Network failures (the request never got a response) and `503 busy` are retried. */
function isTransient(error: unknown): boolean {
  if (error instanceof ApiError) return error.status === 503;
  return error instanceof NetworkError;
}

/** Default `retry`: never for an HTTP error but 503, twice for network errors and 503. */
export function shouldRetry(failureCount: number, error: unknown): boolean {
  return failureCount < 2 && isTransient(error);
}

/** Default `retryDelay`: the server's `Retry-After` when it sent one, else exponential. */
export function retryDelay(failureCount: number, error: unknown): number {
  if (error instanceof ApiError && error.retryAfter !== null) {
    return Math.min(error.retryAfter * 1000, 30_000);
  }
  return Math.min(1000 * 2 ** failureCount, 30_000);
}

/** The `reset` of every error handler, for `rearmRedirects`. */
const rearmers = new Set<() => void>();

/**
 * Re-arms the 401/forced-change redirect of every error handler. `auth.ts` calls
 * it whenever a session starts or is confirmed (login, TOTP login, `me`, password
 * change), so a login made outside the query cache re-arms it too.
 */
export function rearmRedirects(): void {
  for (const rearm of rearmers) rearm();
}

/**
 * Builds the app's error handler: `unauthenticated` → login with `next`,
 * `password-reset-required` → change password, `rate-limited`/`busy` → a toast.
 * Parallel failures navigate once: after one redirect the next are ignored until
 * any query or mutation succeeds again or a session starts (`rearmRedirects`).
 */
export function createErrorHandler(routing: ErrorRouting) {
  let redirecting = false;
  rearmers.add(() => {
    redirecting = false;
  });

  const redirect = (href: string) => {
    if (redirecting) return;
    redirecting = true;
    routing.navigate(href);
  };

  return {
    /** `inline`: the screen shows the error itself, so no toast (redirects still apply). */
    handle(error: unknown, { inline = false }: { inline?: boolean } = {}): void {
      if (!(error instanceof ApiError)) return;
      const path = routing.currentPath();
      switch (error.slug) {
        case "unauthenticated":
          if (SIGN_IN_PAGES.some((page) => isOn(path, page))) return;
          setCsrfToken(null);
          // The cache stays: the same user signing back in keeps it. A different user
          // gets it cleared when their session starts (`startSession`).
          redirect(`/login?${new URLSearchParams({ next: path }).toString()}`);
          return;
        case "password-reset-required":
          if (isOn(path, CHANGE_PASSWORD_PAGE)) return;
          redirect("/change-password");
          return;
        case "rate-limited":
          // Sign-in screens show their own countdown; a toast there would say it twice
          // (and sonner replays it once the shell's <Toaster> mounts).
          if (inline || onAuthScreen(path)) return;
          routing.notify(
            error.slug,
            error.retryAfter === null
              ? i18n.t("common:errors.rateLimitedLater")
              : i18n.t("common:errors.rateLimited", { seconds: error.retryAfter }),
          );
          return;
        case "busy":
          if (inline || onAuthScreen(path)) return;
          routing.notify(
            error.slug,
            error.retryAfter === null
              ? i18n.t("common:errors.busyLater")
              : i18n.t("common:errors.busy", { seconds: error.retryAfter }),
          );
          return;
        default:
          return;
      }
    },
    reset(): void {
      redirecting = false;
    },
  };
}

/**
 * A query's `meta` for a screen that shows its own error message (the palette's search):
 * the global handler still redirects on `401`/forced change but does not toast.
 */
export const INLINE_ERRORS = { inlineErrors: true } as const;

/** The app's handler for writes made outside the query cache (set by `createQueryClient`). */
let outsideHandler: ((error: unknown) => void) | null = null;

/**
 * Hands a failure of a write made outside TanStack Query (the editor's save and
 * autosave) to the global handler: a `401` goes to login with `next`, a forced change to
 * the change-password screen. No toast: the caller shows its own message. The handler
 * only navigates; it never clears the editor's `pendingDrafts` (the editor puts its
 * draft there before calling this, F37).
 */
export function reportApiError(error: unknown): void {
  outsideHandler?.(error);
}

/** The app's `QueryClient` with the global error handling and retry policy. */
export function createQueryClient(routing: ErrorRouting): QueryClient {
  const errors = createErrorHandler(routing);
  outsideHandler = (error) => {
    errors.handle(error, { inline: true });
  };
  const onError = (error: unknown) => {
    errors.handle(error);
  };
  const onSuccess = () => {
    errors.reset();
  };
  return new QueryClient({
    queryCache: new QueryCache({
      onError: (error, query) => {
        errors.handle(error, { inline: query.meta?.inlineErrors === true });
      },
      onSuccess,
    }),
    mutationCache: new MutationCache({ onError, onSuccess }),
    defaultOptions: {
      queries: { retry: shouldRetry, retryDelay },
      // Writes are not idempotent: a mutation is never retried automatically.
      mutations: { retry: false },
    },
  });
}

/** The path and query of the current page, without the fragment (it may hold a secret). */
export function currentPath(): string {
  return `${globalThis.location.pathname}${globalThis.location.search}`;
}
