// Where a secret could linger in the running admin (07 §2 "shown once"): the address,
// `history.state`, both web storages, and the TanStack Query caches. The app exposes no
// test hook in its production build, so the caches are reached through React's own
// fiber tree: the `QueryClientProvider`'s `client` prop.

import type { Page } from "@playwright/test";

/** Everything the page keeps outside its DOM, as one string to search. */
export async function keptState(page: Page): Promise<string> {
  return page.evaluate(() => {
    interface Fiber {
      child: Fiber | null;
      sibling: Fiber | null;
      memoizedProps: unknown;
    }
    interface Cache<T> {
      getAll(): T[];
    }
    interface Client {
      getQueryCache(): Cache<{ queryKey: unknown; state: { data: unknown } }>;
      getMutationCache(): Cache<{ state: { variables: unknown; data: unknown } }>;
    }
    const root = document.getElementById("root") as unknown as Record<string, unknown> | null;
    const key = root ? Object.keys(root).find((k) => k.startsWith("__reactContainer$")) : null;
    let client: Client | null = null;
    const stack: Fiber[] = [];
    const start = root && key ? (root[key] as Fiber) : null;
    if (start) stack.push(start);
    while (stack.length > 0 && !client) {
      const fiber = stack.pop() as Fiber;
      const props = fiber.memoizedProps as { client?: Partial<Client> } | null;
      if (props?.client && typeof props.client.getQueryCache === "function") {
        client = props.client as Client;
        break;
      }
      if (fiber.sibling) stack.push(fiber.sibling);
      if (fiber.child) stack.push(fiber.child);
    }
    if (!client) throw new Error("no QueryClientProvider in the page");
    const storage = (s: Storage) =>
      Array.from({ length: s.length }, (_, i) => {
        const k = s.key(i) ?? "";
        return [k, s.getItem(k)];
      });
    return JSON.stringify({
      href: location.href,
      history: history.state as unknown,
      local: storage(localStorage),
      session: storage(sessionStorage),
      queries: client
        .getQueryCache()
        .getAll()
        .map((q) => [q.queryKey, q.state.data]),
      mutations: client
        .getMutationCache()
        .getAll()
        .map((m) => [m.state.variables, m.state.data]),
    });
  });
}
