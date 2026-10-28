import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { UserDto } from "@/api/users";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ProfileCard } from "./profile-card";

const USER: UserDto = {
  id: "11111111-1111-4111-8111-111111111111",
  login: "ada",
  email: "ada@example.test",
  display_name: "Ada",
  role: "author",
  status: "active",
  must_reset: false,
  totp_enabled: false,
  created_at: "2026-01-02T03:04:05Z",
  last_login_at: "2026-10-27T09:00:00Z",
};

function problem(status: number, slug: string, headers: Record<string, string> = {}): Response {
  return new Response(
    JSON.stringify({ type: `https://ferritecms.dev/problems/${slug}`, title: slug, status }),
    { status, headers: { "Content-Type": "application/problem+json", ...headers } },
  );
}

test("email_change_requires_password", async () => {
  const bodies: unknown[] = [];
  let answer: (body: Record<string, unknown>) => Response = (body) => json({ ...USER, ...body });
  stubRoutes({
    [`/api/v1/users/${USER.id}`]: async (_url, req) => {
      const body = (await req.json()) as Record<string, unknown>;
      bodies.push(body);
      return answer(body);
    },
  });
  renderWithRouter(<ProfileCard user={USER} />);

  // Login, role and dates are read-only.
  expect(await screen.findByText("ada")).toBeInTheDocument();
  expect(screen.getByText("Author")).toBeInTheDocument();
  expect(screen.queryByLabelText("Current password")).toBeNull();

  // A display-name change alone sends no password and no e-mail.
  const name = screen.getByLabelText("Display name");
  await userEvent.clear(name);
  await userEvent.type(name, "Ada L.");
  await userEvent.click(screen.getByRole("button", { name: "Save profile" }));
  await waitFor(() => expect(bodies).toHaveLength(1));
  expect(bodies[0]).toEqual({ display_name: "Ada L." });

  // A different e-mail asks for the current password, and is not sent without it.
  const email = await screen.findByLabelText("E-mail");
  await userEvent.clear(email);
  await userEvent.type(email, "ada@new.test");
  const password = await screen.findByLabelText("Current password");
  await userEvent.click(screen.getByRole("button", { name: "Save profile" }));
  expect(
    await screen.findByText("Enter your current password to change your e-mail."),
  ).toBeInTheDocument();
  expect(bodies).toHaveLength(1);

  // Wrong password: shown at the field.
  answer = () => problem(403, "forbidden");
  await userEvent.type(password, "not-my-password");
  await userEvent.click(screen.getByRole("button", { name: "Save profile" }));
  expect(await screen.findByText("Wrong password.")).toBeInTheDocument();
  expect(bodies[1]).toEqual({ email: "ada@new.test", current_password: "not-my-password" });

  // Right password: saved, and the field goes away with the change.
  answer = (body) => json({ ...USER, display_name: "Ada L.", email: body.email });
  await userEvent.clear(password);
  await userEvent.type(password, "my-password-123");
  await userEvent.click(screen.getByRole("button", { name: "Save profile" }));
  await waitFor(() => expect(screen.queryByLabelText("Current password")).toBeNull());
  expect(bodies[2]).toEqual({ email: "ada@new.test", current_password: "my-password-123" });
});

test("email_change_rate_limit_shows_the_wait", async () => {
  stubRoutes({
    [`/api/v1/users/${USER.id}`]: () => problem(429, "rate-limited", { "Retry-After": "42" }),
  });
  renderWithRouter(<ProfileCard user={USER} />);
  const email = await screen.findByLabelText("E-mail");
  await userEvent.clear(email);
  await userEvent.type(email, "ada@new.test");
  await userEvent.type(await screen.findByLabelText("Current password"), "my-password-123");
  await userEvent.click(screen.getByRole("button", { name: "Save profile" }));
  expect(await screen.findByText(/try again in 42 s/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Save profile" })).toBeDisabled();
});
