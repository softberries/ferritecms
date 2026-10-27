import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, expect, test } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ME, invalid, noContent, user } from "./fixtures";
import { UserSheet } from "./user-sheet";

beforeAll(() => {
  // Radix Select measures and captures the pointer; jsdom has neither.
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
  proto.hasPointerCapture ??= () => false;
  proto.releasePointerCapture ??= () => {};
});

const SELF = user({
  id: ME.id,
  login: ME.login,
  email: ME.email,
  display_name: ME.display_name,
  role: "admin",
  totp_enabled: true,
});
const TOKEN = {
  id: "44444444-4444-4444-8444-444444444444",
  name: "Deploy script",
  scopes: ["content:read"],
  created_at: "2026-10-01T10:00:00Z",
  expires_at: null,
  last_used_at: null,
};

test("own_row_hides_controls", async () => {
  stubRoutes({
    [`/api/v1/users/${ME.id}`]: () => json(SELF),
    [`/api/v1/users/${ME.id}/tokens`]: () => json({ items: [TOKEN], next_cursor: null }),
  });
  renderWithRouter(<UserSheet id={ME.id} me={ME} onClose={() => {}} onDeleted={() => {}} />);
  const sheet = await screen.findByRole("dialog");
  expect(await within(sheet).findByRole("heading", { name: "Ada Lovelace" })).toBeInTheDocument();
  const link = within(sheet).getByRole("link", { name: "Manage your account" });
  expect(link).toHaveAttribute("href", "/admin/account");
  // Their tokens are still listed.
  expect(await within(sheet).findByText("Deploy script")).toBeInTheDocument();
  for (const name of ["Role", "Status"]) {
    expect(within(sheet).queryByRole("combobox", { name })).toBeNull();
  }
  for (const name of ["Set password", "Turn off two-factor", "Delete user", "Save profile"]) {
    expect(within(sheet).queryByRole("button", { name })).toBeNull();
  }
});

test("other_row_has_the_controls", async () => {
  const grace = user({ totp_enabled: true });
  stubRoutes({
    [`/api/v1/users/${grace.id}`]: () => json(grace),
    [`/api/v1/users/${grace.id}/tokens`]: () => json({ items: [], next_cursor: null }),
  });
  renderWithRouter(<UserSheet id={grace.id} me={ME} onClose={() => {}} onDeleted={() => {}} />);
  const sheet = await screen.findByRole("dialog");
  expect(await within(sheet).findByRole("combobox", { name: "Role" })).toBeInTheDocument();
  expect(within(sheet).getByRole("combobox", { name: "Status" })).toBeInTheDocument();
  for (const name of ["Set password", "Turn off two-factor", "Delete user", "Save profile"]) {
    expect(within(sheet).getByRole("button", { name })).toBeInTheDocument();
  }
  expect(within(sheet).queryByRole("link", { name: "Manage your account" })).toBeNull();
});

test("last_admin_refusal_is_shown", async () => {
  // From the UI this needs a race (an active admin acting on another admin is never the
  // last one), so the server's refusal is mocked.
  const other = user({ role: "admin" });
  const patches: unknown[] = [];
  stubRoutes({
    [`/api/v1/users/${other.id}`]: async (_url, req) => {
      if (req.method === "PATCH") {
        patches.push(await req.json());
        return invalid("the site needs at least one active admin");
      }
      return json(other);
    },
    [`/api/v1/users/${other.id}/tokens`]: () => json({ items: [], next_cursor: null }),
  });
  renderWithRouter(<UserSheet id={other.id} me={ME} onClose={() => {}} onDeleted={() => {}} />);
  const sheet = await screen.findByRole("dialog");
  await userEvent.click(await within(sheet).findByRole("combobox", { name: "Role" }));
  await userEvent.click(await screen.findByRole("option", { name: "Editor" }));

  const confirm = await screen.findByRole("alertdialog");
  expect(within(confirm).getByText(/sessions end/i)).toBeInTheDocument();
  await userEvent.click(within(confirm).getByRole("button", { name: "Change role" }));
  expect(await within(confirm).findByRole("alert")).toHaveTextContent(
    "the site needs at least one active admin",
  );
  expect(patches).toEqual([{ role: "editor" }]);
  // Still open, and the role shown is unchanged.
  expect(screen.getByRole("alertdialog")).toBeInTheDocument();
});

