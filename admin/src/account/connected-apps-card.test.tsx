import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { GrantDto } from "@/api/oauth";
import type { MeDto } from "@/auth/session";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { UserApps } from "@/users/user-apps";
import { ConnectedAppsCard } from "./connected-apps-card";

const ME = {
  id: "11111111-1111-4111-8111-111111111111",
  login: "ada",
  email: "ada@example.test",
  display_name: "Ada",
  role: "editor",
  must_reset: false,
  totp_enabled: false,
  capabilities: ["read_admin"],
  scopes: ["content:write"],
  csrf_token: "csrf",
} satisfies MeDto;

const GRANT: GrantDto = {
  id: "33333333-3333-4333-8333-333333333333",
  user_id: ME.id,
  client: {
    id: "https://claude.ai/oauth/metadata",
    kind: "metadata",
    name: "Claude",
    client_uri: null,
    logo_uri: null,
  },
  scopes: ["content:write", "media:read"],
  created_at: "2026-10-30T10:00:00Z",
  last_used_at: "2026-11-01T10:00:00Z",
  expires_at: "2026-12-01T10:00:00Z",
};
const META = () => json({ api: "v1", site_url: "https://blog.example.test/", version: "0.1.0" });
const SETTINGS = (on: boolean | null) => () => json({ "mcp.enabled": on });

test("lists_apps_and_disconnects_after_a_confirm", async () => {
  let items = [GRANT];
  const deletes: string[] = [];
  const seen = stubRoutes({
    "/api/v1/meta": META,
    "/api/v1/settings": SETTINGS(null),
    "/api/v1/oauth/grants": () => json({ items }),
    [`/api/v1/oauth/grants/${GRANT.id}`]: (_url, req) => {
      deletes.push(req.method);
      items = [];
      return new Response(null, { status: 204 });
    },
  });
  renderWithRouter(<ConnectedAppsCard me={ME} />);
  const list = await screen.findByRole("list", { name: "Connected apps" });
  const app = within(list).getByRole("listitem");
  expect(app).toHaveTextContent("Claude");
  expect(app).toHaveTextContent("Verified");
  expect(app).toHaveTextContent("Writes Content · 2 areas");
  expect(app).toHaveTextContent(/Connected .+ ago.*Last used .+ ago/);
  // The own list asks without `?user=`.
  expect(seen.find((u) => u.pathname === "/api/v1/oauth/grants")?.search).toBe("");

  await userEvent.click(within(app).getByRole("button", { name: "Disconnect Claude" }));
  const confirm = await screen.findByRole("alertdialog", { name: "Disconnect Claude?" });
  expect(confirm).toHaveTextContent("It loses access to this site at once.");
  await userEvent.click(within(confirm).getByRole("button", { name: "Disconnect" }));
  await waitFor(() => expect(deletes).toEqual(["DELETE"]));
  expect(await screen.findByText("No connected apps")).toBeInTheDocument();
});

test("empty_state_invites_connecting_with_the_url", async () => {
  stubRoutes({
    "/api/v1/meta": META,
    "/api/v1/settings": SETTINGS(true),
    "/api/v1/oauth/grants": () => json({ items: [] }),
  });
  renderWithRouter(
    <ConnectedAppsCard me={{ ...ME, capabilities: ["read_admin", "manage_settings"] }} />,
  );
  expect(await screen.findByText("No connected apps")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Connect an assistant" })).toHaveAttribute(
    "href",
    "/admin/settings/ai-assistants",
  );
  expect(screen.getByRole("link", { name: "Read the guide" })).toHaveAttribute(
    "href",
    "https://softberries.github.io/ferritecms/guides/ai-assistants/",
  );
  expect(await screen.findByRole("textbox", { name: "MCP URL" })).toHaveValue(
    "https://blog.example.test/mcp",
  );
});

test("without_manage_settings_the_empty_state_skips_the_settings_link", async () => {
  stubRoutes({
    "/api/v1/meta": META,
    "/api/v1/settings": SETTINGS(null),
    "/api/v1/oauth/grants": () => json({ items: [] }),
  });
  renderWithRouter(<ConnectedAppsCard me={ME} />);
  expect(await screen.findByText("No connected apps")).toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Connect an assistant" })).toBeNull();
});

test("apps_say_they_are_paused_while_the_switch_is_off", async () => {
  stubRoutes({
    "/api/v1/meta": META,
    "/api/v1/settings": SETTINGS(false),
    "/api/v1/oauth/grants": () => json({ items: [GRANT] }),
  });
  renderWithRouter(<ConnectedAppsCard me={ME} />);
  expect(await screen.findByText(/connected apps are paused/)).toBeInTheDocument();
  expect(screen.getByRole("list", { name: "Connected apps" })).toBeInTheDocument();
});

test("an_admin_sees_another_users_apps_by_id", async () => {
  const other = "44444444-4444-4444-8444-444444444444";
  const seen = stubRoutes({
    "/api/v1/oauth/grants": () =>
      json({ items: [{ ...GRANT, user_id: other, scopes: ["content:read"] }] }),
  });
  renderWithRouter(<UserApps userId={other} userName="Grace" />);
  const app = await screen.findByRole("listitem");
  expect(app).toHaveTextContent("Read only · 1 area");
  expect(seen[0]?.searchParams.get("user")).toBe(other);
  await userEvent.click(within(app).getByRole("button", { name: "Disconnect Claude" }));
  expect(await screen.findByRole("alertdialog")).toHaveTextContent(
    "It loses access to Grace's account on this site at once.",
  );
});

test("another_user_without_apps_says_so", async () => {
  stubRoutes({ "/api/v1/oauth/grants": () => json({ items: [] }) });
  renderWithRouter(<UserApps userId="x" userName="Grace" />);
  expect(
    await screen.findByText("This user has not connected any AI assistants."),
  ).toBeInTheDocument();
});
