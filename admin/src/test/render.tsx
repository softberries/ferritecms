// Component tests with a stubbed API (M2b P17): renders `ui` inside a QueryClient and
// a one-route router (so `Link` works), and answers `fetch` from a table of handlers.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { render } from "@testing-library/react";
import type { ReactNode } from "react";
import { vi } from "vitest";

export function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

export type Handler = (url: URL, req: Request) => Response | Promise<Response>;

/**
 * Stubs `fetch`: each request goes to the handler of its path (`/api/v1/…`, without the
 * query); an unknown path fails the test. Returns every requested URL.
 */
export function stubRoutes(routes: Record<string, Handler>): URL[] {
  const seen: URL[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (req: Request) => {
      const url = new URL(req.url);
      seen.push(url);
      const handler = routes[url.pathname];
      if (!handler) throw new Error(`unexpected ${req.method} ${url.pathname}${url.search}`);
      return handler(url, req);
    }),
  );
  return seen;
}

export function renderWithRouter(ui: ReactNode): QueryClient {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree: createRootRoute({ component: () => ui }),
    basepath: "/admin",
    history: createMemoryHistory({ initialEntries: ["/admin/"] }),
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return queryClient;
}
