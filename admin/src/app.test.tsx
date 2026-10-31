import { QueryClient } from "@tanstack/react-query";
import { createMemoryHistory } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { App } from "./app";
import { DensityProvider } from "./design/density";
import { ThemeProvider } from "./design/theme";
import { i18n } from "./i18n";
import { createAppRouter } from "./router";

afterEach(() => {
  vi.unstubAllGlobals();
});

const json = (body: unknown) =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });

const ME = {
  id: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
  login: "alice",
  email: "alice@example.test",
  display_name: "Alice",
  role: "author",
  must_reset: false,
  totp_enabled: false,
  capabilities: ["read_admin", "edit_own"],
  scopes: [],
  csrf_token: "t",
};

function stubApi() {
  // jsdom has no `matchMedia` (the sidebar's mobile check and the theme use it).
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  vi.stubGlobal(
    "fetch",
    vi.fn((r: Request) => {
      const path = new URL(r.url).pathname;
      if (path === "/api/v1/auth/me") return Promise.resolve(json(ME));
      if (path === "/api/v1/settings") return Promise.resolve(json({ "site.title": "Kestrel" }));
      if (path === "/api/v1/types")
        return Promise.resolve(
          json({
            items: [
              {
                name: "post",
                label: "Posts",
                built_in: true,
                hierarchical: false,
                has_archive: true,
              },
            ],
          }),
        );
      if (path === "/api/v1/meta")
        return Promise.resolve(json({ api: "v1", version: "0", site_url: "http://localhost" }));
      return Promise.resolve(new Response(null, { status: 404 }));
    }),
  );
}

function renderApp(queryClient: QueryClient, router: ReturnType<typeof createAppRouter>) {
  render(
    <ThemeProvider>
      <DensityProvider>
        <App queryClient={queryClient} router={router} />
      </DensityProvider>
    </ThemeProvider>,
  );
}

test("renders_the_shell_with_translated_title", async () => {
  // The dashboard sits behind the login guard: a signed-in user.
  stubApi();
  const queryClient = new QueryClient();
  const history = createMemoryHistory({ initialEntries: ["/admin/"] });
  const router = createAppRouter(queryClient, history);

  renderApp(queryClient, router);

  const name = i18n.t("app.name");
  expect(name).toBe("FerriteCMS");
  // The dashboard's route chunk imports Recharts; its first transform in Vitest takes
  // about a second cold and several under load, longer than the default `findBy` timeout.
  // The test's own timeout (below) is longer than this wait, so a slow transform fails
  // here with the query's message, not with a bare test timeout.
  expect(
    await screen.findByRole("heading", { level: 1, name: "Dashboard" }, { timeout: 15_000 }),
  ).toBeInTheDocument();
  expect(screen.getByRole("navigation", { name: "Main" })).toBeInTheDocument();
  expect(await screen.findByRole("link", { name: "Posts" })).toHaveAttribute(
    "href",
    "/admin/content/post",
  );
  await waitFor(() => {
    expect(document.title).toBe(`Dashboard · ${name}`);
  });
}, 20_000);

test("unknown_admin_path_is_not_found_inside_the_shell", async () => {
  stubApi();
  const queryClient = new QueryClient();
  const history = createMemoryHistory({ initialEntries: ["/admin/no/such/screen"] });
  const router = createAppRouter(queryClient, history);

  renderApp(queryClient, router);

  expect(await screen.findByRole("heading", { name: "Page not found" })).toBeInTheDocument();
  expect(screen.getByRole("navigation", { name: "Main" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Go to dashboard" })).toHaveAttribute("href", "/admin/");
});
