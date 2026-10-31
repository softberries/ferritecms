// The sign-in screens under the real server: password and TOTP login, logout, the
// forced password change (Review Focus 4), `next` (Review Focus 5) and the reset link's
// token (Review Focus 1), and a second account signing in on a tab whose session ended
// seeing nothing of the first (the query cache is cleared on a change of user). Login contexts of this file use spec number 9
// (`forwardedFor(9, n)`, F24); every test signs in with its own account or a fresh
// session, never a shared storage state that it then ends (F23).

import { randomBytes } from "node:crypto";
import {
  request as apiRequest,
  type Browser,
  type BrowserContext,
  type Page,
  type Route,
} from "@playwright/test";
import { keptState } from "./support/app-state";
import { ContentApi } from "./support/content-api";
import { expect, test, type Guard } from "./support/fixtures";
import { authFile, type SiteInfo } from "./support/server";
import { resetLink } from "./support/smtp-sink";
import { VIEWPORTS } from "./support/snap";
import { nextFreshStep, stepAt, totpCode } from "./support/totp";
import { uid } from "./support/uid";
import { account, forwardedFor } from "./support/users";

const SPEC = 9;
// The browser logs every non-2xx fetch as a console error; these tests expect some.
const EXPECTED_FAILURES =
  /Failed to load resource: the server responded with a status of (401|403|422|429|503)/;
const PROBLEM = "https://ferritecms.dev/problems/";

const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  await Promise.all(contexts.splice(0).map((c) => c.close()));
});

/** A page with no session, from its own forwarded address (closed after the test). */
async function anonymousPage(
  browser: Browser,
  site: SiteInfo,
  guard: Guard,
  n: number,
): Promise<Page> {
  const ctx = await browser.newContext({
    baseURL: site.url,
    viewport: VIEWPORTS.desktop,
    colorScheme: "light",
    extraHTTPHeaders: { "X-Forwarded-For": forwardedFor(SPEC, n) },
  });
  contexts.push(ctx);
  await guard.watch(ctx);
  return ctx.newPage();
}

async function signIn(page: Page, login: string, password: string): Promise<void> {
  await page.getByLabel("Login or e-mail").fill(login);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
}

async function expectDashboard(page: Page, site: SiteInfo): Promise<void> {
  await expect(page).toHaveURL(`${site.url}/admin/`);
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
}

async function expectNoHorizontalScroll(page: Page): Promise<void> {
  const size = page.viewportSize();
  await page.setViewportSize(VIEWPORTS.mobile);
  await expect
    .poll(() =>
      page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      ),
    )
    .toBe(0);
  if (size) await page.setViewportSize(size);
}

function problem(status: number, slug: string, headers: Record<string, string> = {}) {
  return (route: Route) =>
    route.fulfill({
      status,
      contentType: "application/problem+json",
      headers,
      body: JSON.stringify({ type: PROBLEM + slug, title: slug, status, detail: "" }),
    });
}

/** A new `must_reset` account, made by the admin over the API (one per attempt). */
async function createMustResetUser(site: SiteInfo, siteKey: string) {
  const ctx = await apiRequest.newContext({
    baseURL: site.url,
    storageState: authFile(siteKey, "admin"),
    extraHTTPHeaders: { Origin: site.url },
  });
  try {
    const me = (await (await ctx.get("/api/v1/auth/me")).json()) as { csrf_token: string };
    const login = uid("mustreset");
    const password = `e2e-onetime-${randomBytes(6).toString("hex")}`;
    const res = await ctx.post("/api/v1/users", {
      headers: { "X-CSRF-Token": me.csrf_token },
      data: {
        login,
        email: `${login}@example.test`,
        display_name: "Must Reset",
        role: "author",
        password,
        must_reset: true,
      },
    });
    expect(res.status(), await res.text()).toBe(201);
    return { login, password };
  } finally {
    await ctx.dispose();
  }
}

