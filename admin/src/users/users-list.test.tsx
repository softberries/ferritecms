import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ME, user } from "./fixtures";
import { UsersScreen } from "./users-list";

const SELF = user({ id: ME.id, login: ME.login, display_name: ME.display_name, role: "admin" });

test("list_sends_filters_and_pages_on", async () => {
  const grace = user({ totp_enabled: true });
  const linus = user({
    id: "33333333-3333-4333-8333-333333333333",
    login: "linus",
    display_name: "Linus",
    status: "disabled",
  });
  const seen = stubRoutes({
    "/api/v1/users": (url) =>
      url.searchParams.get("cursor") === "next"
        ? json({ items: [linus], next_cursor: null })
        : json({ items: [SELF, grace], next_cursor: "next" }),
  });
  const setSearch = vi.fn();
  renderWithRouter(
    <UsersScreen
      search={{ q: "a", role: "editor", status: "active" }}
      setSearch={setSearch}
      me={ME}
    />,
  );
  expect(await screen.findByText("Grace Hopper")).toBeInTheDocument();
  const first = seen[0];
  expect(first?.searchParams.get("q")).toBe("a");
  expect(first?.searchParams.get("role")).toBe("editor");
  expect(first?.searchParams.get("status")).toBe("active");
  expect(first?.searchParams.get("limit")).toBe("50");
  // The viewer's own row says so; two-factor shows as a badge.
  expect(screen.getAllByText("You").length).toBeGreaterThan(0);
  expect(screen.getAllByText("Two-factor authentication on").length).toBeGreaterThan(0);

  await userEvent.click(screen.getByRole("button", { name: "Load more" }));
  expect(await screen.findByText("Linus")).toBeInTheDocument();
  expect(seen.at(-1)?.searchParams.get("cursor")).toBe("next");

  // The role chip in force is pressed; pressing it again clears it.
  const editor = screen.getByRole("button", { name: "Editor", pressed: true });
  await userEvent.click(editor);
  expect(setSearch).toHaveBeenCalledWith({ role: undefined });
  await userEvent.click(screen.getByRole("button", { name: "Disabled" }));
  expect(setSearch).toHaveBeenCalledWith({ status: "disabled" });

  // A row opens its user in the sheet (`?id=`).
  await userEvent.click(screen.getByRole("button", { name: "Open Grace Hopper" }));
  expect(setSearch).toHaveBeenCalledWith({ id: grace.id });
});

test("search_waits_250ms", async () => {
  stubRoutes({ "/api/v1/users": () => json({ items: [SELF], next_cursor: null }) });
  const setSearch = vi.fn();
  renderWithRouter(<UsersScreen search={{}} setSearch={setSearch} me={ME} />);
  await userEvent.type(await screen.findByRole("searchbox", { name: "Search users" }), "gr");
  expect(setSearch).not.toHaveBeenCalled();
  await waitFor(() => expect(setSearch).toHaveBeenCalledWith({ q: "gr" }), { timeout: 1000 });
});
