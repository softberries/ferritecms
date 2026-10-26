// Editor safety under the real server (plan Task 16; 07 §3, Review Focus 3 and 5): the
// autosave slot brings a draft back after a reload; a 412 keeps the user's text and puts
// both versions side by side (Keep mine saves it at the next version, Take theirs keeps
// it in the slot); an expired session sends the user to sign in and back to the same
// editor with the draft; leaving with unsaved changes asks; and the review screens of
// the save indicator, the conflict dialog and the restore banner. Every item is created
// by the test itself (E2E data hygiene); the expiry test signs in a fresh session of an
// account of its own (F23) from its own address (F24).

import { request as apiRequest, type Page } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { mod } from "./support/keys";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";
import { forwardedFor } from "./support/users";

/** This spec's `X-Forwarded-For` block (F24). */
const SPEC = 16;

const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });
const body = (page: Page) => page.getByRole("textbox", { name: "Body" });
const indicator = (page: Page) => page.locator("[data-slot=editor-bar] [data-slot=save-state]");
/** The restore banner (the local backup's offer, `editor-followups.spec.ts`, aside). */
const banner = (page: Page) => page.locator("[data-slot=restore-banner]:not([data-mode=backup])");
const conflict = (page: Page) => page.locator("[data-slot=conflict-dialog]");
const leaveDialog = (page: Page) => page.locator("[data-slot=leave-dialog]");

/** The autosave tick is 10 s; a slow machine gets some room. */
const AUTOSAVE = { timeout: 25_000 };

async function openItem(page: Page, type: string, id: string): Promise<void> {
  await page.goto(`/admin/content/${type}/${id}`);
  await expect(body(page)).toHaveAttribute("contenteditable", "true");
}

/** Types `text` as a new paragraph at the end of the body. */
async function typeAtEnd(page: Page, text: string): Promise<void> {
  await body(page).click();
  await page.keyboard.press(`${await mod(page)}+End`);
  await page.keyboard.press("Enter");
  await page.keyboard.type(text);
}

/** What a sighted user reads in `locator` (screen-reader-only text left out). */
async function visibleText(locator: ReturnType<Page["locator"]>): Promise<string> {
  return locator.evaluate((el) => {
    const copy = el.cloneNode(true) as HTMLElement;
    copy.querySelectorAll(".sr-only").forEach((n) => n.remove());
    return copy.textContent ?? "";
  });
}

const bodyText = (item: unknown) => JSON.stringify((item as { body?: unknown }).body ?? null);

async function slotOf(api: ContentApi, item: Item): Promise<{ status: number; json: unknown }> {
  const res = await api.ctx.get(`/api/v1/content/${item.type}/${item.id}/autosave`);
  // `204` when there is none.
  return { status: res.status(), json: res.status() === 200 ? await res.json() : null };
}

test("autosave_restores_after_reload", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const item = await api.create("post", `Autosave ${uid()}`);
  const page = await as("author");
  await openItem(page, "post", item.id);

  const words = `Kept by the autosave ${uid()}`;
  await typeAtEnd(page, words);
  await expect(indicator(page)).toHaveAttribute("data-state", "dirty");
  // No explicit save: the slot takes it within the 10 s tick.
  await expect(indicator(page)).toHaveAttribute("data-state", "autosaved", AUTOSAVE);
  await expect(indicator(page)).toContainText("Autosaved at");
  await snap(page, "editor-autosaved");

  // The slot holds it, so a reload is not asked about (F38) and brings it back.
  await page.reload();
  await expect(banner(page)).toBeVisible();
  await expect(banner(page)).toContainText("Unsaved changes from");
  await expect(banner(page)).toContainText("were restored");
  await expect(body(page)).toContainText(words);
  // The item itself is untouched until the user saves.
  expect(bodyText(await api.get("post", item.id))).not.toContain(words);
  await snap(page, "editor-restore-banner");
  await expectAccessible(page);

  // Discard: the slot goes, the stored text comes back, Undo is offered.
  await banner(page).getByRole("button", { name: "Discard" }).click();
  await expect(banner(page)).toHaveCount(0);
  await expect(body(page)).not.toContainText(words);
  await expect(page.locator("[data-sonner-toast]").filter({ hasText: "discarded" })).toBeVisible();
  expect((await slotOf(api, item)).status).toBe(204);
  await api.dispose();
});

