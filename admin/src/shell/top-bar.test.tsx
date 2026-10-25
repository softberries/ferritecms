import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { Crumbs } from "./top-bar";

test("non_last_crumb_links_inside_the_admin_once", async () => {
  const root = createRootRoute({
    component: () => (
      <>
        <Crumbs />
        <Outlet />
      </>
    ),
  });
  const parent = createRoute({
    getParentRoute: () => root,
    path: "/parent",
    staticData: { crumb: () => "Parent" },
    component: Outlet,
  });
  const child = createRoute({
    getParentRoute: () => parent,
    path: "/child",
    staticData: { crumb: () => "Child" },
    component: () => <p>child</p>,
  });
  const router = createRouter({
    routeTree: root.addChildren([parent.addChildren([child])]),
    basepath: "/admin",
    history: createMemoryHistory({ initialEntries: ["/admin/parent/child"] }),
  });
  render(<RouterProvider router={router} />);
  expect(await screen.findByText("child")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Parent" })).toHaveAttribute("href", "/admin/parent");
  expect(screen.getByText("Child")).toHaveAttribute("aria-current", "page");
});
