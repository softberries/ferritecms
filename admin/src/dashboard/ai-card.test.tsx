import { screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { AiCard } from "./ai-card";

const ME = "11111111-1111-4111-8111-111111111111";
const GRANT = {
  id: "33333333-3333-4333-8333-333333333333",
  user_id: ME,
  client: { id: "ferc_x", kind: "registered", name: "Cursor", client_uri: null, logo_uri: null },
  scopes: ["content:read"],
  created_at: "2026-10-30T10:00:00Z",
  last_used_at: "2026-11-01T10:00:00Z",
  expires_at: "2026-12-01T10:00:00Z",
};

function api(grants: unknown[], enabled: boolean | null = null) {
  stubRoutes({
    "/api/v1/meta": () =>
      json({ api: "v1", site_url: "https://blog.example.test", version: "0.1.0" }),
    "/api/v1/settings": () => json({ "mcp.enabled": enabled }),
    "/api/v1/oauth/grants": () => json({ items: grants }),
  });
}

test("before_any_grant_it_invites_with_the_url", async () => {
  api([]);
  renderWithRouter(<AiCard me={ME} canSettings />);
  expect(
    await screen.findByRole("heading", { name: "Manage this site with AI" }),
  ).toBeInTheDocument();
  expect(await screen.findByRole("textbox", { name: "MCP URL" })).toHaveValue(
    "https://blog.example.test/mcp",
  );
  expect(screen.getByRole("link", { name: "Set up" })).toHaveAttribute(
    "href",
    "/admin/settings/ai-assistants",
  );
  expect(screen.getByRole("link", { name: "How it works" })).toHaveAttribute(
    "href",
    "https://softberries.github.io/ferritecms/guides/ai-assistants/",
  );
});

test("without_manage_settings_there_is_no_setup_link", async () => {
  api([]);
  renderWithRouter(<AiCard me={ME} canSettings={false} />);
  expect(await screen.findByRole("textbox", { name: "MCP URL" })).toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Set up" })).toBeNull();
});

test("with_grants_it_counts_them_and_links_to_account", async () => {
  api([GRANT, { ...GRANT, id: "x" }]);
  renderWithRouter(<AiCard me={ME} canSettings />);
  expect(await screen.findByText("2 connected apps")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Manage" })).toHaveAttribute("href", "/admin/account");
  expect(screen.queryByRole("textbox", { name: "MCP URL" })).toBeNull();
});

test("switched_off_it_says_so", async () => {
  api([GRANT], false);
  renderWithRouter(<AiCard me={ME} canSettings />);
  expect(await screen.findByRole("heading", { name: "AI assistants are off" })).toBeVisible();
  expect(screen.getByRole("link", { name: "Turn on" })).toHaveAttribute(
    "href",
    "/admin/settings/ai-assistants",
  );
});