test("disable_confirm_says_tokens_are_revoked", async () => {
  const grace = user();
  const patches: unknown[] = [];
  stubRoutes({
    [`/api/v1/users/${grace.id}`]: async (_url, req) => {
      if (req.method === "PATCH") {
        const body = (await req.json()) as Record<string, unknown>;
        patches.push(body);
        return json({ ...grace, ...body });
      }
      return json(grace);
    },
    [`/api/v1/users/${grace.id}/tokens`]: () => json({ items: [], next_cursor: null }),
  });
  renderWithRouter(<UserSheet id={grace.id} me={ME} onClose={() => {}} onDeleted={() => {}} />);
  const sheet = await screen.findByRole("dialog");
  await userEvent.click(await within(sheet).findByRole("combobox", { name: "Status" }));
  await userEvent.click(await screen.findByRole("option", { name: "Disabled" }));
  const confirm = await screen.findByRole("alertdialog");
  expect(within(confirm).getByText(/sessions end/i)).toBeInTheDocument();
  expect(within(confirm).getByText(/API tokens are revoked/i)).toBeInTheDocument();
  await userEvent.click(within(confirm).getByRole("button", { name: "Disable account" }));
  await waitFor(() => expect(patches).toEqual([{ status: "disabled" }]));
});

test("profile_edit_asks_when_changed_by_someone_else", async () => {
  const grace = user();
  let stored = grace;
  const patches: unknown[] = [];
  stubRoutes({
    [`/api/v1/users/${grace.id}`]: async (_url, req) => {
      if (req.method === "PATCH") {
        const body = (await req.json()) as Record<string, unknown>;
        patches.push(body);
        stored = { ...stored, ...body };
      }
      return json(stored);
    },
    [`/api/v1/users/${grace.id}/tokens`]: () => json({ items: [], next_cursor: null }),
  });
  renderWithRouter(<UserSheet id={grace.id} me={ME} onClose={() => {}} onDeleted={() => {}} />);
  const sheet = await screen.findByRole("dialog");
  const name = await within(sheet).findByLabelText("Display name");
  // Someone else renames her meanwhile.
  stored = { ...grace, display_name: "Rear Admiral Hopper" };
  await userEvent.clear(name);
  await userEvent.type(name, "Grace B. Hopper");
  await userEvent.click(within(sheet).getByRole("button", { name: "Save profile" }));
  const stale = await screen.findByRole("alertdialog");
  expect(within(stale).getByText(/changed by someone else/i)).toBeInTheDocument();
  expect(patches).toEqual([]);
  await userEvent.click(within(stale).getByRole("button", { name: /overwrite/i }));
  await waitFor(() => expect(patches).toEqual([{ display_name: "Grace B. Hopper" }]));
});

test("set_password_says_they_must_change_it", async () => {
  const grace = user();
  const posts: unknown[] = [];
  stubRoutes({
    [`/api/v1/users/${grace.id}`]: () => json(grace),
    [`/api/v1/users/${grace.id}/tokens`]: () => json({ items: [], next_cursor: null }),
    [`/api/v1/users/${grace.id}/password`]: async (_url, req) => {
      posts.push(await req.json());
      return noContent();
    },
  });
  renderWithRouter(<UserSheet id={grace.id} me={ME} onClose={() => {}} onDeleted={() => {}} />);
  const sheet = await screen.findByRole("dialog");
  await userEvent.click(await within(sheet).findByRole("button", { name: "Set password" }));
  const dialog = await screen.findByRole("dialog", { name: /set a password/i });
  expect(within(dialog).getByText(/must change it/i)).toBeInTheDocument();
  expect(within(dialog).getByText(/sessions end/i)).toBeInTheDocument();
  await userEvent.type(within(dialog).getByLabelText("New password"), "a-long-new-pass-1");
  await userEvent.click(within(dialog).getByRole("button", { name: "Set password" }));
  await waitFor(() => expect(posts).toEqual([{ password: "a-long-new-pass-1" }]));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: /set a password/i })).toBeNull());
});

test("turn_off_two_factor_confirms", async () => {
  const grace = user({ totp_enabled: true });
  const calls: string[] = [];
  stubRoutes({
    [`/api/v1/users/${grace.id}`]: () => json(grace),
    [`/api/v1/users/${grace.id}/tokens`]: () => json({ items: [], next_cursor: null }),
    [`/api/v1/users/${grace.id}/totp/disable`]: (_url, req) => {
      calls.push(req.method);
      return noContent();
    },
  });
  renderWithRouter(<UserSheet id={grace.id} me={ME} onClose={() => {}} onDeleted={() => {}} />);
  const sheet = await screen.findByRole("dialog");
  await userEvent.click(await within(sheet).findByRole("button", { name: "Turn off two-factor" }));
  const confirm = await screen.findByRole("alertdialog");
  expect(calls).toEqual([]);
  await userEvent.click(within(confirm).getByRole("button", { name: "Turn off" }));
  await waitFor(() => expect(calls).toEqual(["POST"]));
});
