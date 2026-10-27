import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { NODE_A, job, node } from "./fixtures";
import { JobsTab } from "./jobs-tab";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const HOSTILE = '<img src=x onerror="alert(1)"> boom';

function rowOf(id: string): HTMLElement {
  const row = document.querySelector<HTMLElement>(`tr[data-row-id="${id}"]`);
  if (!row) throw new Error(`no row ${id}`);
  return row;
}

test("dead_job_retry_says_when_no_node_handles_the_kind", async () => {
  const dead = job({ kind: "plugin.action", locked_by: null });
  const seen = stubRoutes({
    "/api/v1/system/jobs": () => json({ items: [dead], next_cursor: null }),
    "/api/v1/system/nodes": () =>
      json({ items: [node({ meta: { job_kinds: ["media.variants", "content.publish"] } })] }),
    [`/api/v1/system/jobs/${dead.id}/retry`]: () =>
      json({ ...dead, status: "queued", handled: false, finished_at: null }),
  });
  renderWithRouter(<JobsTab search={{ status: "dead" }} setSearch={() => {}} />);
  await screen.findByText("plugin.action");
  // The filters reach the request.
  expect(seen.find((u) => u.pathname === "/api/v1/system/jobs")?.searchParams.get("status")).toBe(
    "dead",
  );
  const row = rowOf(dead.id);
  // Attempts / max (also in the phone line under the kind).
  expect(within(row).getAllByText("1 / 5").length).toBeGreaterThan(0);
  await userEvent.click(within(row).getByRole("button", { name: "Retry plugin.action" }));
  await waitFor(() => expect(toast.warning).toHaveBeenCalledTimes(1));
  expect(toast.warning.mock.calls[0]?.[0]).toBe("Queued again. No node handles this kind yet");
  expect(toast.warning.mock.calls[0]?.[1]).toEqual({
    description: "It waits until a node that runs plugin.action jobs joins.",
  });
  expect(toast.success).not.toHaveBeenCalled();
});

test("retry_of_a_handled_kind_says_queued", async () => {
  const dead = job();
  stubRoutes({
    "/api/v1/system/jobs": () => json({ items: [dead], next_cursor: null }),
    "/api/v1/system/nodes": () => json({ items: [node()] }),
    [`/api/v1/system/jobs/${dead.id}/retry`]: () =>
      json({ ...dead, status: "queued", handled: true }),
  });
  renderWithRouter(<JobsTab search={{}} setSearch={() => {}} />);
  await screen.findByText("media.variants");
  await userEvent.click(screen.getByRole("button", { name: "Retry media.variants" }));
  await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Queued again"));
});

test("job_rows_show_text_and_unhandled_kinds", async () => {
  const long = `${HOSTILE} ${"x".repeat(300)}`;
  const queued = job({
    id: "0190a1b2-c3d4-7e5f-8a9b-000000000002",
    kind: "newer.kind",
    status: "queued",
    handled: false,
    last_error: null,
    finished_at: null,
  });
  const running = job({
    id: "0190a1b2-c3d4-7e5f-8a9b-000000000003",
    status: "running",
    locked_by: NODE_A,
    last_error: long,
  });
  stubRoutes({
    "/api/v1/system/jobs": () => json({ items: [queued, running], next_cursor: null }),
    "/api/v1/system/nodes": () => json({ items: [node()] }),
  });
  renderWithRouter(<JobsTab search={{}} setSearch={() => {}} />);
  await screen.findByText("newer.kind");
  // H-Y1: a kind no live node runs says so.
  expect(within(rowOf(queued.id)).getByText("No node handles this kind yet")).toBeVisible();
  // Only dead jobs can be retried.
  expect(within(rowOf(queued.id)).queryByRole("button", { name: /^Retry/ })).toBeNull();
  // Locked by: the node's name.
  const run = rowOf(running.id);
  expect(within(run).getAllByText("node-a").length).toBeGreaterThan(0);
  // The error is text, never markup, and a long one expands.
  expect(document.querySelector("img")).toBeNull();
  const error = within(run).getByText(long);
  expect(error.tagName).not.toBe("IMG");
  const more = within(run).getByRole("button", { name: "Show the whole error" });
  expect(more).toHaveAttribute("aria-expanded", "false");
  await userEvent.click(more);
  expect(within(run).getByRole("button", { name: "Show less" })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
});

test("status_chips_and_empty_state", async () => {
  const setSearch = vi.fn();
  stubRoutes({
    "/api/v1/system/jobs": () => json({ items: [], next_cursor: null }),
    "/api/v1/system/nodes": () => json({ items: [node()] }),
  });
  renderWithRouter(<JobsTab search={{ status: "dead" }} setSearch={setSearch} />);
  expect(await screen.findByText("No dead jobs")).toBeVisible();
  const group = screen.getByRole("group", { name: "Status" });
  expect(within(group).getByRole("button", { name: "Dead" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await userEvent.click(within(group).getByRole("button", { name: "Queued" }));
  expect(setSearch).toHaveBeenLastCalledWith({ status: "queued" });
  await userEvent.click(within(group).getByRole("button", { name: "Dead" }));
  expect(setSearch).toHaveBeenLastCalledWith({ status: undefined });
});

test("summary_line_covers_every_hidden_column", async () => {
  // Below 1280 px some columns are hidden: the line under the kind shows each piece
  // until its own column appears.
  const running = job({
    status: "running",
    locked_by: NODE_A,
    finished_at: "2026-10-27T08:00:02Z",
  });
  stubRoutes({
    "/api/v1/system/jobs": () => json({ items: [running], next_cursor: null }),
    "/api/v1/system/nodes": () => json({ items: [node()] }),
  });
  renderWithRouter(<JobsTab search={{}} setSearch={() => {}} />);
  await screen.findByText("media.variants");
  const summary = rowOf(running.id).querySelector<HTMLElement>("[data-slot=job-summary]");
  if (!summary) throw new Error("no summary line");
  expect(summary).toHaveClass("xl:hidden");
  const piece = (name: string) => {
    const el = summary.querySelector<HTMLElement>(`[data-piece=${name}]`);
    if (!el) throw new Error(`no ${name} in the summary`);
    return el;
  };
  expect(piece("status")).toHaveClass("sm:hidden");
  expect(piece("attempts")).toHaveClass("md:hidden");
  expect(piece("finished")).toHaveClass("lg:hidden");
  expect(piece("run_at")).toHaveClass("xl:hidden");
  expect(piece("locked_by")).toHaveClass("xl:hidden");
  expect(piece("locked_by")).toHaveTextContent("node-a");
  expect(piece("run_at")).toHaveTextContent(/^Run /);
});

test("kind_picker_offers_kinds_of_loaded_rows", async () => {
  if (!("scrollIntoView" in Element.prototype)) {
    Object.defineProperty(Element.prototype, "scrollIntoView", { value: () => {} });
  }
  const orphan = job({ kind: "gone.kind", handled: false });
  stubRoutes({
    "/api/v1/system/jobs": () => json({ items: [orphan], next_cursor: null }),
    // A node with malformed meta: no crash, its bad parts dropped.
    "/api/v1/system/nodes": () =>
      json({ items: [node({ meta: { job_kinds: "media.variants" } as never })] }),
  });
  const setSearch = vi.fn();
  renderWithRouter(<JobsTab search={{ kind: "url.kind" }} setSearch={setSearch} />);
  await screen.findAllByText("gone.kind");
  await userEvent.click(screen.getByRole("button", { name: "Kind: url.kind. Change kind" }));
  const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
  expect(options).toEqual(["gone.kind", "url.kind"]);
});