test("conflict_keeps_my_text", async ({ as, site, backend, snap, expectAccessible, guard }) => {
  // The refused save is a 412 by design: the browser logs that failed load.
  guard.allowFailedResource(/\/api\/v1\/content\/post\/[0-9a-f-]+$/);
  const editorApi = await ContentApi.as(backend, site.url, "editor");
  const adminApi = await ContentApi.as(backend, site.url, "admin");
  const original = `Conflict ${uid()}`;
  const item = await editorApi.create("post", original);
  const a = await as("editor");
  const b = await as("admin");
  await openItem(a, "post", item.id);
  await openItem(b, "post", item.id);

  // A saves first.
  const theirTitle = `Their title ${uid()}`;
  await title(a).fill(theirTitle);
  await a.keyboard.press(`${await mod(a)}+s`);
  await expect(indicator(a)).toHaveAttribute("data-state", "saved");

  // B saves over the version it loaded: 412, and the dialog shows both.
  const mine = `My paragraph ${uid()}`;
  await typeAtEnd(b, mine);
  await b.keyboard.press(`${await mod(b)}+s`);
  await expect(conflict(b)).toBeVisible();
  const titleDiff = conflict(b).locator("[data-field=title]");
  await expect(titleDiff).toBeVisible();
  expect(await visibleText(titleDiff.locator("[data-side=theirs]"))).toContain(theirTitle);
  expect(await visibleText(titleDiff.locator("[data-side=mine]"))).toContain(original);
  expect(await visibleText(conflict(b).locator("[data-field=body] [data-side=mine]"))).toContain(
    mine,
  );
  // What only one side has is marked as such (struck through or underlined).
  await expect(titleDiff.locator("[data-side=theirs] del").first()).toBeVisible();
  await expect(conflict(b).locator("[data-field=body] [data-side=mine] ins").first()).toBeVisible();
  // My text is still in the editor behind it.
  await expect(b.locator("[data-slot=editor-title]")).toHaveValue(original);
  // An outside click does not close it.
  await b.mouse.click(5, 5);
  await expect(conflict(b)).toBeVisible();
  await snap(b, "editor-conflict-dialog");
  await expectAccessible(b);

  // Keep mine: B's text is stored at the next version (create 1, A's save 2, B's 3).
  await conflict(b).getByRole("button", { name: "Keep mine" }).click();
  await expect(conflict(b)).toHaveCount(0);
  await expect(indicator(b)).toHaveAttribute("data-state", "saved");
  const kept = (await adminApi.get("post", item.id)) as Item & { body: unknown };
  expect(kept.version).toBe(3);
  expect(kept.title).toBe(original);
  expect(bodyText(kept)).toContain(mine);

  // The reverse path: Take theirs, and B's draft stays in B's autosave slot.
  const again = await editorApi.create("post", `Conflict ${uid()}`);
  await openItem(a, "post", again.id);
  await openItem(b, "post", again.id);
  const theirs2 = `Their second ${uid()}`;
  await title(a).fill(theirs2);
  await a.keyboard.press(`${await mod(a)}+s`);
  await expect(indicator(a)).toHaveAttribute("data-state", "saved");
  const mine2 = `My second paragraph ${uid()}`;
  await typeAtEnd(b, mine2);
  await b.keyboard.press(`${await mod(b)}+s`);
  await expect(conflict(b)).toBeVisible();
  await conflict(b).getByRole("button", { name: "Take theirs" }).click();
  await expect(conflict(b)).toHaveCount(0);
  await expect(title(b)).toHaveValue(theirs2);
  await expect(body(b)).not.toContainText(mine2);
  await expect(banner(b)).toHaveAttribute("data-mode", "kept");
  const slot = await slotOf(adminApi, again);
  expect(slot.status).toBe(200);
  expect(bodyText(slot.json)).toContain(mine2);
  expect((slot.json as { base_version: number }).base_version).toBe(1);
  await snap(b, "editor-conflict-kept");

  // Restore mine brings it back as the working draft (made from version 1: saving it
  // would ask again).
  await banner(b).getByRole("button", { name: "Restore mine" }).click();
  await expect(body(b)).toContainText(mine2);
  await expect(banner(b)).toHaveAttribute("data-mode", "restored");
  await expect(banner(b)).toContainText("has changed since then");
  // Leave it in the slot for nothing: discard so the shared admin has no stray slot.
  await banner(b).getByRole("button", { name: "Discard" }).click();
  await expect(banner(b)).toHaveCount(0);
  await editorApi.dispose();
  await adminApi.dispose();
});

