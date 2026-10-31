// The Users screen under the real server (M2b-3 Task 7; 07 §3 Users): search and the role
// chip, a new user with a password who must change it at their first sign-in, a new user
// sent a reset e-mail (through the SMTP sink) who sets a password from its link, one's own
// row without role, status or delete (Review Focus 1), deleting a user who owns content
// asks for a reassignment (Review Focus 4), an admin turning off another user's TOTP, and
// the review screens. Every test makes its own users (`uid()`); the one sign-in context
// uses spec number 22 (`forwardedFor(22, n)`, F24).

import { randomBytes } from "node:crypto";
import type {
  APIRequest,
  APIRequestContext,
  BrowserContext,
  Locator,
  Page,
} from "@playwright/test";
import { expect, test } from "./support/fixtures";
import type { SiteInfo } from "./support/server";
import { resetLink } from "./support/smtp-sink";
import { VIEWPORTS } from "./support/snap";
import { stepAt, totpCode } from "./support/totp";
import { uid } from "./support/uid";
import { forwardedFor } from "./support/users";

const SPEC = 22;
/** A delete without `reassign` of a user who owns content is refused first (P46); the
 * browser logs that `422`. */
const DELETE_REFUSED = /\/api\/v1\/users\/[0-9a-f-]{36}(?:\?|$)/;

interface Made {
  id: string;
  login: string;
  email: string;
  display_name: string;
  password: string;
}

const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  await Promise.all(contexts.splice(0).map((c) => c.close()));
});

/** A login unique to this run (`[a-z0-9-]`, at most 60). */
const login = (prefix: string) => uid(prefix).replace(/[^a-z0-9-]/g, "");

async function csrfOf(request: APIRequestContext): Promise<string> {
  const me = (await (await request.get("/api/v1/auth/me")).json()) as { csrf_token: string };
  return me.csrf_token;
}

/** A user made over the admin page's API (with a known password, not `must_reset`). */
async function makeUser(
  page: Page,
  site: SiteInfo,
  opts: { login: string; role?: string; name?: string },
): Promise<Made> {
  const csrf = await csrfOf(page.request);
  const password = `e2e-users-${randomBytes(6).toString("hex")}`;
  const display_name = opts.name ?? `User ${opts.login.slice(-6)}`;
  const email = `${opts.login}@example.test`;
  const res = await page.request.post("/api/v1/users", {
    headers: { Origin: site.url, "X-CSRF-Token": csrf },
    data: {
      login: opts.login,
      email,
      display_name,
      role: opts.role ?? "author",
      password,
      must_reset: false,
    },
  });
  expect(res.status(), await res.text()).toBe(201);
  const { id } = (await res.json()) as { id: string };
  return { id, login: opts.login, email, display_name, password };
}

/** An API context signed in as `user` (its own session), with its CSRF token. */
async function sessionOf(
  site: SiteInfo,
  request: APIRequest,
  user: Made,
  n: number,
): Promise<{ api: APIRequestContext; csrf: string }> {
  const api = await request.newContext({
    baseURL: site.url,
    extraHTTPHeaders: { Origin: site.url, "X-Forwarded-For": forwardedFor(SPEC, n) },
  });
  const res = await api.post("/api/v1/auth/login", {
    data: { login: user.login, password: user.password },
  });
  expect(res.status(), await res.text()).toBe(200);
  const csrf = ((await res.json()) as { csrf_token: string }).csrf_token;
  return { api, csrf };
}

const screen = (page: Page) => page.locator("[data-slot=users-screen]");
// By CSS, not role: while the sheet is open the page behind it is hidden from the
// accessibility tree.
const table = (page: Page) => page.locator("[data-slot=users-screen] table");
const rows = (page: Page) => table(page).locator("tr[data-row]");
const sheet = (page: Page) => page.locator("[data-slot=user-sheet]");
const toast = (page: Page, text: string) =>
  page.locator("[data-sonner-toast]").filter({ hasText: text });

