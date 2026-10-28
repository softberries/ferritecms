import { screen } from "@testing-library/react";
import { expect, test } from "vitest";
import type { MeDto } from "@/auth/session";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { TokensCard } from "./tokens-card";

const ME = {
  id: "11111111-1111-4111-8111-111111111111",
  login: "ada",
  email: "ada@example.test",
  display_name: "Ada",
  role: "author",
  must_reset: false,
  totp_enabled: false,
  capabilities: ["read_admin"],
  scopes: ["content:write"],
  csrf_token: "csrf",
} satisfies MeDto;

test("new_token_is_off_at_the_limit", async () => {
  const items = Array.from({ length: 50 }, (_, i) => ({
    id: `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`,
    name: `Token ${i}`,
    scopes: ["content:read"],
    created_at: "2026-10-27T10:00:00Z",
    expires_at: null,
    last_used_at: null,
  }));
  stubRoutes({ [`/api/v1/users/${ME.id}/tokens`]: () => json({ items, next_cursor: null }) });
  renderWithRouter(<TokensCard me={ME} />);
  expect(await screen.findByText("50 of 50 tokens")).toBeInTheDocument();
  const button = screen.getByRole("button", { name: "New token" });
  expect(button).toBeDisabled();
  expect(button).toHaveAccessibleDescription(
    "You have 50 tokens, the most an account can hold. Revoke one to make a new one.",
  );
});
