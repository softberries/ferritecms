// The dashboard under the real server (07 §3, M2b P16/P17): asserts only what the
// seeded site has; states it cannot produce (dead jobs, disabled plugins, an empty
// activity chart) are component tests. The file runs serially: `tiles_match_counts`
// compares post counts, so no other test of this file writes posts while it runs
// (ruling at Task 12 dispatch, F35).

import { request as apiRequest, type Page, type Route } from "@playwright/test";
import { expect, test } from "./support/fixtures";
import { authFile } from "./support/server";
import { uid } from "./support/uid";
import type { Role } from "./support/users";

test.describe.configure({ mode: "serial" });

const PROBLEM = "https://ferritecms.dev/problems/";
const EXPECTED_FAILURES = /Failed to load resource: the server responded with a status of 500/;

async function openDashboard(page: Page): Promise<void> {
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
}

/** Waits until every card has left its loading state. */
async function loaded(page: Page): Promise<void> {
  await expect(page.locator("[aria-busy=true]")).toHaveCount(0);
  await expect(page.locator("[data-slot=tile-skeleton]")).toHaveCount(0);
}

/** A request context of `role`'s shared session, sending the site's `Origin`. */
async function api(backend: "sqlite" | "pg", url: string, role: Role) {
  const ctx = await apiRequest.newContext({
    baseURL: url,
    storageState: authFile(backend, role),
    extraHTTPHeaders: { Origin: url },
  });
  const me = (await (await ctx.get("/api/v1/auth/me")).json()) as {
    id: string;
    csrf_token: string;
  };
  return { ctx, me, csrf: { "X-CSRF-Token": me.csrf_token } };
}

/** A post of `role`'s own, left as a draft or submitted for review. */
async function createPost(
  backend: "sqlite" | "pg",
  url: string,
  role: Role,
  status: "draft" | "pending",
): Promise<string> {
  const { ctx, csrf } = await api(backend, url, role);
  try {
    const title = `${uid(status === "draft" ? "Draft" : "Review")} dashboard`;
    const res = await ctx.post("/api/v1/content/post", { headers: csrf, data: { title } });
    expect(res.status(), await res.text()).toBe(201);
    const item = (await res.json()) as { id: string; version: number };
    if (status === "pending") {
      const submit = await ctx.post(`/api/v1/content/post/${item.id}/submit`, {
        headers: { ...csrf, "If-Match": `"${item.version}"` },
      });
      expect(submit.status(), await submit.text()).toBe(200);
    }
    return title;
  } finally {
    await ctx.dispose();
  }
}

function fail500(route: Route) {
  return route.fulfill({
    status: 500,
    contentType: "application/problem+json",
    body: JSON.stringify({
      type: `${PROBLEM}internal`,
      title: "Internal error",
      status: 500,
      detail: "The server could not answer. It has been logged.",
    }),
  });
}

test("admin_sees_health_and_attention", async ({ as, site, backend }) => {
  // Two anonymous reads of the home page: at least one cache lookup on the node.
  const anon = await apiRequest.newContext({ baseURL: site.url });
  for (let i = 0; i < 2; i++) expect((await anon.get("/")).status()).toBe(200);
  await anon.dispose();
  await createPost(backend, site.url, "author2", "pending");

  const page = await as("admin");
  await openDashboard(page);
  await loaded(page);

  const health = page.getByRole("region", { name: "Site health" });
  await expect(health).toBeVisible();
  // The page cache: numbers as text, scoped to this node.
  const cache = health.getByTestId("cache-ratio");
  await expect(cache).toBeVisible();
  await expect(health.getByText("This node only")).toBeVisible();
  const counts = await cache.locator("dd.tabular-nums").allTextContents();
  expect(counts).toHaveLength(4);
  for (const c of counts.slice(0, 3)) expect(c).toMatch(/^\d[\d,]*$/);
  const [hits = "0", misses = "0"] = counts;
  expect(Number(hits.replace(/,/g, "")) + Number(misses.replace(/,/g, ""))).toBeGreaterThan(0);
  await expect(cache.getByText(/^\d+%$/).first()).toBeVisible();

  // Jobs by status: a bar chart with every status named.
  const jobs = health.getByTestId("jobs-chart");
  await expect(jobs.locator(".recharts-bar")).toHaveCount(1);
  for (const label of ["Queued", "Running", "Done", "Dead"]) {
    await expect(jobs.locator("svg text", { hasText: label })).toBeVisible();
  }
  // Plugin failures: every node's, so not "this node only".
  await expect(health.getByText("All nodes")).toBeVisible();

  const attention = page.getByRole("region", { name: "Needs attention" });
  // `system.spec.ts` makes a dead job for a few seconds (retry_dead_job) and the specs run
  // in parallel: the check has loaded, with no dead job or with a link to them.
  await expect(attention.getByText(/^(No dead jobs|\d+ dead jobs?)$/)).toBeVisible();
  if (await attention.getByText(/^\d+ dead jobs?$/).isVisible()) {
    await expect(attention.getByRole("link", { name: "View dead jobs" })).toHaveAttribute(
      "href",
      "/admin/system/jobs?status=dead",
    );
  }
  await expect(attention.getByText(/^\d+ items? wait/)).toBeVisible();
  await expect(attention.getByText("Up to date", { exact: false })).toBeVisible();
});

