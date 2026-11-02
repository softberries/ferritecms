import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import type { ConsentDto } from "@/api/oauth";
import type { MeDto } from "@/auth/session";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ConsentScreen } from "./consent-screen";

const ME = {
  id: "11111111-1111-4111-8111-111111111111",
  login: "ada",
  email: "ada@example.test",
  display_name: "Ada",
  role: "author",
  must_reset: false,
  totp_enabled: false,
  capabilities: ["read_admin", "edit_own"],
  scopes: ["content:write"],
  csrf_token: "csrf",
} satisfies MeDto;

const ID = "22222222-2222-4222-8222-222222222222";
const BASE = `/api/v1/oauth/requests/${ID}`;

function consent(over: Partial<ConsentDto> = {}): ConsentDto {
  return {
    id: ID,
    client: {
      id: "ferc_abcdefghijklmnopqrstuv",
      kind: "registered",
      name: "Claude Code",
      client_uri: null,
      logo_uri: null,
    },
    verified: false,
    redirect_host: "127.0.0.1",
    requested: ["content:write", "media:write", "system:read"],
    grantable: ["content:write", "media:read"],
    ...over,
  };
}

const META = () => json({ api: "v1", site_url: "https://blog.example.test", version: "0.1.0" });

function api(body: ConsentDto | Response, decisions: { url: string; body: unknown }[] = []) {
  stubRoutes({
    "/api/v1/meta": META,
    // Like the server: a decided request is gone.
    [BASE]: () => (body instanceof Response || decisions.length > 0 ? notFound() : json(body)),
    [`${BASE}/approve`]: async (url, req) => {
      decisions.push({ url: url.pathname, body: await req.json() });
      return json({ redirect_to: "http://127.0.0.1:4567/cb?code=c0de&state=s1&iss=x" });
    },
    [`${BASE}/deny`]: (url) => {
      decisions.push({ url: url.pathname, body: null });
      return json({ redirect_to: "http://127.0.0.1:4567/cb?error=access_denied&state=s1" });
    },
  });
  return decisions;
}

function notFound(): Response {
  return new Response(
    JSON.stringify({ type: "about:blank", title: "Not found", status: 404, code: "not-found" }),
    { status: 404, headers: { "Content-Type": "application/problem+json" } },
  );
}

const rows = () => screen.getAllByRole("radiogroup");
const row = (name: string) => screen.getByRole("radiogroup", { name });

test("shows_the_client_its_return_host_and_grantable_areas", async () => {
  api(consent());
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={vi.fn()} />);
  expect(await screen.findByRole("heading", { name: "Connect Claude Code" })).toBeInTheDocument();
  expect(await screen.findByText(/asking to manage blog\.example\.test/)).toBeInTheDocument();
  expect(screen.getByText("Self-registered")).toBeInTheDocument();
  expect(screen.getByText(/This app named itself/)).toBeInTheDocument();
  expect(screen.getByText("127.0.0.1")).toBeInTheDocument();
  // One row per grantable area, preselected at its most; no write above grantable.
  expect(rows().map((g) => g.getAttribute("aria-labelledby"))).toHaveLength(2);
  expect(within(row("Content")).getByRole("radio", { name: "Write" })).toBeChecked();
  expect(within(row("Media")).getByRole("radio", { name: "Read" })).toBeChecked();
  expect(within(row("Media")).queryByRole("radio", { name: "Write" })).toBeNull();
  // System was asked for but the role cannot give it.
  expect(screen.getByText(/also asked for System, which your role does not allow/)).toBeVisible();
  expect(screen.getByText(/acts as you, Ada, with your role \(Author\)/)).toBeInTheDocument();
  expect(screen.getByText("Signed in as ada.")).toBeInTheDocument();
  // No monogram image for a client without a logo.
  expect(document.querySelector("[data-slot=client-logo] img")).toBeNull();
});

test("a_verified_client_shows_its_https_logo_and_origin", async () => {
  api(
    consent({
      verified: true,
      client: {
        id: "https://claude.ai/oauth/claude-code-client-metadata",
        kind: "metadata",
        name: "Claude",
        client_uri: "https://claude.ai",
        logo_uri: "https://claude.ai/logo.png",
      },
    }),
  );
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={vi.fn()} />);
  expect(await screen.findByText("Verified")).toBeInTheDocument();
  expect(screen.getByText("Its details come from claude.ai.")).toBeInTheDocument();
  expect(screen.queryByText(/This app named itself/)).toBeNull();
  const img = document.querySelector("[data-slot=client-logo] img");
  expect(img).toHaveAttribute("src", "https://claude.ai/logo.png");
  expect(img).toHaveAttribute("referrerpolicy", "no-referrer");
});

