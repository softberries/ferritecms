import { createMemoryHistory } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createQueryClient } from "@/api/client";
import { getCsrfToken, setCsrfToken } from "@/api/csrf";
import { App } from "@/app";
import { createAppRouter } from "@/router";

const PROBLEM = "https://ferritecms.dev/problems/";

function problem(status: number, slug: string, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify({ type: PROBLEM + slug, title: slug, status, detail: "" }), {
    status,
    headers: { "Content-Type": "application/problem+json", ...headers },
  });
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

const META = { api: "v1", version: "0.1.0", site_url: "https://blog.example" };
const ME = {
  id: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
  login: "alice",
  email: "alice@example.test",
  display_name: "Alice",
  role: "editor",
  must_reset: false,
  totp_enabled: false,
  capabilities: ["read_admin"],
  scopes: [],
  csrf_token: "csrf-1",
};

type Handler = (req: Request) => Response | Promise<Response>;

/** Answers `GET /meta` itself; every other request goes to the next queued handler. */
function stubApi(queue: Handler[]) {
  const seen: Request[] = [];
  const fetch = vi.fn(async (req: Request) => {
    const path = new URL(req.url).pathname;
    if (path === "/api/v1/meta") return json(META);
    seen.push(req.clone());
    const next = queue.shift();
    if (!next) throw new Error(`unexpected ${req.method} ${path}`);
    return next(req);
  });
  vi.stubGlobal("fetch", fetch);
  return seen;
}

/** Every client a test made: its queries are cancelled afterwards, so a retry of one
 * test's request never reaches the next test's stubbed `fetch`. */
const clients: QueryClient[] = [];

function renderAt(path: string) {
  const queryClient = createQueryClient({
    navigate: () => undefined,
    currentPath: () => path,
    notify: () => undefined,
  });
  const history = createMemoryHistory({ initialEntries: [path] });
  const router = createAppRouter(queryClient, history);
  clients.push(queryClient);
  render(<App queryClient={queryClient} router={router} />);
  return { router, history, queryClient };
}

beforeEach(() => {
  setCsrfToken(null);
});

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.cancelQueries()));
  vi.unstubAllGlobals();
});

describe("login_maps_problems", () => {
  it("invalid credentials, a rate-limit countdown, then the TOTP step", async () => {
    const user = userEvent.setup();
    const seen = stubApi([
      () => problem(401, "invalid-credentials"),
      () => problem(429, "rate-limited", { "Retry-After": "2" }),
      () => json({ status: "totp_required", challenge: "sealed-challenge" }),
    ]);
    const { router } = renderAt("/admin/login?next=%2Fadmin%2Fcontent%2Fpost");

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
    // The site's host from GET /meta (F22).
    expect(await screen.findByTitle("blog.example", { exact: true })).toBeInTheDocument();

    const submit = screen.getByRole("button", { name: "Sign in" });
    await user.type(screen.getByLabelText("Login or e-mail"), "alice");
    await user.type(screen.getByLabelText("Password"), "wrong-password-1");
    await user.click(submit);

    // One generic message; the password is cleared for the next try.
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "That login and password don't match an account.",
    );
    expect(screen.getByLabelText("Password")).toHaveValue("");
    expect(screen.getByLabelText("Password")).toHaveFocus();
    const first = seen[0];
    expect(first?.method).toBe("POST");
    expect(await first?.json()).toEqual({ login: "alice", password: "wrong-password-1" });

    // 429: a countdown from Retry-After; the form stays locked until it ends.
    await user.type(screen.getByLabelText("Password"), "another-password");
    await user.click(submit);
    expect(await screen.findByText("Too many attempts, try again in 2 s")).toBeVisible();
    expect(screen.getByRole("button", { name: "Sign in" })).toBeDisabled();
    expect(await screen.findByText("Too many attempts, try again in 1 s")).toBeVisible();
    await waitFor(
      () => {
        expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      },
      { timeout: 3000 },
    );
    expect(screen.getByRole("button", { name: "Sign in" })).toBeEnabled();

    // totp_required: on to /login/totp, the challenge in history state, not the URL.
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(
      await screen.findByRole("heading", { level: 1, name: "Two-factor code" }),
    ).toBeInTheDocument();
    const location = router.state.location;
    expect(location.pathname).toBe("/login/totp");
    expect(location.href).not.toContain("sealed-challenge");
    expect(location.state.totpChallenge).toBe("sealed-challenge");
    expect(location.search).toEqual({ next: "/admin/content/post" });
    expect(screen.getByLabelText("Authentication code")).toHaveAttribute(
      "autocomplete",
      "one-time-code",
    );
  }, 10_000);

  it("empty fields are named and nothing is sent", async () => {
    const user = userEvent.setup();
    const seen = stubApi([]);
    renderAt("/admin/login");
    await user.click(await screen.findByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("Enter your login or e-mail.")).toBeVisible();
    expect(screen.getByText("Enter your password.")).toBeVisible();
    expect(screen.getByLabelText("Login or e-mail")).toHaveFocus();
    expect(screen.getByLabelText("Login or e-mail")).toHaveAttribute("aria-invalid", "true");
    expect(seen).toEqual([]);
  });

  it("a session goes to next and keeps the CSRF token", async () => {
    const user = userEvent.setup();
    stubApi([() => json({ status: "session", user: ME, csrf_token: "csrf-1" })]);
    const { router } = renderAt("/admin/login?next=%2Fadmin%2F%3Fx%3D1");
    await user.type(await screen.findByLabelText("Login or e-mail"), "alice");
    await user.type(screen.getByLabelText("Password"), "correct-horse-battery");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    await waitFor(() => {
      expect(router.state.location.href).toBe("/?x=1");
    });
    expect(getCsrfToken()).toBe("csrf-1");
  });

  it.each(["%2F%2Fevil.example", "%2Fadmin%2F%2Fevil.example", "/admin//evil.example"])(
    "an open-redirect next (%s) lands on the dashboard",
    async (next) => {
      const user = userEvent.setup();
      stubApi([() => json({ status: "session", user: ME, csrf_token: "csrf-1" })]);
      const { router } = renderAt(`/admin/login?next=${next}`);
      await user.type(await screen.findByLabelText("Login or e-mail"), "alice");
      await user.type(screen.getByLabelText("Password"), "correct-horse-battery");
      await user.click(screen.getByRole("button", { name: "Sign in" }));
      // Landing on the dashboard loads its route chunk (Recharts), slow on first import.
      await waitFor(
        () => {
          expect(router.state.location.pathname).toBe("/");
        },
        { timeout: 5000 },
      );
      expect(router.state.location.href).toBe("/");
    },
  );
});

describe("ensure_session_guard", () => {
  it("401 sends to login with the page as next", async () => {
    stubApi([() => problem(401, "unauthenticated")]);
    const { router } = renderAt("/admin/_styleguide?section=colour");
    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
    expect(router.state.location.pathname).toBe("/login");
    expect(router.state.location.search).toEqual({ next: "/admin/_styleguide?section=colour" });
  });

  it("must_reset is confined to change-password", async () => {
    stubApi([() => json({ ...ME, must_reset: true })]);
    const { router } = renderAt("/admin/");
    expect(
      await screen.findByRole("heading", { level: 1, name: "Choose a new password" }),
    ).toBeInTheDocument();
    expect(router.state.location.pathname).toBe("/change-password");
    // The guard waited for the token before letting anything write.
    expect(getCsrfToken()).toBe("csrf-1");
  });
});
