import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { SessionsCard } from "./sessions-card";

const AT = "2026-10-27T09:00:00Z";

test("current_session_has_no_sign_out", async () => {
  let items = [
    { id: "aaaaaaaaaaaaaaaa", current: false, created_at: AT, last_seen_at: AT },
    { id: "bbbbbbbbbbbbbbbb", current: true, created_at: AT, last_seen_at: AT },
    { id: "cccccccccccccccc", current: false, created_at: AT, last_seen_at: AT },
  ];
  const calls: string[] = [];
  stubRoutes({
    "/api/v1/auth/sessions": () => json({ items }),
    "/api/v1/auth/sessions/aaaaaaaaaaaaaaaa": (_url, req) => {
      calls.push(`${req.method} a`);
      items = items.filter((s) => s.id !== "aaaaaaaaaaaaaaaa");
      return new Response(null, { status: 204 });
    },
    "/api/v1/auth/sessions/revoke-others": (_url, req) => {
      calls.push(`${req.method} others`);
      items = items.filter((s) => s.current);
      return json({ revoked: 1 });
    },
  });
  renderWithRouter(<SessionsCard />);
  const list = await screen.findByRole("list", { name: "Sessions" });
  const rows = within(list).getAllByRole("listitem");
  expect(rows).toHaveLength(3);
  // This session is pinned first, whatever the server's order, and has no Sign out.
  const current = rows[0] as HTMLElement;
  expect(current).toHaveTextContent("This session");
  expect(within(current).queryByRole("button")).toBeNull();

  await userEvent.click(within(rows[1] as HTMLElement).getByRole("button", { name: /Sign out/ }));
  await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(2));
  await userEvent.click(screen.getByRole("button", { name: "Sign out all other sessions" }));
  await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(1));
  expect(calls).toEqual(["DELETE a", "POST others"]);
  expect(screen.getByRole("button", { name: "Sign out all other sessions" })).toBeDisabled();
  expect(screen.getByText("No other session is signed in as you.")).toBeInTheDocument();
});
