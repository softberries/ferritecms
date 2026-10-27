import { render, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { report, permission } from "./fixtures";
import { PermissionReport } from "./permission-report";

test("high_risk_first_and_added_marked", () => {
  const head = permission();
  const kv = permission({ key: "kv", text: "store its own data" });
  const http = permission({
    key: "http",
    text: "connect to api.example.com",
    detail: ["api.example.com"],
    risk: "high",
    high_risk: true,
  });
  const read = permission({
    key: "content.read",
    text: "read published posts and pages",
    detail: ["post", "page"],
  });
  render(
    <PermissionReport
      report={report({
        update: true,
        items: [head, read, kv, http],
        // On this update the plugin newly asks for the host and for pages.
        added: [
          http,
          permission({ key: "content.read", text: "read published pages", detail: ["page"] }),
        ],
        limits: [
          {
            name: "memory",
            requested: 48 << 20,
            default: 32 << 20,
            ceiling: 256 << 20,
            above_default: true,
          },
          {
            name: "fuel",
            requested: 1_000_000,
            default: 50_000_000,
            ceiling: 500_000_000,
            above_default: false,
          },
        ],
      })}
    />,
  );
  const items = screen.getAllByRole("listitem").filter((li) => li.dataset.slot === "permission");
  // The high-risk item first, then the others in the server's order.
  expect(items.map((li) => li.dataset.key)).toEqual(["http", "render.head", "content.read", "kv"]);
  expect(items[0]).toHaveAttribute("data-risk", "high");
  expect(within(items[0] as HTMLElement).getByText("High risk")).toBeVisible();
  // Added on this update: marked New (the widened read too); the others not.
  const isNew = (li: HTMLElement) => li.dataset.new === "true";
  expect(items.map((li) => isNew(li))).toEqual([true, false, true, false]);
  expect(within(items[0] as HTMLElement).getByText("New")).toBeVisible();
  // The detail lines, the new one marked.
  const read2 = items[2] as HTMLElement;
  expect(within(read2).getByText("post")).toBeVisible();
  expect(within(read2).getByText("page").closest("[data-new]")).toHaveAttribute("data-new", "true");
  // A limit above the default is marked; one below is not.
  const limits = screen.getAllByRole("row").filter((r) => r.dataset.slot === "limit");
  expect(limits.map((r) => r.dataset.above)).toEqual(["true", "false"]);
  expect(within(limits[0] as HTMLElement).getByText("Above the default")).toBeVisible();
  expect(limits[0]).toHaveTextContent("48 MiB");
  expect(limits[0]).toHaveTextContent("32 MiB");
  // The facts: plugin, version, artifact hash.
  const facts = screen.getByRole("group", { name: "What is being installed" });
  expect(facts).toHaveTextContent("seo-helper");
  expect(facts).toHaveTextContent("1.2.0");
  expect(facts).toHaveTextContent("a".repeat(64));
});

test("report_text_is_text", () => {
  render(
    <PermissionReport
      report={report({
        items: [permission({ text: "<img src=x onerror=alert(1)>", detail: ["<b>x</b>"] })],
        added: [],
      })}
    />,
  );
  expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeVisible();
  expect(screen.getByText("<b>x</b>")).toBeVisible();
  expect(document.querySelector("img")).toBeNull();
});

test("nothing_asked_says_so", () => {
  render(<PermissionReport report={report({ items: [], added: [] })} />);
  expect(screen.getByText("This plugin asks for no access to the site.")).toBeVisible();
});
