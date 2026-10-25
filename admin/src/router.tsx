import type { QueryClient } from "@tanstack/react-query";
import { type RouterHistory, createRouter } from "@tanstack/react-router";
import { routeTree } from "./routeTree.gen";

export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
  return createRouter({
    routeTree,
    basepath: "/admin",
    defaultPreload: "intent",
    context: { queryClient },
    ...(history ? { history } : {}),
  });
}

export type AppRouter = ReturnType<typeof createAppRouter>;

declare module "@tanstack/react-router" {
  interface Register {
    router: AppRouter;
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