async function open(page: Page, query = ""): Promise<void> {
  await page.goto(`/admin/users${query}`);
  await expect(screen(page)).toBeVisible();
  await expect(table(page)).not.toHaveAttribute("aria-busy", "true");
}

const row = (page: Page, text: string): Locator => rows(page).filter({ hasText: text });

test("filter_by_q_and_role", async ({ as, site }) => {
  const page = await as("admin");
  const tag = login("flt");
  const editor = await makeUser(page, site, { login: `${tag}-ed`, role: "editor" });
  const author = await makeUser(page, site, { login: `${tag}-au`, role: "author" });
  await open(page);

  await page.getByRole("searchbox", { name: "Search users" }).fill(tag);
  await expect(page).toHaveURL(new RegExp(`[?&]q=${tag}`));
  await expect(rows(page)).toHaveCount(2);
  await expect(row(page, editor.login)).toBeVisible();
  await expect(row(page, author.login)).toBeVisible();

  await page
    .getByRole("group", { name: "Filter by role" })
    .getByRole("button", { name: "Editor" })
    .click();
  await expect(page).toHaveURL(/[?&]role=editor/);
  await expect(rows(page)).toHaveCount(1);
  await expect(row(page, editor.login)).toBeVisible();

  // The URL holds the filters: a reload keeps them.
  await page.reload();
  await expect(rows(page)).toHaveCount(1);
  await expect(page.getByRole("searchbox", { name: "Search users" })).toHaveValue(tag);
  await expect(
    page.getByRole("group", { name: "Filter by role" }).getByRole("button", { name: "Editor" }),
  ).toHaveAttribute("aria-pressed", "true");

  await page.getByRole("button", { name: "Clear filters" }).first().click();
  await expect(page).not.toHaveURL(/role=|q=/);
});

test("create_user_with_password_must_change", async ({ as, browser, site, guard }) => {
  const page = await as("admin");
  await open(page);
  const name = login("new");
  const password = `e2e-first-${randomBytes(6).toString("hex")}`;
  await page.getByRole("button", { name: "New user" }).click();
  const dialog = page.getByRole("dialog", { name: "New user" });
  await dialog.getByLabel("Login").fill(name);
  await dialog.getByLabel("E-mail", { exact: true }).fill(`${name}@example.test`);
  await dialog.getByLabel("Display name").fill("Fresh Author");
  // Mail is set up on the E2E site, so both ways in are offered.
  await expect(dialog.getByRole("radio", { name: "Send a reset e-mail" })).toBeVisible();
  await expect(dialog.getByRole("radio", { name: "Set a password" })).toBeChecked();
  await dialog.getByLabel("Password", { exact: true }).fill(password);
  await dialog.getByRole("button", { name: "Create user" }).click();
  await expect(toast(page, "Fresh Author was added")).toBeVisible();
  await expect(dialog).toBeHidden();
  // The password is not shown back, and the new account must change it.
  await expect(page.getByText(password)).toHaveCount(0);
  await page.getByRole("searchbox", { name: "Search users" }).fill(name);
  await expect(row(page, name)).toContainText("Must change password");

  // Signing in as them (a new context) leads to the forced change.
  const ctx = await browser.newContext({
    baseURL: site.url,
    viewport: VIEWPORTS.desktop,
    colorScheme: "light",
    extraHTTPHeaders: { "X-Forwarded-For": forwardedFor(SPEC, 1) },
  });
  contexts.push(ctx);
  await guard.watch(ctx);
  const them = await ctx.newPage();
  await them.goto("/admin/login");
  await them.getByLabel("Login or e-mail").fill(name);
  await them.getByLabel("Password", { exact: true }).fill(password);
  await them.getByRole("button", { name: "Sign in" }).click();
  await expect(them).toHaveURL(`${site.url}/admin/change-password`);
  await expect(
    them.getByRole("heading", { level: 1, name: "Choose a new password" }),
  ).toBeVisible();
});

