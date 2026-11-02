// Connecting an AI assistant under the real server (M5 Task 9; 12 §3.2, §8): a client
// registers itself (`POST /oauth/register`), the browser opens `/oauth/authorize` with
// PKCE, signs in, sees the consent screen with the client's name, narrows it to read
// only and approves; the browser lands on the client's loopback redirect URI (served by
// the test) with `code`, `state` and `iss`, and the code buys a read-only token. Then
// Account → Connected apps lists the app and Disconnect removes it. Also: Deny, the
// expired request, Settings → AI assistants, the dashboard card and the palette entry,
// with axe and review screens. Each test makes its own author (`uid()`); contexts use
// spec number 31 (`forwardedFor(31, n)`, F24), so the OAuth limits per address are
// this spec's own.

import { createHash, randomBytes } from "node:crypto";
import {
  request as apiRequest,
  type Browser,
  type BrowserContext,
  type Page,
} from "@playwright/test";
import { expect, test, type Guard } from "./support/fixtures";
import { authFile, type SiteInfo } from "./support/server";
import { VIEWPORTS } from "./support/snap";
import { uid } from "./support/uid";
import { mod } from "./support/keys";
import { forwardedFor } from "./support/users";

const SPEC = 31;
// A signed-out page's `GET /auth/me` answers 401 before the sign-in, which the browser logs.
const SIGNED_OUT = /Failed to load resource: the server responded with a status of 401/;
/** A loopback redirect URI (RFC 8252); nothing listens there, the test answers it. */
const REDIRECT = "http://127.0.0.1:47123/callback";

interface User {
  id: string;
  login: string;
  password: string;
}

const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  await Promise.all(contexts.splice(0).map((c) => c.close()));
});

/** A new author with a known password, made by the admin over the API. */
async function newAuthor(site: SiteInfo, siteKey: string): Promise<User> {
  const ctx = await apiRequest.newContext({
    baseURL: site.url,
    storageState: authFile(siteKey, "admin"),
    extraHTTPHeaders: { Origin: site.url },
  });
  try {
    const me = (await (await ctx.get("/api/v1/auth/me")).json()) as { csrf_token: string };
    const login = uid("mcp").replace(/[^a-z0-9-]/g, "");
    const password = `e2e-mcp-${randomBytes(6).toString("hex")}`;
    const res = await ctx.post("/api/v1/users", {
      headers: { "X-CSRF-Token": me.csrf_token },
      data: {
        login,
        email: `${login}@example.test`,
        display_name: "MCP Tester",
        role: "author",
        password,
        must_reset: false,
      },
    });
    expect(res.status(), await res.text()).toBe(201);
    return { id: ((await res.json()) as { id: string }).id, login, password };
  } finally {
    await ctx.dispose();
  }
}

/** A context of its own (address `n`), with no session. */
async function newContext(
  browser: Browser,
  site: SiteInfo,
  guard: Guard,
  n: number,
): Promise<BrowserContext> {
  const ctx = await browser.newContext({
    baseURL: site.url,
    viewport: VIEWPORTS.desktop,
    colorScheme: "light",
    extraHTTPHeaders: { "X-Forwarded-For": forwardedFor(SPEC, n) },
  });
  contexts.push(ctx);
  await guard.watch(ctx);
  return ctx;
}

const b64url = (buf: Buffer) => buf.toString("base64url");

interface Flow {
  clientId: string;
  name: string;
  verifier: string;
  state: string;
  /** `/oauth/authorize?…` for this client. */
  authorize: string;
}

/** Registers a client as an MCP client would and prepares its PKCE request. */
async function register(ctx: BrowserContext): Promise<Flow> {
  const name = `E2E Assistant ${randomBytes(3).toString("hex")}`;
  const res = await ctx.request.post("/oauth/register", {
    data: { client_name: name, redirect_uris: [REDIRECT] },
  });
  expect(res.status(), await res.text()).toBe(201);
  const clientId = ((await res.json()) as { client_id: string }).client_id;
  const verifier = b64url(randomBytes(32));
  const challenge = b64url(createHash("sha256").update(verifier).digest());
  const state = uid("state");
  const query = new URLSearchParams({
    response_type: "code",
    client_id: clientId,
    redirect_uri: REDIRECT,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state,
  });
  return { clientId, name, verifier, state, authorize: `/oauth/authorize?${query.toString()}` };
}

