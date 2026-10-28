import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ME } from "@/users/fixtures";
import { MailGroup } from "./mail";

const SYSTEM = { ...ME, capabilities: ["read_admin", "manage_settings", "manage_system"] };

function status(mail: { configured: boolean; from: string | null }) {
  return {
    database: "sqlite",
    mode: "solo",
    media: "local",
    site_url: "https://example.test",
    version: "0",
    mail,
  };
}

test("mail_panel_shows_from_and_sends_a_test", async () => {
  const posts: string[] = [];
  stubRoutes({
    "/api/v1/system/status": () =>
      json(status({ configured: true, from: "Site <noreply@example.test>" })),
    "/api/v1/system/mail/test": (_url, req) => {
      posts.push(req.method);
      return json({ to: ME.email });
    },
  });
  renderWithRouter(<MailGroup me={SYSTEM} />);
  expect(await screen.findByText("Mail is set up")).toBeInTheDocument();
  expect(screen.getByText("Site <noreply@example.test>")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Send test e-mail" }));
  await waitFor(() => expect(posts).toEqual(["POST"]));
});

test("mail_panel_without_mail_explains_the_config", async () => {
  stubRoutes({
    "/api/v1/system/status": () => json(status({ configured: false, from: null })),
  });
  renderWithRouter(<MailGroup me={SYSTEM} />);
  expect(await screen.findByText("Mail is not set up")).toBeInTheDocument();
  expect(screen.getByText(/\[mail\]/, { selector: "pre" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Send test e-mail" })).toBeNull();
});

test("mail_test_needs_manage_system", async () => {
  // Without `manage_system` the status is not read: `MeDto.mail_configured` says it.
  const seen = stubRoutes({});
  renderWithRouter(
    <MailGroup
      me={{ ...ME, mail_configured: true, capabilities: ["read_admin", "manage_settings"] }}
    />,
  );
  expect(await screen.findByText("Mail is set up")).toBeInTheDocument();
  expect(screen.getByText("Sending a test e-mail needs System access.")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Send test e-mail" })).toBeNull();
  expect(seen).toEqual([]);
});
