import { screen, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { status } from "./fixtures";
import { StatusTab } from "./status-tab";

afterEach(() => {
  vi.unstubAllGlobals();
});

function section(name: string): HTMLElement {
  return screen.getByRole("region", { name });
}

test("status_shows_backends_schema_mail_theme_jobs_nodes", async () => {
  stubRoutes({
    "/api/v1/system/status": () =>
      json(
        status({
          schema: { verdict: "needs_up", db_version: 7, binary_version: 8, pending: [8] },
          mail: { configured: false, from: null },
          theme: { active: null, fallback_nodes: ["node-b"] },
          nodes: { live: 2, stale: 1 },
          database: "postgres",
          media: "s3",
          mode: "cluster",
        }),
      ),
  });
  renderWithRouter(<StatusTab />);
  await screen.findByText("Postgres");

  const site = section("Site");
  expect(within(site).getByText("0.1.0")).toBeVisible();
  expect(within(site).getByText("Cluster")).toBeVisible();
  expect(within(site).getByText("S3-compatible storage")).toBeVisible();
  expect(within(site).getByText("http://localhost:8080")).toBeVisible();

  const schema = section("Database schema");
  expect(within(schema).getByText("1 migration to apply")).toBeVisible();
  expect(within(schema).getByText("8", { selector: "li" })).toBeVisible();

  expect(within(section("Mail")).getByText("Mail is not set up")).toBeVisible();

  const theme = section("Theme");
  expect(within(theme).getByText("The built-in default")).toBeVisible();
  expect(within(theme).getByText("node-b")).toBeVisible();

  const jobs = section("Background jobs");
  const dead = within(jobs).getByRole("link", { name: /Dead/ });
  expect(dead).toHaveAttribute("href", "/admin/system/jobs?status=dead");
  expect(dead).toHaveTextContent("1");

  const nodes = section("Nodes");
  expect(within(nodes).getByRole("link", { name: /Live/ })).toHaveAttribute(
    "href",
    "/admin/system/nodes",
  );
  expect(within(nodes).getByText("2")).toBeVisible();
});
