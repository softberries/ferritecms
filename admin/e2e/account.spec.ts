// The Account screen under the real server (M2b-3 Task 6; 07 §3 Account): a password
// change keeps the session (the rotated CSRF token is stored), TOTP enrolment with a code
// computed from the shown secret and a sign-in with it, turning TOTP off needs the
// password, a new token's value and the TOTP secret are shown once and kept nowhere
// (Review Focus 3), signing out other sessions, and the review screens. Every test that
// changes an account makes its own (`uid()`); login contexts use spec number 21
// (`forwardedFor(21, n)`, F24).

import { randomBytes } from "node:crypto";
import {
  request as apiRequest,
  type Browser,
  type BrowserContext,
  type Locator,
  type Page,
} from "@playwright/test";
import { keptState } from "./support/app-state";
import { expect, test, type Guard } from "./support/fixtures";
import { authFile, type SiteInfo } from "./support/server";
import { VIEWPORTS } from "./support/snap";
import { nextFreshStep, stepAt, totpCode } from "./support/totp";
import { uid } from "./support/uid";
import { forwardedFor } from "./support/users";

const SPEC = 21;
// The browser logs every non-2xx fetch as a console error; these tests expect some.
const EXPECTED_FAILURES =
  /Failed to load resource: the server responded with a status of (401|403|422|429)/;

interface User {
  id: string;
  login: string;
  email: string;
  password: string;
}

const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  await Promise.all(contexts.splice(0).map((c) => c.close()));
});

/** A new author with a known password, made by the admin over the API. */
async function newUser(site: SiteInfo, siteKey: string, name = "Account Test"): Promise<User> {
  const ctx = await apiRequest.newContext({
    baseURL: site.url,
    storageState: authFile(siteKey, "admin"),
    extraHTTPHeaders: { Origin: site.url },
  });
  try {
    const me = (await (await ctx.get("/api/v1/auth/me")).json()) as { csrf_token: string };
    const login = uid("acct").replace(/[^a-z0-9-]/g, "");
    const password = `e2e-acct-${randomBytes(6).toString("hex")}`;
    const res = await ctx.post("/api/v1/users", {
      headers: { "X-CSRF-Token": me.csrf_token },
      data: {
        login,
        email: `${login}@example.test`,
        display_name: name,
        role: "author",
        password,
        must_reset: false,
      },
    });
    expect(res.status(), await res.text()).toBe(201);
    const user = (await res.json()) as { id: string };
    return { id: user.id, login, email: `${login}@example.test`, password };
  } finally {
    await ctx.dispose();
  }
}

/** A browser context of its own (address `n`), with no session yet. */
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

/** A page signed in as `user` on a new session; also returns its CSRF token. */
async function signedIn(
  browser: Browser,
  site: SiteInfo,
  guard: Guard,
  user: User,
  n: number,
): Promise<{ page: Page; csrf: string }> {
  const ctx = await newContext(browser, site, guard, n);
  const res = await ctx.request.post("/api/v1/auth/login", {
    headers: { Origin: site.url },
    data: { login: user.login, password: user.password },
  });
  expect(res.status(), await res.text()).toBe(200);
  const csrf = ((await res.json()) as { csrf_token: string }).csrf_token;
  return { page: await ctx.newPage(), csrf };
}

const section = (page: Page, name: string): Locator =>
  page.getByRole("region", { name, exact: true });
const toast = (page: Page, text: string) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });

async function openAccount(page: Page): Promise<void> {
  await page.goto("/admin/account");
  await expect(page.getByRole("heading", { level: 1, name: "Account" })).toBeVisible();
  await expect(section(page, "Profile").getByLabel("Display name")).not.toHaveValue("");
}

/** Starts TOTP set-up in the open screen and returns the secret the dialog shows. */
async function startTotp(page: Page): Promise<string> {
  await section(page, "Two-factor authentication").getByRole("button", { name: "Set up" }).click();
  const dialog = page.getByRole("dialog", { name: "Set up two-factor authentication" });
  const shown = dialog.locator("[data-slot=totp-secret]");
  await expect(shown).toHaveText(/^[A-Z2-7]{4}( [A-Z2-7]{1,4})+$/);
  return (await shown.innerText()).replace(/\s+/g, "");
}