/** A on `a` saves a new title, B on `b` saves over the old version and takes theirs. */
async function takeTheirsOn(
  a: Page,
  b: Page,
  editorApi: ContentApi,
): Promise<{ item: Item; mine: string; theirs: string }> {
  const item = await editorApi.create("post", `Kept ${uid()}`);
  await openItem(a, "post", item.id);
  await openItem(b, "post", item.id);
  const theirs = `Their kept ${uid()}`;
  await title(a).fill(theirs);
  await a.keyboard.press(`${await mod(a)}+s`);
  await expect(indicator(a)).toHaveAttribute("data-state", "saved");
  const mine = `My kept paragraph ${uid()}`;
  await typeAtEnd(b, mine);
  await b.keyboard.press(`${await mod(b)}+s`);
  await conflict(b).getByRole("button", { name: "Take theirs" }).click();
  await expect(banner(b)).toHaveAttribute("data-mode", "kept");
  return { item, mine, theirs };
}

// Three scenarios of the kept copy after Take theirs, one test each: each waits for a
// real autosave tick (10 s) on purpose, and together they do not fit one test's 30 s
// budget on a CI runner (the cause of the CI failure in run 40291037923).
test.describe("kept_copy_survives_new_edits", () => {
  async function editors(
    as: (role: "editor" | "admin") => Promise<Page>,
    site: { url: string },
    backend: "sqlite" | "pg",
  ) {
    const editorApi = await ContentApi.as(backend, site.url, "editor");
    const adminApi = await ContentApi.as(backend, site.url, "admin");
    const a = await as("editor");
    const b = await as("admin");
    return { editorApi, adminApi, a, b };
  }

  test.beforeEach(({ guard }) => {
    // The refused save is a 412 by design: the browser logs that failed load.
    guard.allowFailedResource(/\/api\/v1\/content\/post\/[0-9a-f-]+$/);
  });

  test("edit_then_reload_asks_and_keeps_the_copy", async ({ as, site, backend }) => {
    const { editorApi, adminApi, a, b } = await editors(as, site, backend);
    // Take theirs, edit, reload: the edits are not autosaved over the kept copy, so the
    // reload asks first; after it the kept copy is still there and comes back.
    const first = await takeTheirsOn(a, b, editorApi);
    const puts: string[] = [];
    b.on("request", (r) => {
      if (r.method() === "PUT" && r.url().endsWith(`/${first.item.id}/autosave`))
        puts.push(r.url());
    });
    const newer = `Newer edit ${uid()}`;
    await typeAtEnd(b, newer);
    await expect(banner(b)).toContainText("not autosaved while your version is kept");
    // Proving that nothing is written needs one full autosave tick (10 s) to pass.
    await b.waitForTimeout(11_000);
    expect(puts).toEqual([]);
    await expect(indicator(b)).toHaveAttribute("data-state", "dirty");
    const slot = await slotOf(adminApi, first.item);
    expect(bodyText(slot.json)).toContain(first.mine);
    expect(bodyText(slot.json)).not.toContain(newer);
    const prompts: string[] = [];
    b.on("dialog", (d) => {
      prompts.push(d.type());
      void d.accept();
    });
    await b.reload();
    expect(prompts).toEqual(["beforeunload"]);
    await expect(banner(b)).toHaveAttribute("data-mode", "restored");
    await expect(body(b)).toContainText(first.mine);
    // The new edits were backed up in this browser (M2b-2 Task 14): offered beside it.
    await expect(b.locator("[data-slot=restore-banner][data-mode=backup]")).toBeVisible();
    await banner(b).getByRole("button", { name: "Discard" }).click();
    await expect(banner(b)).toHaveCount(0);
    await editorApi.dispose();
    await adminApi.dispose();
  });

  test("discarding_the_copy_autosaves_the_new_edits", async ({ as, site, backend }) => {
    const { editorApi, adminApi, a, b } = await editors(as, site, backend);
    // Take theirs, edit, Discard the kept copy: the slot then takes the new edits, and a
    // reload (not asked about) brings those back.
    const second = await takeTheirsOn(a, b, editorApi);
    const kept = `Edit to keep ${uid()}`;
    await typeAtEnd(b, kept);
    // The real condition: the autosave PUT of the new edits (waited for from before the
    // Discard, so it cannot be missed).
    const put = b.waitForResponse(
      (r) => r.request().method() === "PUT" && r.url().endsWith(`/${second.item.id}/autosave`),
      AUTOSAVE,
    );
    await banner(b).getByRole("button", { name: "Discard" }).click();
    await expect(banner(b)).toHaveCount(0);
    expect((await put).status()).toBe(200);
    await expect(indicator(b)).toHaveAttribute("data-state", "autosaved");
    const slot = await slotOf(adminApi, second.item);
    expect(bodyText(slot.json)).toContain(kept);
    expect(bodyText(slot.json)).not.toContain(second.mine);
    const prompts: string[] = [];
    b.on("dialog", (d) => {
      prompts.push(d.type());
      void d.accept();
    });
    await b.reload();
    expect(prompts).toEqual([]);
    await expect(banner(b)).toHaveAttribute("data-mode", "restored");
    await expect(body(b)).toContainText(kept);
    await expect(title(b)).toHaveValue(second.theirs);
    await banner(b).getByRole("button", { name: "Discard" }).click();
    await expect(banner(b)).toHaveCount(0);
    await editorApi.dispose();
    await adminApi.dispose();
  });

  test("leaving_with_discard_keeps_the_copy", async ({ as, site, backend }) => {
    const { editorApi, adminApi, a, b } = await editors(as, site, backend);
    // Take theirs, edit, leave with Discard changes: only the new edits go; the kept copy
    // stays in the slot, and coming back brings it back (made from the older version).
    const third = await takeTheirsOn(a, b, editorApi);
    await typeAtEnd(b, `Dropped on leave ${uid()}`);
    await b.locator("[data-slot=editor-bar]").getByRole("link", { name: "Posts" }).click();
    await expect(leaveDialog(b)).toContainText("Your own version stays kept as an autosave.");
    await leaveDialog(b).getByRole("button", { name: "Discard changes" }).click();
    await expect(b).toHaveURL(/\/admin\/content\/post$/);
    const slot = await slotOf(adminApi, third.item);
    expect(slot.status).toBe(200);
    expect(bodyText(slot.json)).toContain(third.mine);
    await openItem(b, "post", third.item.id);
    await expect(banner(b)).toHaveAttribute("data-mode", "restored");
    await expect(banner(b)).toContainText("has changed since then");
    await expect(body(b)).toContainText(third.mine);
    await banner(b).getByRole("button", { name: "Discard" }).click();
    await expect(banner(b)).toHaveCount(0);
    await editorApi.dispose();
    await adminApi.dispose();
  });
});

