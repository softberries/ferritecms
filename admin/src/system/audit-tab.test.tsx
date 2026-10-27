import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { AuditTab } from "./audit-tab";
import { ADMIN_ID, audit } from "./fixtures";

beforeAll(() => {
  // jsdom has no scrolling; cmdk scrolls the selected option into view.
  if (!("scrollIntoView" in Element.prototype)) {
    Object.defineProperty(Element.prototype, "scrollIntoView", { value: () => {} });
  }
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const USERS = {
  items: [
    {
      id: ADMIN_ID,
      login: "admin",
      email: "admin@example.com",
      display_name: "Ada Admin",
      role: "admin",
      status: "active",
      must_reset: false,
      totp_enabled: false,
      created_at: "2026-10-01T08:00:00Z",
      last_login_at: null,
    },
  ],
  next_cursor: null,
};

test("audit_rows_are_readable_text", async () => {
  stubRoutes({
    "/api/v1/system/audit": () =>
      json({
        items: [
          audit({
            id: "a1",
            action: "token.create",
            target_kind: "api_token",
            target_id: "0190a1b2-c3d4-7e5f-8a9b-0000000000c1",
            details: {
              name: "<img src=x onerror=alert(1)>",
              scopes: ["content:read", "media:write"],
            } as unknown as Record<string, never>,
          }),
          audit({ id: "a2", action: "backup.create", actor: null, target_kind: "backup" }),
        ],
        next_cursor: null,
      }),
  });
  renderWithRouter(<AuditTab search={{}} setSearch={() => {}} />);
  expect(await screen.findByText("Created an API token")).toBeVisible();
  const first = document.querySelector<HTMLElement>('tr[data-row-id="a1"]');
  if (!first) throw new Error("no row");
  expect(within(first).getByText("token.create")).toBeVisible();
  expect(within(first).getAllByText("API token").length).toBeGreaterThan(0);
  // Details: a key-value list of text.
  const details = within(first).getByRole("list", { name: "Details" });
  expect(within(details).getByText("<img src=x onerror=alert(1)>")).toBeVisible();
  expect(within(details).getByText("content:read, media:write")).toBeVisible();
  expect(document.querySelector("img")).toBeNull();
  // Unknown actions raw; no actor: done outside a signed-in account.
  const second = document.querySelector<HTMLElement>('tr[data-row-id="a2"]');
  if (!second) throw new Error("no row");
  expect(within(second).getAllByText("backup.create").length).toBeGreaterThan(0);
  expect(within(second).getAllByText("System").length).toBeGreaterThan(0);
});

test("filters_reach_the_request_and_the_url", async () => {
  const setSearch = vi.fn();
  const seen = stubRoutes({
    "/api/v1/system/audit": () => json({ items: [], next_cursor: null }),
    "/api/v1/users": () => json(USERS),
    [`/api/v1/users/${ADMIN_ID}`]: () => json(USERS.items[0]),
  });
  renderWithRouter(
    <AuditTab
      search={{ actor: ADMIN_ID, action: "user.", target_kind: "user" }}
      setSearch={setSearch}
    />,
  );
  expect(await screen.findByText("No entries match")).toBeVisible();
  const request = seen.find((u) => u.pathname === "/api/v1/system/audit");
  expect(request?.searchParams.get("actor")).toBe(ADMIN_ID);
  expect(request?.searchParams.get("action")).toBe("user.");
  expect(request?.searchParams.get("target_kind")).toBe("user");
  expect(request?.searchParams.get("limit")).toBe("50");
  // The chips say what narrows the list.
  expect(
    await screen.findByRole("button", { name: "Actor: Ada Admin. Change actor" }),
  ).toBeVisible();
  expect(
    screen.getByRole("button", { name: "Action: starts with user.. Change action" }),
  ).toBeVisible();

  // "Starts with": a typed prefix ending in a dot.
  await userEvent.click(
    screen.getByRole("button", { name: "Action: starts with user.. Change action" }),
  );
  const box = await screen.findByRole("combobox", { name: "Find an action" });
  await userEvent.type(box, "token.");
  await userEvent.click(await screen.findByRole("option", { name: "Starts with token." }));
  expect(setSearch).toHaveBeenLastCalledWith({ action: "token." });

  // An exact action from the list.
  await userEvent.click(
    screen.getByRole("button", { name: "Action: starts with user.. Change action" }),
  );
  await userEvent.click(await screen.findByRole("option", { name: /^Revoked an API token/ }));
  expect(setSearch).toHaveBeenLastCalledWith({ action: "token.revoke" });

  // Clear all.
  await userEvent.click(screen.getAllByRole("button", { name: "Clear filters" })[0]!);
  expect(setSearch).toHaveBeenLastCalledWith({
    actor: undefined,
    action: undefined,
    target_kind: undefined,
    target_id: undefined,
  });
});

test("actor_picker_searches_users", async () => {
  const setSearch = vi.fn();
  const seen = stubRoutes({
    "/api/v1/system/audit": () => json({ items: [audit()], next_cursor: null }),
    "/api/v1/users": () => json(USERS),
  });
  renderWithRouter(<AuditTab search={{}} setSearch={setSearch} />);
  await screen.findByText("Changed site settings");
  await userEvent.click(screen.getByRole("button", { name: "Actor" }));
  const box = await screen.findByRole("combobox", { name: "Find a user" });
  await userEvent.type(box, "ada");
  await waitFor(() =>
    expect(
      seen.some((u) => u.pathname === "/api/v1/users" && u.searchParams.get("q") === "ada"),
    ).toBe(true),
  );
  await userEvent.click(await screen.findByRole("option", { name: /Ada Admin/ }));
  expect(setSearch).toHaveBeenLastCalledWith({ actor: ADMIN_ID });
});