/** Enrols TOTP over the API (set-up and confirm); returns the secret and the step used. */
async function enrolOverApi(page: Page, csrf: string, user: User, site: SiteInfo) {
  const headers = { Origin: site.url, "X-CSRF-Token": csrf };
  const setup = await page.request.post("/api/v1/auth/totp/setup", { headers });
  expect(setup.status()).toBe(200);
  const s = (await setup.json()) as { secret_base32: string; enrollment: string };
  const step = stepAt();
  const confirm = await page.request.post("/api/v1/auth/totp/confirm", {
    headers,
    data: {
      enrollment: s.enrollment,
      code: totpCode(s.secret_base32, step),
      password: user.password,
    },
  });
  expect(confirm.status(), await confirm.text()).toBe(204);
  return { secret: s.secret_base32, step };
}

test("password_change_keeps_the_session", async ({ browser, site, siteKey, guard }) => {
  const user = await newUser(site, siteKey);
  const { page } = await signedIn(browser, site, guard, user, 1);
  const other = await signedIn(browser, site, guard, user, 2);
  await openAccount(page);

  const card = section(page, "Password");
  const next = `e2e-new-${randomBytes(6).toString("hex")}`;
  await card.getByLabel("Current password").fill(user.password);
  await card.getByLabel("New password", { exact: true }).fill(next);
  await card.getByLabel("Repeat new password").fill(next);
  await card.getByRole("button", { name: "Change password" }).click();
  await expect(toast(page, "Password changed")).toContainText("Other sessions were signed out");
  await expect(card.getByLabel("Current password")).toHaveValue("");

  // The other session ended; this one goes on, with the new CSRF token for its writes.
  expect((await other.page.request.get("/api/v1/auth/me")).status()).toBe(401);
  const profile = section(page, "Profile");
  await profile.getByLabel("Display name").fill("Renamed After Change");
  await profile.getByRole("button", { name: "Save profile" }).click();
  await expect(toast(page, "Profile saved")).toBeVisible();
  await page.reload();
  await expect(page.getByRole("heading", { level: 1, name: "Account" })).toBeVisible();
  await expect(section(page, "Profile").getByLabel("Display name")).toHaveValue(
    "Renamed After Change",
  );
});

