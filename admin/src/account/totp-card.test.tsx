import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { MeDto } from "@/auth/session";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { TotpCard } from "./totp-card";

const ME: MeDto = {
  id: "11111111-1111-4111-8111-111111111111",
  login: "ada",
  email: "ada@example.test",
  display_name: "Ada",
  role: "author",
  must_reset: false,
  totp_enabled: false,
  capabilities: ["read_admin", "edit_own"],
  scopes: [],
  csrf_token: "csrf",
};

const SECRETS = ["JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP", "KRSXG5CTMVRXEZLUKRSXG5CTMVRXEZLU"];

function problem(status: number, slug: string, detail = ""): Response {
  return new Response(
    JSON.stringify({
      type: `https://ferritecms.dev/problems/${slug}`,
      title: slug,
      status,
      detail,
    }),
    { status, headers: { "Content-Type": "application/problem+json" } },
  );
}

/** Everything the query and mutation caches hold, as text. */
function cachesText(qc: ReturnType<typeof renderWithRouter>): string {
  return JSON.stringify([
    qc
      .getQueryCache()
      .getAll()
      .map((q) => [q.queryKey, q.state.data]),
    qc
      .getMutationCache()
      .getAll()
      .map((m) => [m.state.variables, m.state.data]),
  ]);
}

test("totp_dialog_drops_secret_on_close", async () => {
  let setups = 0;
  stubRoutes({
    "/api/v1/auth/totp/setup": () => {
      const secret = SECRETS[setups] ?? "";
      setups += 1;
      return json({
        secret_base32: secret,
        otpauth_url: `otpauth://totp/Site:ada?secret=${secret}&issuer=Site`,
        enrollment: `sealed-${setups}`,
      });
    },
  });
  const qc = renderWithRouter(<TotpCard me={ME} />);
  expect(await screen.findByText("Off")).toBeInTheDocument();

  await userEvent.click(await screen.findByRole("button", { name: "Set up" }));
  // The secret in groups of four, beside the QR code.
  expect(await screen.findByText("JBSW Y3DP EHPK 3PXP JBSW Y3DP EHPK 3PXP")).toBeInTheDocument();
  expect(screen.getByRole("img", { name: /Scan/ })).toBeInTheDocument();

  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(document.body.textContent).not.toContain("JBSW");
  expect(cachesText(qc)).not.toContain(SECRETS[0]);

  // A new opening starts a new enrolment: the old secret never comes back.
  await userEvent.click(await screen.findByRole("button", { name: "Set up" }));
  expect(await screen.findByText("KRSX G5CT MVRX EZLU KRSX G5CT MVRX EZLU")).toBeInTheDocument();
  expect(document.body.textContent).not.toContain("JBSW");
  expect(setups).toBe(2);
  expect(cachesText(qc)).not.toContain(SECRETS[1]);
});

test("totp_confirm_errors_show_inline", async () => {
  let answer: () => Response = () => problem(422, "validation", "wrong code");
  const bodies: unknown[] = [];
  stubRoutes({
    "/api/v1/auth/totp/setup": () =>
      json({
        secret_base32: SECRETS[0],
        otpauth_url: `otpauth://totp/Site:ada?secret=${SECRETS[0]}`,
        enrollment: "sealed",
      }),
    "/api/v1/auth/totp/confirm": async (_url, req) => {
      bodies.push(await req.json());
      return answer();
    },
  });
  renderWithRouter(<TotpCard me={ME} />);
  await userEvent.click(await screen.findByRole("button", { name: "Set up" }));
  await userEvent.click(await screen.findByRole("button", { name: "Next" }));
  await userEvent.type(screen.getByLabelText("Six-digit code"), "123456");
  await userEvent.type(screen.getByLabelText("Your password"), "pw-0123456789");
  await userEvent.click(screen.getByRole("button", { name: "Turn on" }));
  expect(await screen.findByText(/That code is not right/)).toBeInTheDocument();
  expect(bodies[0]).toEqual({ enrollment: "sealed", code: "123456", password: "pw-0123456789" });

  // The wrong code is cleared for the next one.
  expect(screen.getByLabelText("Six-digit code")).toHaveValue("");
  answer = () => problem(403, "forbidden", "current password");
  await userEvent.type(screen.getByLabelText("Six-digit code"), "654321");
  await userEvent.click(screen.getByRole("button", { name: "Turn on" }));
  expect(await screen.findByText("Wrong password.")).toBeInTheDocument();
});
