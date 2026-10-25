import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { act, render, screen } from "@testing-library/react";
import { LazyMotion, domMax } from "motion/react";
import { useEffect } from "react";
import { expect, test } from "vitest";
import { RouteTransition } from "./route-transition";

const mounts = { a: 0, b: 0 };

function Probe({ name }: { name: "a" | "b" }) {
  useEffect(() => {
    mounts[name] += 1;
  }, [name]);
  return <h1>{name}</h1>;
}

test("one_navigation_mounts_the_new_route_once", async () => {
  const root = createRootRoute({
    component: () => (
      <LazyMotion features={domMax}>
        <RouteTransition>
          <Outlet />
        </RouteTransition>
      </LazyMotion>
    ),
  });
  const a = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <Probe name="a" />,
  });
  const b = createRoute({
    getParentRoute: () => root,
    path: "/b",
    component: () => <Probe name="b" />,
  });
  const router = createRouter({
    routeTree: root.addChildren([a, b]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(<RouterProvider router={router} />);
  expect(await screen.findByRole("heading", { name: "a" })).toBeInTheDocument();
  expect(mounts.a).toBe(1);

  // The history, not the typed `navigate`: this tree is not the registered app router.
  await act(async () => {
    router.history.push("/b");
    await Promise.resolve();
  });
  expect(await screen.findByRole("heading", { name: "b" })).toBeInTheDocument();
  // Let any exit or presence bookkeeping run.
  await act(() => new Promise((r) => setTimeout(r, 400)));
  expect(mounts.b).toBe(1);
  expect(screen.getAllByRole("heading")).toHaveLength(1);
  expect(document.querySelectorAll("[data-slot=route-transition]")).toHaveLength(1);
});
