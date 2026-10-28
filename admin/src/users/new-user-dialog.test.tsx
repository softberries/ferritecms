import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ME, invalid, user } from "./fixtures";
import { NewUserDialog } from "./new-user-dialog";

beforeAll(() => {
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
  proto.hasPointerCapture ??= () => false;
  proto.releasePointerCapture ??= () => {};
});

async function fill(dialog: HTMLElement) {
  await userEvent.type(within(dialog).getByLabelText("Login"), "linus");
  await userEvent.type(within(dialog).getByLabelText("E-mail"), "linus@example.test");
  await userEvent.type(within(dialog).getByLabelText("Display name"), "Linus");
}

test("set_password_creates_with_must_reset", async () => {
  const bodies: Record<string, unknown>[] = [];
  let answer: () => Response = () => invalid("login taken", "login");
  stubRoutes({
    "/api/v1/users": async (_url, req) => {
      bodies.push((await req.json()) as Record<string, unknown>);
      return answer();
    },
  });
  const onCreated = vi.fn();
  renderWithRouter(<NewUserDialog open onOpenChange={() => {}} me={ME} onCreated={onCreated} />);
  const dialog = await screen.findByRole("dialog", { name: "New user" });
  // No mail configured: no reset e-mail choice.
  expect(within(dialog).queryByRole("radio", { name: /reset e-mail/i })).toBeNull();
  expect(within(dialog).getByText(/must change it when they first sign in/i)).toBeInTheDocument();
  await fill(dialog);
  await userEvent.type(within(dialog).getByLabelText("Password"), "short");
  await userEvent.click(within(dialog).getByRole("button", { name: "Create user" }));
  expect(bodies).toEqual([]);
  const password = within(dialog).getByLabelText("Password");
  await userEvent.clear(password);
  await userEvent.type(password, "a-long-first-pass");
  await userEvent.click(within(dialog).getByRole("button", { name: "Create user" }));
  // A server field error shows at its field.
  expect(await within(dialog).findByText("login taken")).toBeInTheDocument();
  answer = () => json(user({ login: "linus", display_name: "Linus" }), 201);
  await userEvent.click(within(dialog).getByRole("button", { name: "Create user" }));
  await waitFor(() => expect(onCreated).toHaveBeenCalled());
  expect(bodies[1]).toEqual({
    login: "linus",
    email: "linus@example.test",
    display_name: "Linus",
    role: "author",
    password: "a-long-first-pass",
    must_reset: true,
  });
});

test("reset_email_creates_then_sends_the_link", async () => {
  const creates: Record<string, unknown>[] = [];
  const resets: unknown[] = [];
  stubRoutes({
    "/api/v1/users": async (_url, req) => {
      const body = (await req.json()) as Record<string, unknown>;
      creates.push(body);
      return json(user({ login: "linus", email: "linus@example.test" }), 201);
    },
    "/api/v1/auth/password-reset": async (_url, req) => {
      resets.push(await req.json());
      return new Response(null, { status: 202 });
    },
  });
  const onCreated = vi.fn();
  renderWithRouter(
    <NewUserDialog
      open
      onOpenChange={() => {}}
      me={{ ...ME, mail_configured: true }}
      onCreated={onCreated}
    />,
  );
  const dialog = await screen.findByRole("dialog", { name: "New user" });
  await fill(dialog);
  await userEvent.click(within(dialog).getByRole("radio", { name: /send a reset e-mail/i }));
  expect(within(dialog).queryByLabelText("Password")).toBeNull();
  await userEvent.click(within(dialog).getByRole("button", { name: "Create user" }));
  await waitFor(() => expect(onCreated).toHaveBeenCalled());
  expect(creates).toEqual([
    {
      login: "linus",
      email: "linus@example.test",
      display_name: "Linus",
      role: "author",
      must_reset: true,
    },
  ]);
  expect(resets).toEqual([{ login: "linus" }]);
  expect(onCreated.mock.calls[0]?.[1]).toBe("A reset link was sent to linus@example.test");
});
