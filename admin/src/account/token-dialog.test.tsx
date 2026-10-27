import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { expiresAt, offeredScopes } from "./account-model";
import { TokenDialog } from "./token-dialog";

beforeAll(() => {
  // Radix Select measures and captures the pointer; jsdom has neither.
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
  proto.hasPointerCapture ??= () => false;
  proto.releasePointerCapture ??= () => {};
});

const ME = { id: "11111111-1111-4111-8111-111111111111" };

test("token_scopes_offer_only_held_scopes", async () => {
  // `write` implies `read`; an area the session holds nothing of is not offered.
  expect(offeredScopes(["content:write", "media:read", "system:read"])).toEqual([
    { area: "content", write: true },
    { area: "media", write: false },
    { area: "system", write: false },
  ]);

  const seen = stubRoutes({
    [`/api/v1/users/${ME.id}/tokens`]: async (_url, req) => {
      const body = (await req.json()) as { name: string; scopes: string[] };
      return json(
        {
          id: "22222222-2222-4222-8222-222222222222",
          name: body.name,
          scopes: body.scopes,
          created_at: "2026-10-27T10:00:00Z",
          expires_at: null,
          last_used_at: null,
          token: "fer_secret_value_once",
        },
        201,
      );
    },
  });
  renderWithRouter(
    <TokenDialog
      open
      onOpenChange={() => {}}
      userId={ME.id}
      held={["content:write", "media:read", "system:read"]}
      onCreated={() => {}}
    />,
  );
  const content = await screen.findByRole("radiogroup", { name: "Content" });
  expect(within(content).getAllByRole("radio")).toHaveLength(3);
  for (const name of ["None", "Read", "Write"]) within(content).getByRole("radio", { name });
  expect(within(content).getByRole("radio", { name: "None" })).toBeChecked();
  const media = screen.getByRole("radiogroup", { name: "Media" });
  expect(within(media).getAllByRole("radio")).toHaveLength(2);
  expect(within(media).queryByRole("radio", { name: "Write" })).toBeNull();
  expect(screen.queryByRole("radiogroup", { name: "Users" })).toBeNull();
  expect(screen.queryByRole("radiogroup", { name: "Settings" })).toBeNull();

  // Without a scope the form refuses before asking the server.
  await userEvent.type(screen.getByLabelText("Name"), "Deploy bot");
  await userEvent.click(screen.getByRole("button", { name: "Create token" }));
  expect(await screen.findByText("Choose at least one scope.")).toBeInTheDocument();
  expect(seen).toHaveLength(0);

  await userEvent.click(within(content).getByRole("radio", { name: "Write" }));
  await userEvent.click(within(media).getByRole("radio", { name: "Read" }));
  await userEvent.click(screen.getByRole("button", { name: "Create token" }));
  // Shown once, in the dialog.
  expect(await screen.findByDisplayValue("fer_secret_value_once")).toBeInTheDocument();
  expect(seen).toHaveLength(1);
});

test("token_expiry_is_rfc3339_utc", () => {
  const now = Date.UTC(2026, 9, 27, 10, 30, 15, 123);
  expect(expiresAt("never", now)).toBeNull();
  expect(expiresAt("30", now)).toBe("2026-11-26T10:30:15Z");
  expect(expiresAt("365", now)).toBe("2027-10-27T10:30:15Z");
});

test("token_request_carries_scopes_and_expiry", async () => {
  const bodies: unknown[] = [];
  stubRoutes({
    [`/api/v1/users/${ME.id}/tokens`]: async (_url, req) => {
      bodies.push(await req.json());
      return json(
        {
          id: "22222222-2222-4222-8222-222222222222",
          name: "x",
          scopes: [],
          created_at: "2026-10-27T10:00:00Z",
          token: "fer_x",
        },
        201,
      );
    },
  });
  vi.useFakeTimers({ toFake: ["Date"], now: Date.UTC(2026, 9, 27, 10, 0, 0) });
  try {
    renderWithRouter(
      <TokenDialog
        open
        onOpenChange={() => {}}
        userId={ME.id}
        held={["content:write", "media:write"]}
        onCreated={() => {}}
      />,
    );
    await userEvent.type(await screen.findByLabelText("Name"), "CI");
    await userEvent.click(
      within(screen.getByRole("radiogroup", { name: "Content" })).getByRole("radio", {
        name: "Read",
      }),
    );
    await userEvent.click(
      within(screen.getByRole("radiogroup", { name: "Media" })).getByRole("radio", {
        name: "Write",
      }),
    );
    await userEvent.click(screen.getByRole("combobox", { name: "Expires" }));
    await userEvent.click(await screen.findByRole("option", { name: "In 90 days" }));
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    await waitFor(() => expect(bodies).toHaveLength(1));
    expect(bodies[0]).toEqual({
      name: "CI",
      scopes: ["content:read", "media:write"],
      expires_at: "2027-01-25T10:00:00Z",
    });
  } finally {
    vi.useRealTimers();
  }
});
