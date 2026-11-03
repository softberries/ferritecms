// AI activity under the real server (M6-5, design 17 §11): a personal token makes MCP
// writes (create and update a post, rename a person's term); AI activity lists the set, its
// detail shows the post's diff; a person edits the post meanwhile, so "Revert all" lists
// the conflict and stays disabled until "Overwrite later edits" is ticked; after the
// revert the post is trashed and the term has its name back. axe on both screens.
// Other workers use the same admin: the test finds its own set by its token's name.

import { request as apiRequest, type APIRequestContext } from "@playwright/test";
import { expect, test } from "./support/fixtures";
import { authFile } from "./support/server";
import { uid } from "./support/uid";

interface Session {
  ctx: APIRequestContext;
  me: { id: string; csrf_token: string };
}

async function adminSession(siteKey: string, url: string): Promise<Session> {
  const ctx = await apiRequest.newContext({
    baseURL: url,
    storageState: authFile(siteKey, "admin"),
    extraHTTPHeaders: { Origin: url },
  });
  const me = (await (await ctx.get("/api/v1/auth/me")).json()) as Session["me"];
  return { ctx, me };
}

/** One MCP `tools/call` with `token`; the tool's structured result. */
async function call(
  ctx: APIRequestContext,
  token: string,
  name: string,
  args: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  const res = await ctx.post("/mcp", {
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
      "MCP-Protocol-Version": "2025-06-18",
    },
    data: { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } },
  });
  expect(res.status(), await res.text()).toBe(200);
  const body = (await res.json()) as {
    result: { isError: boolean; structuredContent: Record<string, unknown> };
  };
  expect(body.result.isError, JSON.stringify(body)).toBe(false);
  return body.result.structuredContent;
}

test("ai_activity_review_and_revert", async ({ as, site, siteKey, guard, expectAccessible }) => {
  test.setTimeout(90_000);
  // The dialog's dry run answers 409 change-conflict on purpose: the browser logs it.
  guard.allowFailedResource(/\/api\/v1\/change-sets\/[^/]+\/revert/);
  const { ctx, me } = await adminSession(siteKey, site.url);
  const tokenName = `e2e-ai ${uid("")}`;
  try {
    const made = await ctx.post(`/api/v1/users/${me.id}/tokens`, {
      headers: { "X-CSRF-Token": me.csrf_token },
      data: { name: tokenName, scopes: ["content:write", "terms:write", "users:read"] },
    });
    expect(made.status(), await made.text()).toBe(201);
    const token = ((await made.json()) as { token: string }).token;

    // The assistant's writes.
    const title = `AI post ${uid("")}`;
    const post = await call(ctx, token, "content_create", {
      type: "post",
      title,
      body_markdown: "Opening hours for spring.",
    });
    const edited = await call(ctx, token, "content_update", {
      type: "post",
      id: post.id,
      version: post.version,
      title: `${title} (edited)`,
    });
    // A person's term, renamed by the assistant.
    const termName = `AI term ${uid("")}`;
    const madeTerm = await ctx.post("/api/v1/taxonomies/category/terms", {
      headers: { "X-CSRF-Token": me.csrf_token },
      data: { name: termName },
    });
    expect(madeTerm.status(), await madeTerm.text()).toBe(201);
    const term = (await madeTerm.json()) as { id: string };
    await call(ctx, token, "term_update", { id: term.id, name: `${termName} renamed` });

    // AI activity lists the set.
    const page = await as("admin");
    await page.goto("/admin/ai-activity");
    await expect(page.locator("[data-slot=ai-activity]")).toBeVisible();
    const row = page.getByRole("row").filter({ hasText: `API token ${tokenName}` });
    await expect(row).toBeVisible();
    await expectAccessible(page);
    await row.click();
    await expect(page.locator("[data-slot=change-set-screen]")).toBeVisible();
    await expect(page.locator("[data-slot=change-row]")).toHaveCount(3);

    // The post's diff.
    await page
      .getByRole("button", { name: new RegExp(`Show the changes of ${title} \\(edited\\)`) })
      .first()
      .click();
    await expect(page.locator("[data-slot=content-change-diff]").first()).toBeVisible();
    await expectAccessible(page);

    // A person edits the post meanwhile.
    const patched = await ctx.patch(`/api/v1/content/post/${String(post.id)}`, {
      headers: { "X-CSRF-Token": me.csrf_token, "If-Match": `"${String(edited.version)}"` },
      data: { title: `${title} by a person` },
    });
    expect(patched.status(), await patched.text()).toBe(200);

    // Revert all: the conflict is listed; confirm needs "Overwrite later edits".
    await page.locator("[data-slot=revert-all]").click();
    const dialog = page.locator("[data-slot=revert-dialog]");
    await expect(dialog.locator("[data-slot=revert-conflict]")).toHaveCount(1);
    await expect(dialog.locator("[data-slot=revert-conflict]")).toContainText("Edited by");
    const confirm = dialog.locator("[data-slot=revert-confirm]");
    await expect(confirm).toBeDisabled();
    await expectAccessible(page);
    await dialog.getByLabel(/Overwrite later edits/).check();
    await expect(confirm).toBeEnabled();
    await confirm.click();
    await expect(page.getByText(/changes reverted/)).toBeVisible();
    await expect(page.locator("[data-slot=set-status]")).toHaveAttribute("data-status", "reverted");

    // The post is in the trash, the term has its name back.
    const item = (await (await ctx.get(`/api/v1/content/post/${String(post.id)}`)).json()) as {
      status: string;
    };
    expect(item.status).toBe("trash");
    const back = (await (await ctx.get(`/api/v1/terms/${String(term.id)}`)).json()) as {
      name: string;
    };
    expect(back.name).toBe(termName);
  } finally {
    await ctx.dispose();
  }
});
