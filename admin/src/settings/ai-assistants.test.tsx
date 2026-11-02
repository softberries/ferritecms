import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { SettingsDto } from "@/api/settings";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { AiAssistantsGroup } from "./ai-assistants";

const URL_ = "https://blog.example.test/mcp";

function api(stored: SettingsDto) {
  const patches: SettingsDto[] = [];
  stubRoutes({
    "/api/v1/meta": () =>
      json({ api: "v1", site_url: "https://blog.example.test", version: "0.1.0" }),
    "/api/v1/settings": async (_url, req) => {
      if (req.method === "PATCH") {
        const body = (await req.json()) as SettingsDto;
        patches.push(body);
        return json({ ...stored, ...body });
      }
      return json(stored);
    },
  });
  return patches;
}

test("the_switch_saves_mcp_enabled", async () => {
  const patches = api({ "mcp.enabled": null });
  renderWithRouter(<AiAssistantsGroup settings={{ "mcp.enabled": null }} />);
  const toggle = await screen.findByRole("switch", { name: "Allow AI assistants" });
  // Unset is the default: on.
  expect(toggle).toBeChecked();
  expect(toggle).toHaveAccessibleDescription(/Connected apps are paused, not removed/);
  await userEvent.click(toggle);
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(patches).toEqual([{ "mcp.enabled": false }]));
});

test("off_says_assistants_cannot_connect", async () => {
  api({ "mcp.enabled": false });
  renderWithRouter(<AiAssistantsGroup settings={{ "mcp.enabled": false }} />);
  expect(await screen.findByRole("switch", { name: "Allow AI assistants" })).not.toBeChecked();
  expect(screen.getByText(/AI assistants are turned off/)).toBeInTheDocument();
});

test("copy_puts_the_mcp_url_on_the_clipboard", async () => {
  const user = userEvent.setup();
  api({ "mcp.enabled": true });
  renderWithRouter(<AiAssistantsGroup settings={{ "mcp.enabled": true }} />);
  const field = await screen.findByRole("textbox", { name: "MCP URL" });
  expect(field).toHaveValue(URL_);
  await user.click(screen.getByRole("button", { name: "Copy MCP URL" }));
  expect(await navigator.clipboard.readText()).toBe(URL_);
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
  expect(screen.getByRole("status")).toHaveTextContent("Copied to the clipboard.");
});

test("client_tabs_show_each_clients_steps", async () => {
  const user = userEvent.setup();
  api({ "mcp.enabled": true });
  renderWithRouter(<AiAssistantsGroup settings={{ "mcp.enabled": true }} />);
  const tabs = await screen.findByRole("tablist", { name: "Clients" });
  expect(
    within(tabs)
      .getAllByRole("tab")
      .map((t) => t.textContent),
  ).toEqual(["Claude", "ChatGPT", "Claude Code", "Cursor", "VS Code"]);
  const claude = screen.getByRole("tabpanel");
  expect(claude).toHaveTextContent(
    "Add a custom connector (in Claude's settings, under Connectors)",
  );
  expect(claude).toHaveTextContent("must be reachable over public HTTPS");

  await user.click(within(tabs).getByRole("tab", { name: "Claude Code" }));
  const code = screen.getByRole("tabpanel");
  expect(within(code).getByRole("region", { name: "Command" })).toHaveTextContent(
    `claude mcp add --transport http ferrite ${URL_}`,
  );
  await user.click(within(code).getByRole("button", { name: "Copy Command with a token" }));
  expect(await navigator.clipboard.readText()).toBe(
    `claude mcp add --transport http ferrite ${URL_} --header "Authorization: Bearer <token>"`,
  );

  expect(code).toHaveTextContent("run /mcp in Claude Code, pick ferrite and choose Authenticate");

  await user.click(within(tabs).getByRole("tab", { name: "ChatGPT" }));
  expect(screen.getByRole("tabpanel")).toHaveTextContent("turn on developer mode");
  expect(screen.getByRole("tabpanel")).toHaveTextContent("public HTTPS");

  await user.click(within(tabs).getByRole("tab", { name: "Cursor" }));
  expect(screen.getByRole("tabpanel")).toHaveTextContent('"mcpServers"');
  expect(screen.getByRole("tabpanel")).toHaveTextContent(
    "a browser window opens; sign in and approve",
  );
  await user.click(within(tabs).getByRole("tab", { name: "VS Code" }));
  expect(screen.getByRole("tabpanel")).toHaveTextContent('"type": "http"');

  expect(screen.getByRole("link", { name: /Full guide/ })).toHaveAttribute(
    "href",
    "https://softberries.github.io/ferritecms/guides/ai-assistants/",
  );
  expect(screen.getByText(/No assistant can install or remove plugins/)).toBeInTheDocument();
});
