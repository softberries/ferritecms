import type { QueryClient } from "@tanstack/react-query";

/** What every route's `beforeLoad` and `loader` receive. */
export interface RouterContext {
  queryClient: QueryClient;
}