test("session_expiry_mid_edit_keeps_the_draft", async ({ browser, site, backend, guard, snap }) => {
  // An account of its own: its session is ended on purpose (F23).
  const admin = await ContentApi.as(backend, site.url, "admin");
  const login = uid("expiry").replace(/[^a-z0-9-]/g, "");
  const password = "e2e-expiry-pass-0016";
  const created = await admin.ctx.post("/api/v1/users", {
    headers: { "X-CSRF-Token": await csrfOf(admin) },
    data: {
      login,
      email: `${login}@example.test`,
      display_name: "Expiry",
      role: "author",
      password,
    },
  });
  expect(created.status(), await created.text()).toBe(201);

  const headers = { "X-Forwarded-For": forwardedFor(SPEC, 1) };
  const ctx = await browser.newContext({
    baseURL: site.url,
    viewport: { width: 1440, height: 900 },
    colorScheme: "light",
    extraHTTPHeaders: headers,
  });
  await guard.watch(ctx);
  // The ended session answers 401: the browser logs those failed loads.
  guard.allowFailedResource(/\/api\/v1\//);
  const res = await ctx.request.post("/api/v1/auth/login", {
    headers: { Origin: site.url },
    data: { login, password },
  });
  expect(res.status()).toBe(200);
  const csrf = ((await res.json()) as { csrf_token: string }).csrf_token;
  const made = await ctx.request.post("/api/v1/content/post", {
    headers: { Origin: site.url, "X-CSRF-Token": csrf },
    data: { title: `Expiry ${uid()}` },
  });
  expect(made.status(), await made.text()).toBe(201);
  const item = (await made.json()) as Item;

  const page = await ctx.newPage();
  await openItem(page, "post", item.id);
  const first = `Written before the session ended ${uid()}`;
  await typeAtEnd(page, first);

  // The session ends elsewhere: another context with the same cookie logs out.
  const other = await apiRequest.newContext({
    baseURL: site.url,
    storageState: await ctx.storageState(),
    extraHTTPHeaders: { Origin: site.url, ...headers },
  });
  const out = await other.post("/api/v1/auth/logout", { headers: { "X-CSRF-Token": csrf } });
  expect(out.status()).toBe(204);
  await other.dispose();

  const second = ` and after ${uid()}`;
  await page.keyboard.type(second);
  await page.keyboard.press(`${await mod(page)}+s`);
  // Sign in, with the way back in `next`.
  await expect(page).toHaveURL(/\/admin\/login\?next=/);
  expect(decodeURIComponent(new URL(page.url()).searchParams.get("next") ?? "")).toBe(
    `/admin/content/post/${item.id}`,
  );
  await page.getByLabel("Login or e-mail").fill(login);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();

  // Back in the same editor, with the draft and the banner; the slot holds it now.
  await expect(page).toHaveURL(new RegExp(`/admin/content/post/${item.id}$`));
  await expect(banner(page)).toBeVisible();
  await expect(body(page)).toContainText(first);
  await expect(body(page)).toContainText(second.trim());
  await expect(indicator(page)).toHaveAttribute("data-state", "autosaved");
  await snap(page, "editor-restored-after-sign-in");
  // Saving now stores it in the item.
  await page.keyboard.press(`${await mod(page)}+s`);
  await expect(indicator(page)).toHaveAttribute("data-state", "saved");
  await expect(banner(page)).toHaveCount(0);
  const stored = await page.request.get(`/api/v1/content/post/${item.id}`);
  expect(JSON.stringify(await stored.json())).toContain(first);
  await ctx.close();
  await admin.dispose();
});

test("leave_guard_blocks_navigation", async ({ as, site, backend, snap, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const item = await api.create("post", `Leave ${uid()}`);
  const page = await as("author");
  await openItem(page, "post", item.id);
  const back = page.locator("[data-slot=editor-bar]").getByRole("link", { name: "Posts" });

  // Unsaved and not autosaved yet: closing the tab asks (beforeunload, F38).
  // Typed (a user gesture, which the browser's prompt needs).
  const edited = `Edited ${uid()}`;
  await title(page).fill("");
  await title(page).click();
  await page.keyboard.type(edited);
  const asked = page.waitForEvent("dialog");
  await page.close({ runBeforeUnload: true });
  const prompt = await asked;
  expect(prompt.type()).toBe("beforeunload");
  await prompt.dismiss();
  expect(page.isClosed()).toBe(false);

  // A router navigation asks Save / Discard / Stay.
  await back.click();
  await expect(leaveDialog(page)).toBeVisible();
  await snap(page, "editor-leave-dialog");
  await expectAccessible(page);
  await leaveDialog(page).getByRole("button", { name: "Stay" }).click();
  await expect(leaveDialog(page)).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`/admin/content/post/${item.id}$`));
  await expect(title(page)).toHaveValue(edited);

  // Save and leave: stored, then on the list.
  await back.click();
  await leaveDialog(page).getByRole("button", { name: "Save and leave" }).click();
  await expect(page).toHaveURL(/\/admin\/content\/post$/);
  expect((await api.get("post", item.id)).title).toBe(edited);

  // Discard: nothing stored, and no slot left behind.
  await openItem(page, "post", item.id);
  await title(page).fill(`Thrown away ${uid()}`);
  await back.click();
  await leaveDialog(page).getByRole("button", { name: "Discard changes" }).click();
  await expect(page).toHaveURL(/\/admin\/content\/post$/);
  expect((await api.get("post", item.id)).title).toBe(edited);
  expect((await slotOf(api, item)).status).toBe(204);
  await api.dispose();
});

