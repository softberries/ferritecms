import { QueryClient } from "@tanstack/react-query";
import { createMemoryHistory } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { App } from "./app";
import { i18n } from "./i18n";
import { createAppRouter } from "./router";

afterEach(() => {
  vi.unstubAllGlobals();
});

test("renders_root_with_translated_title", async () => {
  // The dashboard sits behind the login guard: a signed-in user.
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            id: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
            login: "alice",
            email: "alice@example.test",
            display_name: "Alice",
            role: "admin",
            must_reset: false,
            totp_enabled: false,
            capabilities: [],
            scopes: [],
            csrf_token: "t",
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      ),
    ),
  );
  const queryClient = new QueryClient();
  const history = createMemoryHistory({ initialEntries: ["/admin/"] });
  const router = createAppRouter(queryClient, history);

  render(<App queryClient={queryClient} router={router} />);

  const name = i18n.t("app.name");
  expect(name).toBe("FerriteCMS");
  expect(await screen.findByRole("heading", { level: 1, name })).toBeInTheDocument();
  await waitFor(() => {
    expect(document.title).toBe(name);
  });
});