test("author_sees_no_admin_cards", async ({ as, site, backend }) => {
  const title = await createPost(backend, site.url, "author", "draft");
  const page = await as("author");
  const requests: URL[] = [];
  page.on("request", (r) => requests.push(new URL(r.url())));
  await openDashboard(page);
  await loaded(page);
  await page.waitForLoadState("networkidle");

  await expect(page.getByRole("region", { name: "Publishing activity" })).toBeVisible();
  const drafts = page.getByRole("region", { name: "Recent drafts" });
  await expect(drafts.getByRole("link", { name: new RegExp(title) })).toBeVisible();
  await expect(page.getByRole("region", { name: "Site health" })).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Needs attention" })).toHaveCount(0);

  const paths = requests.map((u) => u.pathname);
  expect(paths.filter((p) => p.startsWith("/api/v1/system"))).toEqual([]);
  expect(paths.filter((p) => p.startsWith("/api/v1/plugins"))).toEqual([]);
  // Recent drafts ask for the author's own id, never `me` (pre-flight F34).
  const { ctx, me } = await api(backend, site.url, "author");
  await ctx.dispose();
  const draftLists = requests.filter((u) => u.searchParams.get("status") === "draft");
  expect(draftLists.length).toBeGreaterThan(0);
  for (const u of draftLists) expect(u.searchParams.get("author")).toBe(me.id);
});

test("tiles_match_counts", async ({ as, site, backend }) => {
  await createPost(backend, site.url, "admin", "draft");
  const page = await as("admin");
  await openDashboard(page);
  const tiles = page.getByRole("region", { name: "Posts at a glance" });
  const value = async (label: string) => {
    const text = await tiles
      // By its accessible name's start: captions may contain another tile's label.
      .getByRole("link", { name: new RegExp(`^${label}`) })
      .locator("[data-slot=metric-value]")
      .textContent();
    return Number((text ?? "").replace(/\D/g, ""));
  };
  // Another worker may write posts between the two reads; a reload and a fresh read
  // converge as soon as nothing is written in between.
  await expect
    .poll(
      async () => {
        await page.reload();
        await loaded(page);
        const shown = {
          published: await value("Published"),
          draft: await value("Drafts"),
          pending: await value("Pending review"),
          scheduled: await value("Scheduled"),
        };
        const res = await page.request.get("/api/v1/content/post/counts");
        const c = (await res.json()) as Record<string, number>;
        const api = {
          published: c.published,
          draft: c.draft,
          pending: c.pending,
          scheduled: c.scheduled,
        };
        return JSON.stringify(shown) === JSON.stringify(api)
          ? "equal"
          : JSON.stringify({ shown, api });
      },
      { timeout: 30_000 },
    )
    .toBe("equal");
  expect(await value("Drafts")).toBeGreaterThan(0);
  // Each tile opens the list filtered to its status.
  await expect(tiles.getByRole("link", { name: /^Drafts/ })).toHaveAttribute(
    "href",
    "/admin/content/post?status=draft",
  );
});