test("screens", async ({ as, site, backend, snap, guard }) => {
  const api = await ContentApi.as(backend, site.url, "author");
  const item = await api.create("post", `Screens ${uid()}`);
  const page = await as("author");
  await openItem(page, "post", item.id);

  // Saving: the autosave held in flight.
  let release: () => void = () => undefined;
  const gate = new Promise<void>((r) => {
    release = r;
  });
  await page.route(`**/api/v1/content/post/${item.id}/autosave`, async (route) => {
    await gate;
    await route.continue();
  });
  await typeAtEnd(page, `Being saved ${uid()}`);
  await expect(indicator(page)).toHaveAttribute("data-state", "saving", AUTOSAVE);
  await snap(page, "editor-saving");
  release();
  await expect(indicator(page)).toHaveAttribute("data-state", "autosaved");
  await page.unroute(`**/api/v1/content/post/${item.id}/autosave`);

  // Saved: the item holds it.
  await page.keyboard.press(`${await mod(page)}+s`);
  await expect(indicator(page)).toHaveAttribute("data-state", "saved");
  await snap(page, "editor-saved");

  // Offline: the autosave cannot reach the server and waits to reconnect.
  guard.allowFailedResource(/\/autosave$/);
  guard.allow(/ERR_INTERNET_DISCONNECTED/);
  await page.context().setOffline(true);
  await typeAtEnd(page, `Written offline ${uid()}`);
  await expect(indicator(page)).toHaveAttribute("data-state", "offline", AUTOSAVE);
  await expect(indicator(page)).toContainText("Offline");
  await snap(page, "editor-offline");
  await page.context().setOffline(false);
  // Back online: retried at once (the `online` event).
  await expect(indicator(page)).toHaveAttribute("data-state", "autosaved", AUTOSAVE);
  await api.dispose();
});

async function csrfOf(api: ContentApi): Promise<string> {
  const me = await api.ctx.get("/api/v1/auth/me");
  return ((await me.json()) as { csrf_token: string }).csrf_token;
}
