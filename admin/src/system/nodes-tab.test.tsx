import { screen, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { NODE_A, NODE_B, node, status } from "./fixtures";
import { NodesTab } from "./nodes-tab";

afterEach(() => {
  vi.unstubAllGlobals();
});

function card(name: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-slot=node-card][data-name="${name}"]`);
  if (!el) throw new Error(`no card ${name}`);
  return el;
}

test("node_cards_render_what_meta_holds", async () => {
  stubRoutes({
    "/api/v1/system/status": () => json(status()),
    "/api/v1/system/nodes": () =>
      json({
        items: [
          node({
            id: NODE_A,
            name: "node-a",
            meta: {
              mode: "cluster",
              clock_skew_ms: 2500,
              job_kinds: ["media.variants", "plugin.action"],
              theme: {
                name: "blog",
                origin: {
                  artifact: "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12",
                },
                hash: "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12",
                fallback: true,
                error: "<b>templates/base.html:3</b> unknown filter",
              },
              plugins: {
                seo: { version: "1.2.0", status: "loaded" },
                broken: { version: "0.3.0", status: "failed", error: "trap: unreachable <script>" },
              },
              permalinks_error: "permalink.post: unknown token %foo%",
            },
          }),
          // An older node in a rolling deploy: no meta at all, and stale.
          node({
            id: NODE_B,
            name: "node-b",
            version: "0.0.9",
            schema_version: 7,
            live: false,
            heartbeat_at: "2026-10-27T07:00:00Z",
            meta: {},
          }),
        ],
      }),
  });
  renderWithRouter(<NodesTab />);
  await screen.findByText("node-a");

  const a = card("node-a");
  expect(within(a).getByText("Live")).toBeVisible();
  expect(await within(a).findByText("This node")).toBeVisible();
  // Clock skew over 1 s is flagged.
  expect(within(a).getByText("Clock 2.5 s ahead of the database")).toBeVisible();
  // Theme: name, origin, fallback and its error as text.
  expect(within(a).getByText("blog")).toBeVisible();
  expect(within(a).getByText("Installed bundle")).toBeVisible();
  expect(within(a).getByText("Fallback")).toBeVisible();
  expect(within(a).getByText("<b>templates/base.html:3</b> unknown filter")).toBeVisible();
  // Plugins with their state, errors as text.
  const plugins = within(a).getByRole("list", { name: "Plugins" });
  expect(within(plugins).getByText("seo")).toBeVisible();
  expect(within(plugins).getByText("Failed")).toBeVisible();
  expect(within(plugins).getByText("trap: unreachable <script>")).toBeVisible();
  expect(within(a).getByText("permalink.post: unknown token %foo%")).toBeVisible();
  // Job kinds.
  expect(within(a).getByText("plugin.action")).toBeVisible();
  expect(document.querySelector("b, script")).toBeNull();

  const b = card("node-b");
  expect(within(b).getByText("Stale")).toBeVisible();
  expect(within(b).getByText("0.0.9")).toBeVisible();
  expect(within(b).getByText("Not reported")).toBeVisible();
});

test("malformed_meta_is_dropped_not_thrown", async () => {
  stubRoutes({
    "/api/v1/system/status": () => json(status()),
    "/api/v1/system/nodes": () =>
      json({
        items: [
          node({
            name: "odd",
            meta: {
              plugins: { nil: null, word: "loaded", ok: { status: "loaded", version: 3 } },
              job_kinds: "media.variants",
              theme: "blog",
              clock_skew_ms: "9000",
              plugins_error: { not: "text" },
            } as never,
          }),
          node({ id: NODE_B, name: "null-plugins", meta: { plugins: null } }),
        ],
      }),
  });
  renderWithRouter(<NodesTab />);
  const odd = await screen.findByText("odd");
  const c = card("odd");
  expect(odd).toBeVisible();
  // Only the well-formed plugin entry; its non-text version dropped.
  const plugins = within(c).getByRole("list", { name: "Plugins" });
  expect(within(plugins).getAllByRole("listitem")).toHaveLength(1);
  expect(within(plugins).getByText("ok")).toBeVisible();
  expect(within(c).queryByText(/Clock/)).toBeNull();
  expect(within(c).queryByRole("list", { name: "Job kinds" })).toBeNull();
  expect(within(c).getByText("Not reported")).toBeVisible();
  // `plugins: null` is not reported, not "No plugins loaded".
  expect(within(card("null-plugins")).queryByText("No plugins loaded")).toBeNull();
});