/** Answers the redirect URI with a plain page; resolves the URL the browser landed on. */
async function catchRedirect(page: Page): Promise<() => URL | null> {
  let landed: URL | null = null;
  await page.route(`${REDIRECT}**`, async (route) => {
    landed = new URL(route.request().url());
    await route.fulfill({
      status: 200,
      contentType: "text/html",
      body: "<!doctype html><html lang=en><title>Assistant</title><p>Back in the app.</p>",
    });
  });
  return () => landed;
}

async function signIn(page: Page, user: User): Promise<void> {
  await expect(page).toHaveURL(/\/admin\/login\?next=/);
  await page.getByLabel("Login or e-mail").fill(user.login);
  await page.getByLabel("Password", { exact: true }).fill(user.password);
  await page.getByRole("button", { name: "Sign in" }).click();
}

/** Before a snap: no focused control, the page at its top. */
async function atRest(page: Page): Promise<void> {
  await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
    window.scrollTo(0, 0);
  });
}

const permission = (page: Page, area: string) =>
  page.getByRole("radiogroup", { name: area, exact: true });

test("connect_narrow_approve_list_and_disconnect", async ({
  browser,
  site,
  siteKey,
  guard,
  snap,
  expectAccessible,
}) => {
  guard.allow(SIGNED_OUT);
  const user = await newAuthor(site, siteKey);
  const ctx = await newContext(browser, site, guard, 1);
  const flow = await register(ctx);
  const page = await ctx.newPage();
  const landed = await catchRedirect(page);

  // Signed out: `/oauth/authorize` → consent → sign in → back to the consent screen.
  await page.goto(flow.authorize);
  await signIn(page, user);
  await expect(page).toHaveURL(/\/admin\/connect\?request=[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { level: 1, name: `Connect ${flow.name}` })).toBeVisible();
  await expect(page.getByText("Self-registered")).toBeVisible();
  await expect(page.locator("[data-slot=consent-redirect]")).toHaveText("127.0.0.1:47123");
  // An author can write content: preselected as asked (all areas, within the role).
  await expect(permission(page, "Content").getByRole("radio", { name: "Write" })).toBeChecked();
  await expectAccessible(page);
  await atRest(page);
  await snap(page, "mcp-consent");

  // Narrow to read only, then approve: the browser goes to the redirect URI.
  await page.getByRole("button", { name: "Read only" }).click();
  await expect(permission(page, "Content").getByRole("radio", { name: "Read" })).toBeChecked();
  const approved = page.waitForRequest((r) => r.url().startsWith(REDIRECT));
  await page.getByRole("button", { name: "Approve" }).click();
  await approved;
  await expect(page).toHaveURL(/^http:\/\/127\.0\.0\.1:47123\/callback\?/);
  const back = landed();
  expect(back).not.toBeNull();
  const code = back?.searchParams.get("code") ?? "";
  expect(code).not.toBe("");
  expect(back?.searchParams.get("state")).toBe(flow.state);
  expect(back?.searchParams.get("iss")).toBe(site.url.replace(/\/$/, ""));

  // The code buys a token whose scopes are read-only.
  const token = await ctx.request.post("/oauth/token", {
    form: {
      grant_type: "authorization_code",
      code,
      redirect_uri: REDIRECT,
      client_id: flow.clientId,
      code_verifier: flow.verifier,
    },
  });
  expect(token.status(), await token.text()).toBe(200);
  const scopes = ((await token.json()) as { scope: string }).scope.split(" ");
  expect(scopes.length).toBeGreaterThan(0);
  expect(scopes.every((s) => s.endsWith(":read"))).toBe(true);

  // Account → Connected apps lists it, read only; Disconnect removes it.
  await page.goto("/admin/account");
  const apps = page.getByRole("region", { name: "Connected apps", exact: true });
  const row = apps.locator("[data-slot=connected-app]").filter({ hasText: flow.name });
  await expect(row).toBeVisible();
  await expect(row).toContainText("Read only");
  await expectAccessible(page);
  await atRest(page);
  await snap(page, "mcp-connected-apps");
  await row.getByRole("button", { name: `Disconnect ${flow.name}` }).click();
  const confirm = page.getByRole("alertdialog", { name: `Disconnect ${flow.name}?` });
  await confirm.getByRole("button", { name: "Disconnect" }).click();
  await expect(confirm).toBeHidden();
  await expect(row).toHaveCount(0);
  await expect(apps.getByText("No connected apps")).toBeVisible();
  const grants = await ctx.request.get("/api/v1/oauth/grants");
  expect(((await grants.json()) as { items: unknown[] }).items).toEqual([]);
});