test("users_reset_email_reaches_the_sink", async ({ as, browser, site, guard }) => {
  const page = await as("admin");
  await open(page);
  const name = login("mail");
  const email = `${name}@example.test`;
  await page.getByRole("button", { name: "New user" }).click();
  const dialog = page.getByRole("dialog", { name: "New user" });
  await dialog.getByLabel("Login").fill(name);
  await dialog.getByLabel("E-mail", { exact: true }).fill(email);
  await dialog.getByLabel("Display name").fill("Mailed Author");
  await dialog.getByRole("radio", { name: "Send a reset e-mail" }).click();
  await expect(dialog.getByLabel("Password", { exact: true })).toHaveCount(0);
  const after = Date.now();
  await dialog.getByRole("button", { name: "Create user" }).click();
  await expect(toast(page, `A reset link was sent to ${email}`)).toBeVisible();
  await expect(dialog).toBeHidden();
  await page.getByRole("searchbox", { name: "Search users" }).fill(name);
  await expect(row(page, name)).toContainText("Must change password");

  // The mail the server sent through the SMTP sink holds a working link.
  const link = await resetLink(site, email, after);
  const ctx = await browser.newContext({
    baseURL: site.url,
    viewport: VIEWPORTS.desktop,
    colorScheme: "light",
    extraHTTPHeaders: { "X-Forwarded-For": forwardedFor(SPEC, 2) },
  });
  contexts.push(ctx);
  await guard.watch(ctx);
  // Signed out, the admin asks who is signed in first (a `401` the page handles).
  guard.allowFailedResource(/\/api\/v1\/auth\/me$/);
  const them = await ctx.newPage();
  await them.goto(link.url);
  await expect(
    them.getByRole("heading", { level: 1, name: "Choose a new password" }),
  ).toBeVisible();
  const password = `e2e-mailed-${randomBytes(6).toString("hex")}`;
  await them.getByLabel("New password", { exact: true }).fill(password);
  await them.getByLabel("Repeat the new password").fill(password);
  await them.getByRole("button", { name: "Set password" }).click();
  await expect(them.getByRole("heading", { level: 1, name: "Password changed" })).toBeVisible();
  await them.getByRole("link", { name: "Sign in" }).click();
  await expect(them).toHaveURL(`${site.url}/admin/login`);
  await them.getByLabel("Login or e-mail").fill(name);
  await them.getByLabel("Password", { exact: true }).fill(password);
  await them.getByRole("button", { name: "Sign in" }).click();
  // The reset was the forced change: straight to the dashboard.
  await expect(them).toHaveURL(`${site.url}/admin/`);
  await expect(them.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
});

test("own_row_has_no_role_status_or_delete", async ({ as }) => {
  const page = await as("admin");
  const me = (await (await page.request.get("/api/v1/auth/me")).json()) as {
    id: string;
    login: string;
  };
  await open(page, `?q=${me.login}`);
  const own = rows(page).filter({ has: page.getByText(me.login, { exact: true }) });
  await expect(own).toContainText("You");
  await own.getByText(me.login, { exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`[?&]id=${me.id}`));
  const s = sheet(page);
  await expect(s.getByRole("link", { name: "Manage your account" })).toBeVisible();
  await expect(s.getByRole("combobox")).toHaveCount(0);
  for (const name of ["Set password", "Turn off two-factor", "Delete user", "Save profile"]) {
    await expect(s.getByRole("button", { name })).toHaveCount(0);
  }
  await s.getByRole("link", { name: "Manage your account" }).click();
  await expect(page).toHaveURL(/\/admin\/account$/);
  await expect(page.getByRole("heading", { level: 1, name: "Account" })).toBeVisible();
});

test("delete_with_content_asks_reassign", async ({ as, site, playwright, guard }) => {
  guard.allowFailedResource(DELETE_REFUSED);
  const page = await as("admin");
  const tag = login("del");
  const owner = await makeUser(page, site, { login: `${tag}-own`, name: "Content Owner" });
  const heir = await makeUser(page, site, { login: `${tag}-heir`, name: "Content Heir" });
  // The owner writes a post.
  const { api, csrf } = await sessionOf(site, playwright.request, owner, 2);
  const post = await api.post("/api/v1/content/post", {
    headers: { "X-CSRF-Token": csrf },
    data: { title: `Owned ${tag}` },
  });
  expect(post.status(), await post.text()).toBe(201);
  const postId = ((await post.json()) as { id: string }).id;
  await api.dispose();

  await open(page, `?q=${tag}`);
  await row(page, owner.login).getByText(owner.login, { exact: true }).click();
  await sheet(page).getByRole("button", { name: "Delete user" }).click();
  const dialog = page.getByRole("alertdialog", { name: "Delete Content Owner?" });
  await expect(dialog).toContainText("their sessions end and their API tokens are revoked");
  await dialog.getByRole("button", { name: "Delete user" }).click();

  await expect(dialog.getByText("This user owns content. Reassign it to:")).toBeVisible();
  const confirm = dialog.getByRole("button", { name: "Reassign and delete" });
  await expect(confirm).toBeDisabled();
  await dialog.getByRole("searchbox", { name: "Search users" }).fill(heir.login);
  // The owner is never offered.
  await expect(dialog.getByRole("radio", { name: /Content Heir/ })).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Content Owner/ })).toHaveCount(0);
  await dialog.getByRole("radio", { name: /Content Heir/ }).check();
  await confirm.click();
  await expect(toast(page, "Content Owner was deleted")).toBeVisible();
  await expect(sheet(page)).toBeHidden();
  await expect(row(page, owner.login)).toHaveCount(0);

  expect((await page.request.get(`/api/v1/users/${owner.id}`)).status()).toBe(404);
  const item = (await (await page.request.get(`/api/v1/content/post/${postId}`)).json()) as {
    author_id: string;
  };
  expect(item.author_id).toBe(heir.id);
});

