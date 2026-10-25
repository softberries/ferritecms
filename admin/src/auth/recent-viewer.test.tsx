// The palette's Recent list stays per viewer across sign-out and sign-in in one tab:
// the `_app` guard (`enterShell`) sets the viewer before any child renders, and
// `logout()` clears it (Task 11 review I1).

import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRouteWithContext,
  createRoute,
  createRouter,
  useLocation,
} from "@tanstack/react-router";
import { QueryClient } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { setCsrfToken } from "@/api/csrf";
import type { RouterContext } from "@/lib/router-context";
import { listRecent, pushRecent, recentStorageKey, setRecentViewer } from "@/palette/recent";
import { enterShell, logout } from "./session";

const user = (id: string, login: string) => ({
  id,
  login,
  email: `${login}@example.test`,
  display_name: login,
  role: "editor",
  must_reset: false,
  totp_enabled: false,
  capabilities: ["read_admin"],
  scopes: [],
  csrf_token: `csrf-${login}`,
});
const ALICE = user("00000000-0000-0000-0000-00000000000a", "alice");
const BOB = user("00000000-0000-0000-0000-00000000000b", "bob");

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** `/auth/me` answers with whoever is "signed in" now; logout answers 204. */
function stubServer() {
  const state = { current: ALICE };
  vi.stubGlobal(
    "fetch",
    vi.fn((req: Request) => {
      const path = new URL(req.url).pathname;
      if (path === "/api/v1/auth/me") return Promise.resolve(json(state.current));
      if (path === "/api/v1/auth/logout")
        return Promise.resolve(new Response(null, { status: 204 }));
      return Promise.reject(new Error(`unexpected ${req.method} ${path}`));
    }),
  );
  return state;
}

/** A child that records an item in Recent as it mounts (what the editor will do). */
function OpenItem() {
  // The test router is not the registered app router, so its params are untyped here.
  const id = useLocation({ select: (l) => l.pathname.split("/").at(-1) });
  useEffect(() => {
    if (id) pushRecent({ kind: "item", type: "post", id, title: `Post ${id}`, status: "draft" });
  }, [id]);
  return <p>item {id}</p>;
}

function setup() {
  const queryClient = new QueryClient();
  const root = createRootRouteWithContext<RouterContext>()({ component: Outlet });
  const login = createRoute({
    getParentRoute: () => root,
    path: "/login",
    component: () => <p>login</p>,
  });
  const app = createRoute({
    getParentRoute: () => root,
    id: "_app",
    beforeLoad: async (ctx) => ({ me: await enterShell(ctx) }),
    component: Outlet,
  });
  const item = createRoute({
    getParentRoute: () => app,
    path: "/item/$id",
    component: OpenItem,
  });
  const router = createRouter({
    routeTree: root.addChildren([login, app.addChildren([item])]),
    context: { queryClient },
    history: createMemoryHistory({ initialEntries: ["/item/1"] }),
  });
  render(<RouterProvider router={router} />);
  return { router, queryClient };
}

const stored = (id: string) => localStorage.getItem(recentStorageKey(id));

beforeEach(() => {
  localStorage.clear();
  setCsrfToken(null);
  setRecentViewer(null);
});

afterEach(() => {
  setRecentViewer(null);
  vi.unstubAllGlobals();
});

test("recent_follows_the_signed_in_viewer_across_logout", async () => {
  const server = stubServer();
  const { router, queryClient } = setup();

  // A fresh tab: the child's mount-time push already lands in Alice's list.
  expect(await screen.findByText("item 1")).toBeInTheDocument();
  expect(listRecent().map((e) => e.id)).toEqual(["1"]);

  await logout({
    queryClient,
    navigate: (opts) => router.navigate(opts),
  });
  expect(await screen.findByText("login")).toBeInTheDocument();
  // Signed out: nothing is read, and a push writes nothing.
  expect(listRecent()).toEqual([]);
  const before = { ...localStorage };
  pushRecent({ kind: "screen", id: "dashboard" });
  expect({ ...localStorage }).toEqual(before);

  // Bob signs in in the same tab (no reload) and opens an item.
  server.current = BOB;
  await router.navigate({ to: "/item/2" as "/" });
  expect(await screen.findByText("item 2")).toBeInTheDocument();
  expect(listRecent().map((e) => e.id)).toEqual(["2"]);
  expect(stored(BOB.id)).toContain('"2"');
  // Alice's list never sees Bob's item.
  expect(stored(ALICE.id)).not.toContain('"2"');
  expect(JSON.parse(stored(ALICE.id) ?? "[]")).toHaveLength(1);
});

test("push_without_a_viewer_writes_nothing", () => {
  pushRecent({ kind: "item", type: "post", id: "x", title: "X", status: "draft" });
  expect(localStorage.length).toBe(0);
  expect(listRecent()).toEqual([]);
});