test("login_password_and_logout", async ({ browser, site, guard }) => {
  guard.allow(EXPECTED_FAILURES);
  // `editor` signs in with a session of its own; the shared storage state is untouched.
  const a = account("editor");
  const page = await anonymousPage(browser, site, guard, 1);
  await page.goto("/admin/");
  await expect(page).toHaveURL(`${site.url}/admin/login?next=%2Fadmin%2F`);
  await expect(page.getByRole("heading", { level: 1, name: "Sign in" })).toBeVisible();
  await expect(page.getByLabel("Login or e-mail")).toHaveAttribute("autocomplete", "username");
  await expect(page.getByLabel("Password", { exact: true })).toHaveAttribute(
    "autocomplete",
    "current-password",
  );

  // Show / hide the password.
  await page.getByLabel("Password", { exact: true }).fill(a.password ?? "");
  await page.getByRole("button", { name: "Show password" }).click();
  await expect(page.getByLabel("Password", { exact: true })).toHaveAttribute("type", "text");
  await page.getByRole("button", { name: "Hide password" }).click();
  await expect(page.getByLabel("Password", { exact: true })).toHaveAttribute("type", "password");

  await signIn(page, a.login, a.password ?? "");
  await expectDashboard(page, site);
  // The password went with the call: it is kept nowhere (no cached mutation holds it).
  const kept = await keptState(page);
  expect(kept).toContain(a.login);
  expect(kept).not.toContain(a.password ?? "");
  // Sign out from the account menu, which names who is signed in.
  await page.getByRole("button", { name: "Account" }).click();
  await expect(page.getByRole("menu")).toContainText(a.email);
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  await expect(page).toHaveURL(`${site.url}/admin/login`);
  await expect(page.getByRole("status")).toHaveText("You're signed out.");
  // The session is gone on the server: the guard sends the dashboard back to login.
  await page.goto("/admin/");
  await expect(page).toHaveURL(/\/admin\/login\?next=/);
  const me = await page.request.get("/api/v1/auth/me");
  expect(me.status()).toBe(401);
});

test("login_with_totp", async ({ browser, site, guard }, testInfo) => {
  test.setTimeout(120_000);
  const a = account("totp");
  const page = await anonymousPage(browser, site, guard, 2);
  await page.goto("/admin/login");
  await signIn(page, a.login, a.password ?? "");
  await expect(page).toHaveURL(`${site.url}/admin/login/totp`);
  await expect(page.getByRole("heading", { level: 1, name: "Two-factor code" })).toBeVisible();
  const input = page.getByLabel("Authentication code");
  await expect(input).toHaveAttribute("inputmode", "numeric");
  await expect(input).toHaveAttribute("autocomplete", "one-time-code");
  await expect(input).toBeFocused();

  // A step is accepted once and setup's confirm used one: wait for a newer step. A retry
  // may follow an attempt that spent the current step, so it waits for the next one.
  const spent = testInfo.retry > 0 ? Math.max(site.totp.step, stepAt()) : site.totp.step;
  const step = await nextFreshStep(spent);
  // Six digits submit on their own.
  const code = totpCode(site.totp.secret, step);
  await input.fill(code);
  await expectDashboard(page, site);
  expect(await keptState(page)).not.toContain(a.password ?? "");
  expect(await keptState(page)).not.toContain(`"${code}"`);
  expect(stepAt()).toBeGreaterThanOrEqual(step);
});

test("must_reset_session_is_confined", async ({
  browser,
  site,
  siteKey,
  guard,
  snap,
  expectAccessible,
}) => {
  guard.allow(EXPECTED_FAILURES);
  // A fresh must_reset account per attempt: the session it ends is single-use.
  const user = await createMustResetUser(site, siteKey);
  const page = await anonymousPage(browser, site, guard, 3);
  await page.goto("/admin/login");
  await signIn(page, user.login, user.password);
  await expect(page).toHaveURL(`${site.url}/admin/change-password`);
  await expect(
    page.getByRole("heading", { level: 1, name: "Choose a new password" }),
  ).toBeVisible();
  await snap(page, "auth-change-forced");
  await expectAccessible(page);

  // Nowhere else: the dashboard and the styleguide come back here.
  for (const path of ["/admin/", "/admin/_styleguide"]) {
    await page.goto(path);
    await expect(page).toHaveURL(`${site.url}/admin/change-password`);
  }
  // And the API agrees (07 §2).
  const list = await page.request.get("/api/v1/content/post");
  expect(list.status()).toBe(403);

  await page.getByLabel("Current or one-time password").fill(user.password);
  await page
    .getByLabel("New password", { exact: true })
    .fill(`e2e-chosen-${randomBytes(6).toString("hex")}`);
  await page.getByRole("button", { name: "Change password" }).click();
  await expectDashboard(page, site);
  const after = await page.request.get("/api/v1/content/post");
  expect(after.status()).toBe(200);
});

test("next_is_not_an_open_redirect", async ({ browser, site, guard }) => {
  const a = account("contributor");
  // A protocol-relative next, and one hiding an empty segment under /admin/.
  for (const [n, next] of [
    [4, "//evil.example"],
    [10, "/admin//evil.example"],
  ] as const) {
    const page = await anonymousPage(browser, site, guard, n);
    await page.goto(`/admin/login?next=${next}`);
    await signIn(page, a.login, a.password ?? "");
    await expectDashboard(page, site);
  }
});