test("admin_turns_off_users_totp", async ({ as, site, playwright }) => {
  const page = await as("admin");
  const user = await makeUser(page, site, { login: login("totp"), name: "Lost Phone" });
  const { api, csrf } = await sessionOf(site, playwright.request, user, 3);
  const setup = await api.post("/api/v1/auth/totp/setup", { headers: { "X-CSRF-Token": csrf } });
  expect(setup.status()).toBe(200);
  const s = (await setup.json()) as { secret_base32: string; enrollment: string };
  const confirmed = await api.post("/api/v1/auth/totp/confirm", {
    headers: { "X-CSRF-Token": csrf },
    data: {
      enrollment: s.enrollment,
      code: totpCode(s.secret_base32, stepAt()),
      password: user.password,
    },
  });
  expect(confirmed.status(), await confirmed.text()).toBe(204);
  await api.dispose();

  await open(page, `?id=${user.id}`);
  const panel = sheet(page);
  await expect(panel.getByText("Two-factor authentication on").first()).toBeAttached();
  await panel.getByRole("button", { name: "Turn off two-factor" }).click();
  const confirm = page.getByRole("alertdialog");
  await expect(confirm).toContainText("Their sessions and tokens are kept");
  await confirm.getByRole("button", { name: "Turn off" }).click();
  await expect(toast(page, "Two-factor authentication is off for Lost Phone")).toBeVisible();
  await expect(panel.getByRole("button", { name: "Turn off two-factor" })).toHaveCount(0);
  const after = (await (await page.request.get(`/api/v1/users/${user.id}`)).json()) as {
    totp_enabled: boolean;
  };
  expect(after.totp_enabled).toBe(false);
});

