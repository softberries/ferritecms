import { createMemoryHistory } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createQueryClient } from "@/api/client";
import { App } from "@/app";
import { createAppRouter } from "@/router";
import { holdResetToken } from "./reset-token";

const META = { api: "v1", version: "0.1.0", site_url: "https://blog.example" };

function validation(field: string, detail: string): Response {
  return new Response(
    JSON.stringify({
      type: "https://ferritecms.dev/problems/validation",
      title: "validation",
      status: 422,
      detail,
      errors: [{ field, detail }],
    }),
    { status: 422, headers: { "Content-Type": "application/problem+json" } },
  );
}

/** The reset page with `token` held, answering the confirm with `confirm`. */
function renderReset(token: string, confirm: () => Response) {
  const sent: unknown[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (req: Request) => {
      const path = new URL(req.url).pathname;
      if (path === "/api/v1/meta") return Response.json(META);
      if (path === "/api/v1/auth/password-reset/confirm") {
        sent.push(await req.json());
        return confirm();
      }
      throw new Error(`unexpected ${req.method} ${path}`);
    }),
  );
  holdResetToken(token);
  const queryClient = createQueryClient({
    navigate: () => undefined,
    currentPath: () => "/admin/reset-password",
    notify: () => undefined,
  });
  const history = createMemoryHistory({ initialEntries: ["/admin/reset-password"] });
  render(<App queryClient={queryClient} router={createAppRouter(queryClient, history)} />);
  return sent;
}

async function submitNewPassword(password: string) {
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText("New password"), password);
  await user.type(screen.getByLabelText("Repeat the new password"), password);
  await user.click(screen.getByRole("button", { name: "Set password" }));
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("reset_confirm_problems", () => {
  it("422 field=token shows the expired-link screen", async () => {
    const sent = renderReset("spent-token", () =>
      validation("token", "the reset link is invalid or has expired"),
    );
    await submitNewPassword("a-long-enough-pass");
    expect(
      await screen.findByRole("heading", { level: 1, name: "Link already used or missing" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("This link is invalid or has expired");
    expect(screen.getByRole("link", { name: "Request a new link" })).toHaveAttribute(
      "href",
      "/admin/forgot-password",
    );
    expect(sent).toEqual([{ token: "spent-token", password: "a-long-enough-pass" }]);
  });

  it("422 field=password shows the server's detail under the field", async () => {
    renderReset("good-token", () => validation("password", "password is too common"));
    await submitNewPassword("a-long-enough-pass");
    expect(await screen.findByText("password is too common")).toBeVisible();
    expect(screen.getByRole("heading", { level: 1, name: "Choose a new password" })).toBeVisible();
  });

  it("204 shows the done screen", async () => {
    renderReset("good-token", () => new Response(null, { status: 204 }));
    await submitNewPassword("a-long-enough-pass");
    expect(
      await screen.findByRole("heading", { level: 1, name: "Password changed" }),
    ).toBeInTheDocument();
  });
});