// The two reset tests below share `resetuser`: they rely on running one after the other
// in one worker (a file's tests do; `--repeat-each` across workers makes them race).
test("forgot_and_reset_flow", async ({ browser, site, guard, snap, expectAccessible }) => {
  test.setTimeout(120_000);
  guard.allow(EXPECTED_FAILURES);
  // `resetuser`: no storage state depends on it (F23). Asks by login; the token test
  // below asks by e-mail, a separate 3-per-hour budget (07 §2).
  const a = account("resetuser");
  const page = await anonymousPage(browser, site, guard, 5);
  await page.goto("/admin/login");
  await page.getByRole("link", { name: "Forgot password?" }).click();
  await expect(page).toHaveURL(`${site.url}/admin/forgot-password`);
  // The route's chunk loads lazily: wait for its screen (the login screen also has a
  // "Login or e-mail" field).
  await expect(page.getByRole("heading", { level: 1, name: "Reset your password" })).toBeVisible();
  const after = Date.now();
  await page.getByLabel("Login or e-mail").fill(a.login);
  await page.getByRole("button", { name: "Send reset link" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Check your e-mail" })).toBeVisible();
  await snap(page, "auth-forgot-sent");
  await expectAccessible(page);

  const link = await resetLink(site, a.email, after);
  await page.goto(link.url);
  await expect(
    page.getByRole("heading", { level: 1, name: "Choose a new password" }),
  ).toBeVisible();
  expect(page.url()).toBe(`${site.url}/admin/reset-password`);
  await snap(page, "auth-reset");
  await expectAccessible(page);

  // The policy, live and on submit.
  await page.getByLabel("New password", { exact: true }).fill("too-short");
  await expect(page.getByText("9 characters · 3 more needed")).toBeVisible();
  await page.getByLabel("Repeat the new password").fill("too-short");
  await page.getByRole("button", { name: "Set password" }).click();
  await expect(page.getByText("Use at least 12 characters.")).toBeVisible();
  await snap(page, "auth-reset-error");
  await expectAccessible(page);

  const password = `e2e-reset-${randomBytes(6).toString("hex")}`;
  await page.getByLabel("New password", { exact: true }).fill(password);
  await page.getByLabel("Repeat the new password").fill(password);
  await page.getByRole("button", { name: "Set password" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Password changed" })).toBeVisible();
  await snap(page, "auth-reset-done");
  await expectAccessible(page);

  await page.getByRole("link", { name: "Sign in" }).click();
  await expect(page).toHaveURL(`${site.url}/admin/login`);
  await signIn(page, a.login, password);
  await expectDashboard(page, site);
});

test("reset_token_is_stripped_and_never_sent", async ({ browser, site, guard, request }) => {
  guard.allow(EXPECTED_FAILURES);
  const a = account("resetuser");
  const after = Date.now();
  const res = await request.post("/api/v1/auth/password-reset", {
    headers: { Origin: site.url, "X-Forwarded-For": forwardedFor(SPEC, 6) },
    data: { login: a.email },
  });
  expect(res.status()).toBe(202);
  const link = await resetLink(site, a.email, after);

  const page = await anonymousPage(browser, site, guard, 7);
  // What the browser sends: every request's URL (a fragment is never transmitted, so it
  // is dropped as the browser drops it) and its Referer.
  // `allHeaders()` waits for the response's raw headers, which WebKit never reports for a
  // request the reload cancelled: after 5 s the headers known at sending time do.
  const sent: Promise<string>[] = [];
  page.on("request", (r) => {
    const late = new Promise<Record<string, string>>((resolve) =>
      setTimeout(() => resolve(r.headers()), 5_000),
    );
    sent.push(
      Promise.race([r.allHeaders(), late]).then((h) => {
        const url = new URL(r.url());
        url.hash = "";
        return `${url.href} referer=${h.referer ?? ""}`;
      }),
    );
  });
  await page.goto(link.url);
  await expect(
    page.getByRole("heading", { level: 1, name: "Choose a new password" }),
  ).toBeVisible();
  expect(page.url()).not.toContain("#");
  expect(page.url()).not.toContain(link.token);
  await page.waitForLoadState("networkidle");

  // A reload finds no token: the link is spent as far as this page knows.
  await page.reload();
  await expect(
    page.getByRole("heading", { level: 1, name: "Link already used or missing" }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toHaveText("This link is invalid or has expired");
  await page.waitForLoadState("networkidle");

  const records = await Promise.all(sent);
  expect(records.length).toBeGreaterThan(2);
  for (const r of records) expect(r).not.toContain(link.token);
});

test("screens", async ({ browser, site, guard, as, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  guard.allow(EXPECTED_FAILURES);
  const page = await anonymousPage(browser, site, guard, 8);

  // Login: empty, wrong password (a real attempt), rate limited (stubbed 429).
  await page.goto("/admin/login");
  await expect(page.getByRole("heading", { level: 1, name: "Sign in" })).toBeVisible();
  await expect(page.getByText(new URL(site.url).host, { exact: true })).toBeVisible();
  await expectNoHorizontalScroll(page);
  await snap(page, "auth-login");
  await expectAccessible(page);

  await signIn(page, "nobody-here", "not-the-password-1");
  await expect(page.getByRole("alert")).toHaveText(
    "That login and password don't match an account.",
  );
  await snap(page, "auth-login-invalid");
  await expectAccessible(page);

  await page.route("**/api/v1/auth/login", problem(429, "rate-limited", { "Retry-After": "45" }));
  await page.getByLabel("Password", { exact: true }).fill("not-the-password-2");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByTestId("rate-limit-countdown")).toHaveText(
    /Too many attempts, try again in 4\d s/,
  );
  await expect(page.getByRole("button", { name: "Sign in" })).toBeDisabled();
  await snap(page, "auth-login-rate-limited");
  await expectAccessible(page);
  await page.unroute("**/api/v1/auth/login");

  // TOTP (stubbed challenge): the code screen and a refused code.
  await page.goto("/admin/login");
  await page.route("**/api/v1/auth/login", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ status: "totp_required", challenge: "stub-challenge" }),
    }),
  );
  await signIn(page, "someone", "any-password-123");
  await expect(page.getByRole("heading", { level: 1, name: "Two-factor code" })).toBeVisible();
  await expectNoHorizontalScroll(page);
  await snap(page, "auth-totp");
  await expectAccessible(page);
  await page.route("**/api/v1/auth/login/totp", problem(401, "invalid-credentials"));
  await page.getByLabel("Authentication code").fill("123456");
  await expect(page.getByRole("alert")).toHaveText(
    "That code is wrong or has expired. Wait for the next code and try again.",
  );
  await snap(page, "auth-totp-invalid");
  await expectAccessible(page);
  await page.unrouteAll();

  // Forgot password: the form and "mail is not set up" (stubbed 503).
  await page.goto("/admin/forgot-password");
  await expect(page.getByRole("heading", { level: 1, name: "Reset your password" })).toBeVisible();
  await expectNoHorizontalScroll(page);
  await snap(page, "auth-forgot");
  await expectAccessible(page);
  await page.route("**/api/v1/auth/password-reset", problem(503, "mail-unavailable"));
  await page.getByLabel("Login or e-mail").fill("someone");
  await page.getByRole("button", { name: "Send reset link" }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "Password reset by e-mail is not set up on this site; ask an administrator",
  );
  await snap(page, "auth-forgot-unavailable");
  await expectAccessible(page);
  await page.unrouteAll();

  // Reset without a token.
  await page.goto("/admin/reset-password");
  await expect(
    page.getByRole("heading", { level: 1, name: "Link already used or missing" }),
  ).toBeVisible();
  await expectNoHorizontalScroll(page);
  await snap(page, "auth-reset-expired");
  await expectAccessible(page);

  // The TOTP challenge times out after 5 minutes: back to login with a notice.
  const timed = await anonymousPage(browser, site, guard, 9);
  await timed.clock.install();
  await timed.goto("/admin/login");
  await timed.route("**/api/v1/auth/login", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ status: "totp_required", challenge: "stub-challenge" }),
    }),
  );
  await signIn(timed, "someone", "any-password-123");
  await expect(timed.getByRole("heading", { level: 1, name: "Two-factor code" })).toBeVisible();
  await timed.clock.fastForward("05:01");
  await expect(timed).toHaveURL(`${site.url}/admin/login`);
  await expect(timed.getByRole("status")).toHaveText(
    "The sign-in took too long, so it was cancelled. Enter your password again.",
  );
  await snap(timed, "auth-login-totp-expired");
  await expectAccessible(timed);

  // Change password by choice (a shared session, only looked at, never submitted).
  const editor = await as("editor");
  await editor.goto("/admin/change-password");
  await expect(
    editor.getByRole("heading", { level: 1, name: "Change your password" }),
  ).toBeVisible();
  await expectNoHorizontalScroll(editor);
  await snap(editor, "auth-change");
  await expectAccessible(editor);
  await editor.getByLabel("Current password").fill("whatever-it-is");
  await editor.getByLabel("New password", { exact: true }).fill("short");
  await editor.getByRole("button", { name: "Change password" }).click();
  await expect(editor.getByText("Use at least 12 characters.")).toBeVisible();
  await snap(editor, "auth-change-error");
  await expectAccessible(editor);
});