test("users_screens", async ({ as, site, playwright, guard, snap, expectAccessible }) => {
  guard.allowFailedResource(DELETE_REFUSED);
  test.setTimeout(180_000);
  const page = await as("admin");
  const tag = login("snap");
  const ada = await makeUser(page, site, {
    login: `${tag}-ada`,
    role: "editor",
    name: "Ada Lovelace",
  });
  await makeUser(page, site, { login: `${tag}-grace`, role: "author", name: "Grace Hopper" });
  const katherine = await makeUser(page, site, {
    login: `${tag}-kj`,
    role: "contributor",
    name: "Katherine Johnson",
  });
  // A token for Ada (the sheet lists it) and a post (her delete asks for a reassignment).
  const { api, csrf } = await sessionOf(site, playwright.request, ada, 4);
  const token = await api.post(`/api/v1/users/${ada.id}/tokens`, {
    headers: { "X-CSRF-Token": csrf },
    data: { name: "Newsletter export", scopes: ["content:read"], expires_at: null },
  });
  expect(token.status(), await token.text()).toBe(201);
  const post = await api.post("/api/v1/content/post", {
    headers: { "X-CSRF-Token": csrf },
    data: { title: `Notes ${tag}` },
  });
  expect(post.status(), await post.text()).toBe(201);
  await api.dispose();
  // Katherine is disabled, so the list shows that status.
  const adminCsrf = await csrfOf(page.request);
  const disabled = await page.request.patch(`/api/v1/users/${katherine.id}`, {
    headers: { Origin: site.url, "X-CSRF-Token": adminCsrf },
    data: { status: "disabled" },
  });
  expect(disabled.status(), await disabled.text()).toBe(200);

  await open(page, `?q=${tag}`);
  await expect(rows(page)).toHaveCount(3);
  await expectAccessible(page);
  await snap(page, "users");

  await open(page, `?q=${tag}&id=${ada.id}`);
  await expect(sheet(page).getByText("Newsletter export")).toBeVisible();
  await expectAccessible(page);
  await snap(page, "users-sheet", { fullPage: false });
  // The sheet scrolls on its own: the end of it (facts, tokens, Delete) is a second snap.
  const danger = sheet(page).getByRole("region", { name: "Delete user" });
  await danger.scrollIntoViewIfNeeded();
  await expectAccessible(page);
  await snap(page, "users-sheet-end", {
    fullPage: false,
    prepare: async () => {
      await danger.scrollIntoViewIfNeeded();
    },
  });

  // One's own row (H-A1): no role, status, password, two-factor or delete.
  const me = (await (await page.request.get("/api/v1/auth/me")).json()) as { id: string };
  await open(page, `?id=${me.id}`);
  await expect(sheet(page).getByRole("link", { name: "Manage your account" })).toBeVisible();
  await expectAccessible(page);
  await snap(page, "users-sheet-own", { fullPage: false });

  await open(page, "?new=true");
  const dialog = page.getByRole("dialog", { name: "New user" });
  await dialog.getByLabel("Login").fill("hedy");
  await dialog.getByLabel("E-mail", { exact: true }).fill("hedy@example.test");
  await dialog.getByLabel("Display name").fill("Hedy Lamarr");
  await expectAccessible(page);
  await snap(page, "users-new", { fullPage: false });
  // On a phone the fields scroll and Cancel and Create user stay in view.
  await page.setViewportSize(VIEWPORTS.mobile);
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeInViewport();
  await expect(dialog.getByRole("button", { name: "Create user" })).toBeInViewport();
  await page.setViewportSize(VIEWPORTS.desktop);
  await dialog.getByRole("button", { name: "Cancel" }).click();

  await open(page, `?q=${tag}&id=${ada.id}`);
  await sheet(page).getByRole("button", { name: "Delete user" }).click();
  const del = page.getByRole("alertdialog", { name: "Delete Ada Lovelace?" });
  await del.getByRole("button", { name: "Delete user" }).click();
  await expect(del.getByText("This user owns content. Reassign it to:")).toBeVisible();
  await del.getByRole("searchbox", { name: "Search users" }).fill(tag);
  await expect(del.getByRole("radio", { name: /Grace Hopper/ })).toBeVisible();
  await del.getByRole("radio", { name: /Grace Hopper/ }).check();
  await expectAccessible(page);
  await snap(page, "users-delete-reassign", { fullPage: false });
  await del.getByRole("button", { name: "Cancel" }).click();
});