test("totp_enrol_and_sign_in_with_code", async ({ browser, site, siteKey, guard }) => {
  test.setTimeout(120_000);
  const user = await newUser(site, siteKey);
  const { page } = await signedIn(browser, site, guard, user, 3);
  await openAccount(page);
  const card = section(page, "Two-factor authentication");
  await expect(card.getByText("Off", { exact: true })).toBeVisible();

  const secret = await startTotp(page);
  const dialog = page.getByRole("dialog", { name: "Set up two-factor authentication" });
  await expect(dialog.getByRole("img", { name: /Scan this code/ })).toBeVisible();
  await dialog.getByRole("button", { name: "Next" }).click();
  const step = stepAt();
  await dialog.getByLabel("Six-digit code").fill(totpCode(secret, step));
  await dialog.getByLabel("Your password").fill(user.password);
  await dialog.getByRole("button", { name: "Turn on" }).click();
  await expect(toast(page, "Two-factor authentication is on")).toContainText(
    "Other sessions were signed out",
  );
  await expect(dialog).toBeHidden();
  await expect(card.getByText("On", { exact: true })).toBeVisible();

  // A sign-in now asks for a code: one from a newer step than the one confirm used.
  const ctx = await newContext(browser, site, guard, 4);
  const login = await ctx.newPage();
  await login.goto("/admin/login");
  await login.getByLabel("Login or e-mail").fill(user.login);
  await login.getByLabel("Password", { exact: true }).fill(user.password);
  await login.getByRole("button", { name: "Sign in" }).click();
  await expect(login).toHaveURL(`${site.url}/admin/login/totp`);
  const fresh = await nextFreshStep(step);
  await login.getByLabel("Authentication code").fill(totpCode(secret, fresh));
  await expect(login).toHaveURL(`${site.url}/admin/`);
  await expect(login.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
});

test("totp_disable_needs_password", async ({ browser, site, siteKey, guard }) => {
  guard.allow(EXPECTED_FAILURES);
  const user = await newUser(site, siteKey);
  const { page, csrf } = await signedIn(browser, site, guard, user, 5);
  await enrolOverApi(page, csrf, user, site);
  await openAccount(page);
  const card = section(page, "Two-factor authentication");
  await expect(card.getByText("On", { exact: true })).toBeVisible();

  await card.getByRole("button", { name: "Turn off" }).click();
  const dialog = page.getByRole("alertdialog", { name: "Turn off two-factor authentication?" });
  // Not without a password.
  await dialog.getByRole("button", { name: "Turn off" }).click();
  await expect(dialog.getByText("Enter your password.")).toBeVisible();
  await dialog.getByLabel("Your password").fill("not-the-password-at-all");
  await dialog.getByRole("button", { name: "Turn off" }).click();
  await expect(dialog.getByText("Wrong password.")).toBeVisible();
  const me = await page.request.get("/api/v1/auth/me");
  expect(((await me.json()) as { totp_enabled: boolean }).totp_enabled).toBe(true);

  await dialog.getByLabel("Your password").fill(user.password);
  await dialog.getByRole("button", { name: "Turn off" }).click();
  await expect(toast(page, "Two-factor authentication is off")).toBeVisible();
  await expect(card.getByText("Off", { exact: true })).toBeVisible();
});

test("token_value_is_shown_once", async ({ browser, site, siteKey, guard }) => {
  const user = await newUser(site, siteKey);
  const { page } = await signedIn(browser, site, guard, user, 6);
  await openAccount(page);
  const card = section(page, "API tokens");
  const name = uid("token");

  await card.getByRole("button", { name: "New token" }).click();
  const dialog = page.getByRole("dialog", { name: "New API token" });
  await dialog.getByLabel("Name").fill(name);
  await dialog
    .getByRole("radiogroup", { name: "Content" })
    .getByRole("radio", { name: "Read" })
    .check();
  await dialog
    .getByRole("radiogroup", { name: "Media" })
    .getByRole("radio", { name: "Write" })
    .check();
  await dialog.getByRole("combobox", { name: "Expires" }).click();
  await page.getByRole("option", { name: "In 30 days" }).click();
  await dialog.getByRole("button", { name: "Create token" }).click();

  const created = page.getByRole("dialog", { name: "Token created" });
  const field = created.getByLabel("Token");
  await expect(field).not.toHaveValue("");
  const value = await field.inputValue();
  expect(value.length).toBeGreaterThan(20);
  // The token works.
  const probe = await apiRequest.newContext({ baseURL: site.url });
  const res = await probe.get("/api/v1/auth/me", {
    headers: { Authorization: `Bearer ${value}` },
  });
  expect(res.status()).toBe(200);
  expect(((await res.json()) as { scopes: string[] }).scopes.sort()).toEqual([
    "content:read",
    "media:write",
  ]);
  await probe.dispose();

  // Held only by the dialog: not in the address, history, storage or either cache.
  expect(await keptState(page)).not.toContain(value);
  // Escape and a click outside do not lose it unread; Done closes.
  await page.keyboard.press("Escape");
  await page.mouse.click(5, 5);
  await expect(created.getByLabel("Token")).toHaveValue(value);
  await created.getByRole("button", { name: "Done" }).click();
  await expect(created).toBeHidden();
  expect(await page.content()).not.toContain(value);
  expect(await keptState(page)).not.toContain(value);

  // The list shows it without the value; Revoke asks first.
  const row = card.getByRole("row").filter({ hasText: name });
  await expect(row).toContainText("content:read");
  await expect(row).toContainText("media:write");
  await page.reload();
  await expect(card.getByRole("row").filter({ hasText: name })).toBeVisible();
  expect(await page.content()).not.toContain(value);
  await card.getByRole("button", { name: `Revoke ${name}` }).click();
  const confirm = page.getByRole("alertdialog", { name: `Revoke “${name}”?` });
  await confirm.getByRole("button", { name: "Revoke token" }).click();
  await expect(toast(page, "Token revoked")).toBeVisible();
  await expect(card.getByRole("row").filter({ hasText: name })).toHaveCount(0);
  const gone = await apiRequest.newContext({ baseURL: site.url });
  expect(
    (await gone.get("/api/v1/auth/me", { headers: { Authorization: `Bearer ${value}` } })).status(),
  ).toBe(401);
  await gone.dispose();
});

test("totp_secret_not_kept", async ({ browser, site, siteKey, guard }) => {
  const user = await newUser(site, siteKey);
  const { page } = await signedIn(browser, site, guard, user, 7);
  await openAccount(page);
  const secret = await startTotp(page);
  const dialog = page.getByRole("dialog", { name: "Set up two-factor authentication" });
  // Shown in the dialog only: not in the address, history, storage or either cache.
  const kept = await keptState(page);
  // The caches were read: they hold this user's own data.
  expect(kept).toContain(user.login);
  expect(kept).not.toContain(secret);
  expect(kept).not.toContain("otpauth");

  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toBeHidden();
  expect(await page.content()).not.toContain(secret);
  expect(await keptState(page)).not.toContain(secret);

  // A new set-up is a new secret.
  const again = await startTotp(page);
  expect(again).not.toBe(secret);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  expect(await page.content()).not.toContain(again);
});

test("sign_out_other_sessions", async ({ browser, site, siteKey, guard }) => {
  guard.allow(EXPECTED_FAILURES);
  const user = await newUser(site, siteKey);
  const { page } = await signedIn(browser, site, guard, user, 8);
  const b = await signedIn(browser, site, guard, user, 9);
  const c = await signedIn(browser, site, guard, user, 10);
  await openAccount(page);
  const card = section(page, "Sessions");
  const rows = card.locator("[data-slot=session-row]");
  await expect(rows).toHaveCount(3);
  await expect(rows.filter({ hasText: "This session" })).toHaveCount(1);
  await expect(
    rows.filter({ hasText: "This session" }).getByRole("button", { name: /Sign out/ }),
  ).toHaveCount(0);

  // One row: exactly one of the other two sessions ends.
  await rows
    .filter({ hasNotText: "This session" })
    .first()
    .getByRole("button", { name: /Sign out/ })
    .click();
  await expect(rows).toHaveCount(2);
  const alive = async () =>
    [
      (await b.page.request.get("/api/v1/auth/me")).status(),
      (await c.page.request.get("/api/v1/auth/me")).status(),
    ].sort();
  expect(await alive()).toEqual([200, 401]);

  // All others: the second browser loses its session and goes to sign in.
  await card.getByRole("button", { name: "Sign out all other sessions" }).click();
  await expect(toast(page, "Signed out 1 other session")).toBeVisible();
  await expect(rows).toHaveCount(1);
  expect(await alive()).toEqual([401, 401]);
  await b.page.goto("/admin/account");
  await expect(b.page).toHaveURL(/\/admin\/login\?next=/);
  // This one is still signed in.
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(200);
});

test("account_screens", async ({ browser, site, siteKey, guard, snap, expectAccessible }) => {
  test.setTimeout(120_000);
  const user = await newUser(site, siteKey, "Ada Lovelace");
  const { page, csrf } = await signedIn(browser, site, guard, user, 11);
  // A second session and two tokens, so the lists have rows.
  await signedIn(browser, site, guard, user, 12);
  for (const [name, scopes, days] of [
    ["Deploy hook", ["content:write", "media:write"], 90],
    ["Read-only reporting", ["content:read", "comments:read", "system:read"], null],
  ] as const) {
    const res = await page.request.post(`/api/v1/users/${user.id}/tokens`, {
      headers: { Origin: site.url, "X-CSRF-Token": csrf },
      data: {
        name,
        scopes,
        expires_at: days ? new Date(Date.now() + days * 86_400_000).toISOString() : null,
      },
    });
    expect(res.status(), await res.text()).toBe(201);
  }
  await openAccount(page);
  await expect(section(page, "API tokens").getByRole("row")).toHaveCount(3);
  await expect(section(page, "Sessions").locator("[data-slot=session-row]")).toHaveCount(2);
  await expectAccessible(page);
  await snap(page, "account");

  // A different e-mail asks for the password.
  const profile = section(page, "Profile");
  await profile.getByLabel("E-mail").fill(`changed-${user.email}`);
  await expect(profile.getByLabel("Current password")).toBeVisible();
  await expectAccessible(page);
  await snap(page, "account-email-change");

  // TOTP set-up, both steps (`openAccount` loads the page again, dropping the edit; a
  // `reload()` before it raced the `goto` in Firefox, NS_BINDING_ABORTED).
  await openAccount(page);
  await startTotp(page);
  const totp = page.getByRole("dialog", { name: "Set up two-factor authentication" });
  await expectAccessible(page);
  await snap(page, "account-totp-scan", { fullPage: false });
  await totp.getByRole("button", { name: "Next" }).click();
  await expect(totp.getByLabel("Six-digit code")).toBeFocused();
  await expectAccessible(page);
  await snap(page, "account-totp-code", { fullPage: false });
  await totp.getByRole("button", { name: "Cancel" }).click();
  await expect(totp).toBeHidden();

  // New token: the form, then the value shown once.
  await section(page, "API tokens").getByRole("button", { name: "New token" }).click();
  const form = page.getByRole("dialog", { name: "New API token" });
  await form.getByLabel("Name").fill("Static site build");
  await form
    .getByRole("radiogroup", { name: "Content" })
    .getByRole("radio", { name: "Read" })
    .check();
  await form
    .getByRole("radiogroup", { name: "Media" })
    .getByRole("radio", { name: "Read" })
    .check();
  await expectAccessible(page);
  await snap(page, "account-token-new", { fullPage: false });
  await form.getByRole("button", { name: "Create token" }).click();
  const created = page.getByRole("dialog", { name: "Token created" });
  await expect(created.getByLabel("Token")).not.toHaveValue("");
  await expectAccessible(page);
  await snap(page, "account-token-created", { fullPage: false });
  await created.getByRole("button", { name: "Done" }).click();
});
