import type { QueryClient } from "@tanstack/react-query";
import { type RouterHistory, createRouter } from "@tanstack/react-router";
import type { TFunction } from "i18next";
import { routeTree } from "./routeTree.gen";
import { RouteError } from "./shell/error-boundary";
import { NotFound } from "./shell/not-found";

export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
  return createRouter({
    routeTree,
    basepath: "/admin",
    defaultPreload: "intent",
    // Inside the `_app` shell when the failing or missing route is under it (fuzzy
    // not-found mode, the default): the not-found screen is a component, not a
    // `/not-found` route (pre-flight F20).
    defaultNotFoundComponent: NotFound,
    defaultErrorComponent: RouteError,
    context: { queryClient },
    ...(history ? { history } : {}),
  });
}

export type AppRouter = ReturnType<typeof createAppRouter>;

declare module "@tanstack/react-router" {
  interface Register {
    router: AppRouter;
  }

  interface StaticDataRouteOption {
    /**
     * This route's breadcrumb in the top bar (`shell` namespace) and the document title;
     * `loaderData` is the route's loader result once it has loaded (a content type's label).
     */
    crumb?: (t: TFunction<"shell">, loaderData?: unknown) => string;
    /** Rendered without the shell (the styleguide brings its own frame). */
    bare?: boolean;
  }

  /** What the sign-in screens pass through history state (never through the URL). */
  interface HistoryState {
    /** `/login/totp`: the sealed challenge of the password step (valid 5 minutes). */
    totpChallenge?: string;
    /** When the challenge was issued (ms since the epoch, the browser's clock). */
    totpIssuedAt?: number;
    /** A notice for the login screen. */
    authNotice?: "totp-expired" | "signed-out";
  }
}