test("another_account_on_the_same_tab_sees_none_of_the_first", async ({
  browser,
  site,
  siteKey,
  guard,
}) => {
  guard.allow(EXPECTED_FAILURES);
  // The ended session answers 401: the browser logs those failed loads.
  guard.allowFailedResource(/\/api\/v1\//);
  const admin = await ContentApi.as(siteKey, site.url, "admin");
  const type = uid("t").slice(0, 32);
  const label = `Switch ${type.slice(-6)}`;
  const password = "e2e-switch-pass-0009";
  const makeAuthor = async (name: string) => {
    const login = uid(name).replace(/[^a-z0-9-]/g, "");
    const res = await admin.ctx.post("/api/v1/users", {
      headers: { "X-CSRF-Token": await csrfOfApi(admin) },
      data: { login, email: `${login}@example.test`, display_name: name, role: "author", password },
    });
    expect(res.status(), await res.text()).toBe(201);
    return login;
  };
  try {
    await admin.createType(type, label);
    const alice = await makeAuthor("alice");
    const bob = await makeAuthor("bob");

    const page = await anonymousPage(browser, site, guard, 10);
    const ctx = page.context();
    const res = await ctx.request.post("/api/v1/auth/login", {
      headers: { Origin: site.url },
      data: { login: alice, password },
    });
    expect(res.status()).toBe(200);
    const csrf = ((await res.json()) as { csrf_token: string }).csrf_token;
    const secret = `Alice only ${uid()}`;
    const made = await ctx.request.post(`/api/v1/content/${type}`, {
      headers: { Origin: site.url, "X-CSRF-Token": csrf },
      data: { title: secret },
    });
    expect(made.status(), await made.text()).toBe(201);

    // Alice's list and counts are in the tab's cache.
    const tabs = page.getByRole("navigation", { name: "Status" });
    const tab = (name: string) => tabs.getByRole("link", { name: new RegExp(`^${name}\\b`) });
    const count = (name: string) => tab(name).locator("[data-slot=tab-count]");
    await page.goto(`/admin/content/${type}`);
    await expect(page.getByRole("link", { name: secret, exact: true })).toBeVisible();
    await expect(count("Draft")).toHaveText("1");

    // Her session ends elsewhere (same cookie, another context).
    const other = await apiRequest.newContext({
      baseURL: site.url,
      storageState: await ctx.storageState(),
      extraHTTPHeaders: { Origin: site.url, "X-Forwarded-For": forwardedFor(SPEC, 10) },
    });
    expect(
      (await other.post("/api/v1/auth/logout", { headers: { "X-CSRF-Token": csrf } })).status(),
    ).toBe(204);
    await other.dispose();

    // The next request in the tab is refused: on to sign in, without a reload.
    await tab("Draft").click();
    await expect(page).toHaveURL(/\/admin\/login\?next=/);
    // From here on, record whether anything of Alice's is ever drawn.
    await page.evaluate((text) => {
      const w = window as unknown as { leaked: boolean };
      w.leaked = false;
      new MutationObserver(() => {
        if (document.body.textContent?.includes(text)) w.leaked = true;
      }).observe(document.body, { childList: true, subtree: true, characterData: true });
    }, secret);
    await signIn(page, bob, password);

    await expect(page).toHaveURL(new RegExp(`/admin/content/${type}\\?status=draft$`));
    await expect(page.getByRole("table", { name: label })).not.toHaveAttribute("aria-busy", "true");
    await expect(count("Draft")).toHaveText("0");
    await expect(count("All")).toHaveText("0");
    await tab("All").click();
    await expect(page.getByRole("table", { name: label })).not.toHaveAttribute("aria-busy", "true");
    await expect(count("All")).toHaveText("0");
    await expect(page.getByText(secret)).toHaveCount(0);
    expect(await page.evaluate(() => (window as unknown as { leaked: boolean }).leaked)).toBe(
      false,
    );
  } finally {
    await admin.removeType(type);
    await admin.dispose();
  }
});

async function csrfOfApi(api: ContentApi): Promise<string> {
  const me = await api.ctx.get("/api/v1/auth/me");
  return ((await me.json()) as { csrf_token: string }).csrf_token;
}