test("settings_failure_keeps_the_dashboard", async ({ as, guard }) => {
  // The header's site title is decoration: without settings the cards still load.
  guard.allow(EXPECTED_FAILURES);
  const page = await as("editor");
  await page.route("**/api/v1/settings", fail500);
  await openDashboard(page);
  await loaded(page);
  await expect(page.getByText("Here's what's happening on your site.")).toBeVisible();
  await expect(page.getByRole("region", { name: "Posts at a glance" })).toBeVisible();
  await expect(page.getByTestId("activity-chart")).toBeVisible();
  await expect(page.getByRole("region", { name: "Recent drafts" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Needs attention" })).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await page.unrouteAll();
});

test("screens", async ({ as, guard, snap, expectAccessible }) => {
  test.setTimeout(300_000);
  guard.allow(EXPECTED_FAILURES);
  for (const role of ["admin", "editor", "author"] as const) {
    const page = await as(role);
    await openDashboard(page);
    await loaded(page);
    await expect(page.getByTestId("activity-chart").locator(".recharts-area")).toHaveCount(2);
    await snap(page, `dashboard-${role}`);
    await expectAccessible(page);
  }

  // Empty cards: no published items and no drafts (the seeded site always has some).
  const page = await as("admin");
  await page.route(/\/api\/v1\/content\/(post|page)\?/, (route) => {
    const url = new URL(route.request().url());
    const status = url.searchParams.get("status");
    if (status === "published" || status === "draft") {
      return route.fulfill({ json: { items: [], next_cursor: null } });
    }
    return route.fallback();
  });
  await openDashboard(page);
  await loaded(page);
  await expect(
    page.getByRole("heading", { name: "No posts published in the last 30 days" }),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "No drafts" })).toBeVisible();
  await snap(page, "dashboard-empty");
  await expectAccessible(page);
  await page.unrouteAll();

  // Everything needing attention at once, for review only (the behaviour is component-
  // tested, P17): a dead job, a plugin disabled after failures, pending migrations, a
  // fallback node and an hour-by-hour failure history of two plugins.
  const hour = (ago: number) =>
    new Date(Math.floor(Date.now() / 3_600_000) * 3_600_000 - ago * 3_600_000).toISOString();
  await page.route(/\/api\/v1\/system\/jobs\?/, (route) =>
    route.fulfill({
      json: {
        items: [
          {
            id: "0190a1b2-c3d4-7e5f-8a9b-000000000001",
            kind: "media.variants",
            status: "dead",
            attempts: 5,
            max_attempts: 5,
            priority: 0,
            created_at: hour(3),
            run_at: hour(3),
            finished_at: hour(2),
            last_error: "decode failed: unsupported colour type",
          },
        ],
        next_cursor: null,
      },
    }),
  );
  await page.route("**/api/v1/system/status", async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as {
      schema: Record<string, unknown>;
      theme: Record<string, unknown>;
    };
    await route.fulfill({
      response,
      json: {
        ...body,
        schema: { ...body.schema, verdict: "needs_up", pending: [8] },
        theme: { ...body.theme, fallback_nodes: ["0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"] },
      },
    });
  });
  const plugin = (name: string, enabled: boolean, reason: string | null) => ({
    name,
    version: "1.2.0",
    enabled,
    disabled_reason: reason,
    installed_at: hour(48),
    updated_at: hour(24),
    nodes: [],
  });
  await page.route("**/api/v1/plugins", (route) =>
    route.fulfill({
      json: {
        items: [
          plugin("seo-tools", true, null),
          plugin("webmentions", true, null),
          plugin("image-cdn", false, "disabled after 5 failures in 60 s"),
        ],
        next_cursor: null,
      },
    }),
  );
  await page.route("**/api/v1/plugins/*/metrics", (route) => {
    const seo = route.request().url().includes("/seo-tools/");
    const failures = Array.from({ length: 24 }, (_, i) => ({
      hook: "render.head",
      bucket_start: hour(i),
      count: seo ? (i * 7) % 5 : i % 6 === 0 ? 3 : 0,
    })).filter((b) => b.count > 0);
    return route.fulfill({
      json: {
        failures,
        window_secs: 86_400,
        node: { id: "node-a", since: hour(30), hooks: [] },
      },
    });
  });
  await openDashboard(page);
  await loaded(page);
  await expect(page.getByTestId("failures-chart").locator(".recharts-bar")).toHaveCount(2);
  await expect(page.getByText("1 dead job")).toBeVisible();
  // Each system check links to its System tab.
  await expect(page.getByRole("link", { name: "View dead jobs" })).toHaveAttribute(
    "href",
    "/admin/system/jobs?status=dead",
  );
  await expect(page.getByRole("link", { name: "View status" })).toHaveAttribute(
    "href",
    "/admin/system/status",
  );
  await expect(page.getByRole("link", { name: "View nodes" })).toHaveAttribute(
    "href",
    "/admin/system/nodes",
  );
  await snap(page, "dashboard-attention");
  await expectAccessible(page);
  await page.unrouteAll();

  // Failing cards: counts and node metrics answer 500; the rest of the page still works.
  await page.route("**/api/v1/content/post/counts", fail500);
  await page.route("**/api/v1/system/metrics/summary", fail500);
  await openDashboard(page);
  await expect(page.getByRole("alert")).toHaveCount(3, { timeout: 15_000 });
  await expect(page.getByRole("region", { name: "Publishing activity" })).toBeVisible();
  await expect(page.getByTestId("activity-chart")).toBeVisible();
  await snap(page, "dashboard-errors");
  await expectAccessible(page);
  await page.unrouteAll();
});
