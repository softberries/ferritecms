import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { DeleteUserDialog } from "./delete-user-dialog";
import { ME, invalid, noContent, user } from "./fixtures";

const GRACE = user();
const HEIR = user({
  id: "33333333-3333-4333-8333-333333333333",
  login: "linus",
  email: "linus@example.test",
  display_name: "Linus",
  role: "author",
});

test("delete_dialog_switches_to_reassign_on_422", async () => {
  const deletes: URL[] = [];
  const lists: URL[] = [];
  stubRoutes({
    [`/api/v1/users/${GRACE.id}`]: (url, req) => {
      expect(req.method).toBe("DELETE");
      deletes.push(url);
      return url.searchParams.has("reassign")
        ? noContent()
        : invalid("reassign the user's content first", "reassign");
    },
    "/api/v1/users": (url) => {
      lists.push(url);
      // The deleted user is in the answer too: the picker leaves it out.
      const self = user({
        id: ME.id,
        login: ME.login,
        display_name: ME.display_name,
        role: "admin",
      });
      return json({ items: [self, GRACE, HEIR], next_cursor: null });
    },
  });
  const onDeleted = vi.fn();
  renderWithRouter(<DeleteUserDialog user={GRACE} onClose={() => {}} onDeleted={onDeleted} />);

  const dialog = await screen.findByRole("alertdialog");
  expect(within(dialog).getByText(/their sessions end/i)).toBeInTheDocument();
  expect(within(dialog).getByText(/API tokens are revoked/i)).toBeInTheDocument();
  await userEvent.click(within(dialog).getByRole("button", { name: "Delete user" }));

  // The same dialog now asks who takes the content over.
  expect(
    await within(dialog).findByText("This user owns content. Reassign it to:"),
  ).toBeInTheDocument();
  expect(deletes).toHaveLength(1);
  expect(deletes[0]?.searchParams.has("reassign")).toBe(false);
  const confirm = within(dialog).getByRole("button", { name: "Reassign and delete" });
  expect(confirm).toBeDisabled();
  // Only other active users are offered.
  await waitFor(() => expect(lists.length).toBeGreaterThan(0));
  expect(lists[0]?.searchParams.get("status")).toBe("active");
  const heir = await within(dialog).findByRole("radio", { name: /Linus/ });
  expect(within(dialog).queryByRole("radio", { name: /Grace Hopper/ })).toBeNull();
  expect(within(dialog).getByRole("radio", { name: /Ada Lovelace/ })).toBeInTheDocument();

  // Searching asks the server (`q`).
  await userEvent.type(within(dialog).getByRole("searchbox", { name: "Search users" }), "lin");
  await waitFor(() => expect(lists.some((u) => u.searchParams.get("q") === "lin")).toBe(true));

  await userEvent.click(heir);
  await userEvent.click(confirm);
  await waitFor(() => expect(onDeleted).toHaveBeenCalledWith(GRACE));
  expect(deletes).toHaveLength(2);
  expect(deletes[1]?.searchParams.get("reassign")).toBe(HEIR.id);
});

test("delete_refusal_shows_its_detail", async () => {
  stubRoutes({
    [`/api/v1/users/${GRACE.id}`]: () => invalid("the site needs at least one active admin"),
  });
  const onDeleted = vi.fn();
  renderWithRouter(<DeleteUserDialog user={GRACE} onClose={() => {}} onDeleted={onDeleted} />);
  const dialog = await screen.findByRole("alertdialog");
  await userEvent.click(within(dialog).getByRole("button", { name: "Delete user" }));
  expect(await within(dialog).findByRole("alert")).toHaveTextContent(
    "the site needs at least one active admin",
  );
  // Still the first step: no picker.
  expect(within(dialog).queryByText("This user owns content. Reassign it to:")).toBeNull();
  expect(onDeleted).not.toHaveBeenCalled();
});