test("deny_goes_back_with_access_denied_and_the_request_is_spent", async ({
  browser,
  site,
  siteKey,
  guard,
}) => {
  // The spent request's `GET` answers 404, which the browser logs.
  guard.allowFailedResource(/\/api\/v1\/oauth\/requests\//);
  guard.allow(SIGNED_OUT);
  const user = await newAuthor(site, siteKey);
  const ctx = await newContext(browser, site, guard, 2);
  const flow = await register(ctx);
  const page = await ctx.newPage();
  const landed = await catchRedirect(page);
  await page.goto(flow.authorize);
  await signIn(page, user);
  await expect(page.getByRole("heading", { level: 1, name: `Connect ${flow.name}` })).toBeVisible();
  const consent = page.url();
  await page.getByRole("button", { name: "Deny" }).click();
  await expect(page).toHaveURL(/^http:\/\/127\.0\.0\.1:47123\/callback\?/);
  expect(landed()?.searchParams.get("error")).toBe("access_denied");
  expect(landed()?.searchParams.get("state")).toBe(flow.state);

  // The request works once: opening it again says it has expired.
  await page.goto(consent);
  await expect(
    page.getByRole("heading", { level: 1, name: "This request has expired" }),
  ).toBeVisible();
});

test("ai_settings_dashboard_card_and_palette", async ({ as, site, snap, expectAccessible }) => {
  const page = await as("admin");
  await page.goto("/admin/settings/ai-assistants");
  const group = page.locator("[data-slot=settings-group]");
  await expect(group).toHaveAttribute("data-group", "ai-assistants");
  await expect(page.getByRole("switch", { name: "Allow AI assistants" })).toBeChecked();
  await expect(page.getByRole("textbox", { name: "MCP URL" })).toHaveValue(
    `${site.url.replace(/\/$/, "")}/mcp`,
  );
  await page.getByRole("tab", { name: "Claude Code" }).click();
  await expect(page.getByRole("region", { name: "Command", exact: true })).toContainText(
    "claude mcp add --transport http ferrite",
  );
  await page.getByRole("tab", { name: "Claude", exact: true }).click();
  await expectAccessible(page);
  await atRest(page);
  await snap(page, "mcp-settings");

  // The palette goes to the same group.
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
  await page.keyboard.press(`${await mod(page)}+k`);
  const field = page.locator("[data-slot=palette-input]");
  await expect(field).toBeFocused();
  await field.fill("connect an ai");
  await page.getByRole("option", { name: "Connect an AI assistant" }).click();
  await expect(page).toHaveURL(/\/admin\/settings\/ai-assistants$/);
});

test("dashboard_card_invites_an_author_without_apps", async ({
  browser,
  site,
  siteKey,
  guard,
  snap,
  expectAccessible,
}) => {
  const user = await newAuthor(site, siteKey);
  const ctx = await newContext(browser, site, guard, 3);
  const res = await ctx.request.post("/api/v1/auth/login", {
    headers: { Origin: site.url },
    data: { login: user.login, password: user.password },
  });
  expect(res.status()).toBe(200);
  const page = await ctx.newPage();
  await page.goto("/admin/");
  const card = page.getByRole("region", { name: "Manage this site with AI" });
  await expect(card.getByRole("textbox", { name: "MCP URL" })).toHaveValue(
    `${site.url.replace(/\/$/, "")}/mcp`,
  );
  // An author does not manage settings: the guide, not the settings group.
  await expect(card.getByRole("link", { name: "Set up" })).toHaveCount(0);
  await expect(card.getByRole("link", { name: "How it works" })).toBeVisible();
  await expectAccessible(page);
  await atRest(page);
  await snap(page, "mcp-dashboard-card");
});