test("narrowed_approve_posts_the_chosen_scopes_and_navigates", async () => {
  const decisions = api(consent());
  const redirect = vi.fn();
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={redirect} />);
  await screen.findByRole("heading", { name: "Connect Claude Code" });
  await userEvent.click(within(row("Content")).getByRole("radio", { name: "Read" }));
  await userEvent.click(within(row("Media")).getByRole("radio", { name: "Off" }));
  expect(row("Media")).toHaveAccessibleDescription("Not allowed.");
  await userEvent.click(screen.getByRole("button", { name: "Approve" }));
  await waitFor(() => expect(redirect).toHaveBeenCalledOnce());
  expect(decisions).toEqual([{ url: `${BASE}/approve`, body: { scopes: ["content:read"] } }]);
  expect(redirect).toHaveBeenCalledWith("http://127.0.0.1:4567/cb?code=c0de&state=s1&iss=x");
  expect(await screen.findByRole("heading", { name: "Connected" })).toBeInTheDocument();
});

test("read_only_and_as_requested_set_every_row", async () => {
  const decisions = api(consent({ grantable: ["content:write", "media:write"] }));
  const redirect = vi.fn();
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={redirect} />);
  await screen.findByRole("heading", { name: "Connect Claude Code" });
  await userEvent.click(screen.getByRole("button", { name: "Read only" }));
  for (const name of ["Content", "Media"]) {
    expect(within(row(name)).getByRole("radio", { name: "Read" })).toBeChecked();
  }
  await userEvent.click(screen.getByRole("button", { name: "As requested" }));
  expect(within(row("Media")).getByRole("radio", { name: "Write" })).toBeChecked();
  await userEvent.click(screen.getByRole("button", { name: "Read only" }));
  await userEvent.click(screen.getByRole("button", { name: "Approve" }));
  await waitFor(() => expect(redirect).toHaveBeenCalledOnce());
  expect(decisions[0]?.body).toEqual({ scopes: ["content:read", "media:read"] });
});

test("approve_is_off_with_nothing_chosen", async () => {
  api(consent({ grantable: ["content:read"] }));
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={vi.fn()} />);
  await screen.findByRole("heading", { name: "Connect Claude Code" });
  await userEvent.click(within(row("Content")).getByRole("radio", { name: "Off" }));
  const approve = screen.getByRole("button", { name: "Approve" });
  expect(approve).toBeDisabled();
  expect(approve).toHaveAccessibleDescription("Choose at least one permission.");
});

test("enter_on_a_permission_does_not_approve", async () => {
  const decisions = api(consent());
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={vi.fn()} />);
  await screen.findByRole("heading", { name: "Connect Claude Code" });
  within(row("Content")).getByRole("radio", { name: "Write" }).focus();
  await userEvent.keyboard("{Enter}");
  await userEvent.keyboard("{Escape}");
  expect(decisions).toEqual([]);
  expect(screen.getByRole("heading", { name: "Connect Claude Code" })).toBeInTheDocument();
});

test("deny_navigates_back_with_the_refusal", async () => {
  const decisions = api(consent());
  const redirect = vi.fn();
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={redirect} />);
  await screen.findByRole("heading", { name: "Connect Claude Code" });
  await userEvent.click(screen.getByRole("button", { name: "Deny" }));
  await waitFor(() =>
    expect(redirect).toHaveBeenCalledWith("http://127.0.0.1:4567/cb?error=access_denied&state=s1"),
  );
  expect(decisions).toEqual([{ url: `${BASE}/deny`, body: null }]);
  expect(await screen.findByRole("heading", { name: "Not connected" })).toBeInTheDocument();
});

test("an_unknown_or_expired_request_says_so", async () => {
  api(notFound());
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={vi.fn()} />);
  expect(
    await screen.findByRole("heading", { name: "This request has expired" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Go to the dashboard" })).toHaveAttribute(
    "href",
    "/admin/",
  );
});

test("a_request_used_meanwhile_turns_into_expired_on_approve", async () => {
  stubRoutes({
    "/api/v1/meta": META,
    [BASE]: () => json(consent()),
    [`${BASE}/approve`]: () => notFound(),
  });
  const redirect = vi.fn();
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={redirect} />);
  await userEvent.click(await screen.findByRole("button", { name: "Approve" }));
  expect(
    await screen.findByRole("heading", { name: "This request has expired" }),
  ).toBeInTheDocument();
  expect(redirect).not.toHaveBeenCalled();
});

test("nothing_grantable_cannot_be_approved", async () => {
  api(consent({ grantable: [] }));
  renderWithRouter(<ConsentScreen requestId={ID} me={ME} redirect={vi.fn()} />);
  expect(await screen.findByText(/Your role allows none of what this app asked for/)).toBeVisible();
  expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Deny" })).toBeEnabled();
});

test("without_a_request_the_page_explains_itself", async () => {
  stubRoutes({ "/api/v1/meta": META });
  renderWithRouter(<ConsentScreen requestId={undefined} me={ME} redirect={vi.fn()} />);
  expect(await screen.findByRole("heading", { name: "No connection request" })).toBeInTheDocument();
});
