import { screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { NeedsAttention } from "./needs-attention";
import { attentionAccess } from "./queries";

const EMPTY = { items: [], next_cursor: null };

const DEAD_JOB = {
  id: "0190a1b2-c3d4-7e5f-8a9b-000000000001",
  kind: "media.variants",
  status: "dead",
  attempts: 5,
  max_attempts: 5,
  priority: 0,
  created_at: "2026-10-25T08:00:00Z",
  run_at: "2026-10-25T08:00:00Z",
  finished_at: "2026-10-25T09:00:00Z",
  last_error: "decode failed: unsupported colour type",
};

const plugin = (name: string, enabled: boolean, reason: string | null) => ({
  name,
  version: "1.2.0",
  enabled,
  disabled_reason: reason,
  installed_at: "2026-10-20T08:00:00Z",
  updated_at: "2026-10-24T08:00:00Z",
  nodes: [],
});

const STATUS = {
  database: "sqlite",
  media: "local",
  mode: "solo",
  site_url: "http://localhost:8080",
  version: "0.1.0",
  jobs: { queued: 0, running: 0, done: 12, dead: 1 },
  node: { id: "node-a", started_at: "2026-10-25T07:00:00Z" },
  nodes: { live: 1, stale: 0 },
  schema: { verdict: "up_to_date", db_version: 7, binary_version: 7, pending: [] },
  theme: { active: "blog", fallback_nodes: [] },
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("needs-attention", () => {
  it("lists_dead_jobs_and_disabled_plugins", async () => {
    const seen = stubRoutes({
      "/api/v1/content/post": () => json(EMPTY),
      "/api/v1/content/page": () => json(EMPTY),
      "/api/v1/system/jobs": () => json({ items: [DEAD_JOB], next_cursor: null }),
      "/api/v1/system/status": () => json(STATUS),
      "/api/v1/plugins": () =>
        json({
          items: [
            plugin("seo-tools", false, "disabled after 5 failures in 60 s"),
            plugin("by-hand", false, null),
            plugin("analytics", true, null),
          ],
          next_cursor: null,
        }),
    });
    renderWithRouter(
      <NeedsAttention
        access={attentionAccess(["edit_others", "manage_system", "manage_plugins"])}
      />,
    );

    const card = await screen.findByRole("region", { name: "Needs attention" });
    expect(await within(card).findByText("1 dead job")).toBeVisible();
    expect(within(card).getByText("media.variants")).toBeVisible();
    expect(within(card).getByText("decode failed: unsupported colour type")).toBeVisible();
    expect(within(card).getByText("5 of 5 attempts")).toBeVisible();

    expect(within(card).getByText("1 plugin disabled after failures")).toBeVisible();
    expect(within(card).getByText("seo-tools")).toBeVisible();
    expect(within(card).getByText("disabled after 5 failures in 60 s")).toBeVisible();
    // Disabled by hand (no reason) or enabled: not something to fix.
    expect(within(card).queryByText("by-hand")).toBeNull();
    expect(within(card).queryByText("analytics")).toBeNull();

    expect(within(card).getByText("Nothing waits for review")).toBeVisible();
    expect(within(card).getByText("Up to date (version 7)")).toBeVisible();
    expect(within(card).getByText("Every node serves the active theme")).toBeVisible();
    expect(within(card).getByText("2 checks need your attention.")).toBeVisible();

    const dead = seen.find((u) => u.pathname === "/api/v1/system/jobs");
    expect(dead?.searchParams.get("status")).toBe("dead");
    expect(dead?.searchParams.get("limit")).toBe("5");
  });

  it("counts every dead job, not just the listed page", async () => {
    stubRoutes({
      "/api/v1/system/jobs": () => json({ items: [DEAD_JOB], next_cursor: "more" }),
      "/api/v1/system/status": () => json({ ...STATUS, jobs: { ...STATUS.jobs, dead: 7 } }),
    });
    renderWithRouter(<NeedsAttention access={attentionAccess(["manage_system"])} />);
    expect(await screen.findByText("7 dead jobs")).toBeVisible();
    expect(screen.getByText("1 check needs your attention.")).toBeVisible();
  });

  it("asks only for what the role may see", async () => {
    const seen = stubRoutes({
      "/api/v1/content/post": () => json(EMPTY),
      "/api/v1/content/page": () => json(EMPTY),
    });
    renderWithRouter(<NeedsAttention access={attentionAccess(["edit_others"])} />);
    expect(await screen.findByText("Nothing waits for review")).toBeVisible();
    expect(screen.getByText("All clear. Nothing is waiting for you.")).toBeVisible();
    expect(screen.queryByText("Background jobs")).toBeNull();
    expect(seen.map((u) => u.pathname).sort()).toEqual([
      "/api/v1/content/page",
      "/api/v1/content/post",
    ]);
  });

  it("a failing check fails alone", async () => {
    stubRoutes({
      "/api/v1/system/jobs": () => json(EMPTY),
      "/api/v1/system/status": () => json(STATUS),
      "/api/v1/plugins": () =>
        new Response(
          JSON.stringify({
            type: "https://ferritecms.dev/problems/internal",
            title: "Internal error",
            status: 500,
            detail: "boom",
          }),
          { status: 500, headers: { "Content-Type": "application/problem+json" } },
        ),
    });
    renderWithRouter(
      <NeedsAttention access={attentionAccess(["manage_system", "manage_plugins"])} />,
    );
    expect(await screen.findByText("Couldn't check Plugins")).toBeVisible();
    expect(screen.getByRole("button", { name: "Retry" })).toBeVisible();
    expect(await screen.findByText("No dead jobs")).toBeVisible();
  });
});
