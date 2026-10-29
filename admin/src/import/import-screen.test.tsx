import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import type { ImportRunDto } from "@/api/import";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ImportScreen } from "./import-screen";

const JOB = "11111111-1111-4111-8111-111111111111";

function run(over: Partial<ImportRunDto> = {}): ImportRunDto {
  const at = "2026-10-29T10:00:00Z";
  return {
    id: "22222222-2222-4222-8222-222222222222",
    source: "http://old.example/",
    state: "done",
    step: "report",
    dry_run: false,
    started_by: "33333333-3333-4333-8333-333333333333",
    started_at: at,
    heartbeat_at: at,
    finished_at: at,
    counts: { post: { created: 5, updated: 1, unchanged: 0, kept: 2, skipped: 3, failed: 0 } },
    error: null,
    job_id: null,
    report: true,
    ...over,
  };
}

const problem = (status: number, slug: string) =>
  new Response(
    JSON.stringify({ type: `/api/v1/problems/${slug}`, title: slug, status, detail: "" }),
    {
      status,
      headers: { "Content-Type": "application/problem+json" },
    },
  );

afterEach(() => {
  vi.unstubAllGlobals();
});

test("empty, loading, error and forbidden states", async () => {
  // Loading: skeletons, busy.
  let release: (r: Response) => void = () => {};
  stubRoutes({
    "/api/v1/import/runs": () => new Promise<Response>((resolve) => (release = resolve)),
  });
  renderScreen();
  expect(await screen.findByRole("heading", { name: "Import", level: 1 })).toBeInTheDocument();
  expect(document.querySelector("[data-slot=import-start-loading]")).toHaveAttribute(
    "aria-busy",
    "true",
  );
  // Empty: the start card and the runs table's empty state.
  release(json({ items: [] }));
  expect(await screen.findByRole("button", { name: "Start import" })).toBeDisabled();
  expect(screen.getByText("No imports yet")).toBeInTheDocument();
  cleanup();

  // Error: the runs table says so and offers a retry; the start card still works.
  stubRoutes({ "/api/v1/import/runs": () => problem(500, "internal") });
  renderScreen();
  expect(await screen.findByRole("button", { name: "Try again" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Start import" })).toBeInTheDocument();
  cleanup();

  // Forbidden: the server's 403 shows the permission-denied state, no start card.
  stubRoutes({ "/api/v1/import/runs": () => problem(403, "forbidden") });
  renderScreen();
  expect(await screen.findByText("You don't have access to this")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Start import" })).toBeNull();
});

test("runs table lists each run with its totals and report links", async () => {
  stubRoutes({
    "/api/v1/import/runs": () =>
      json({
        items: [
          run(),
          run({
            id: "44444444-4444-4444-8444-444444444444",
            state: "failed",
            step: "pass3",
            error: "pass3: disk full",
            report: false,
            dry_run: true,
          }),
        ],
      }),
  });
  renderScreen();
  await screen.findByText("pass3: disk full");
  const table = screen.getByRole("table", { name: "Import runs" });
  const rows = [...table.querySelectorAll<HTMLElement>("tr[data-row-id]")];
  expect(rows).toHaveLength(2);
  const [done, failed] = rows as [HTMLElement, HTMLElement];
  expect(within(done).getAllByText("Done").length).toBeGreaterThan(0);
  expect(done).toHaveTextContent("5 created · 1 updated · 2 kept · 3 skipped");
  const html = within(done).getByRole("link", {
    name: "Open the report of the import from http://old.example/ (opens in a new tab)",
  });
  expect(html).toHaveAttribute("target", "_blank");
  expect(html).toHaveAttribute("href", `/api/v1/import/runs/${run().id}/report.html`);
  expect(
    within(done).getByRole("link", {
      name: "Download the JSON report of the import from http://old.example/",
    }),
  ).toHaveAttribute("download");
  expect(within(failed).getAllByText("Failed").length).toBeGreaterThan(0);
  expect(within(failed).getByText("Dry run")).toBeInTheDocument();
  expect(within(failed).getByText("pass3: disk full")).toBeInTheDocument();
  expect(within(failed).queryByRole("link")).toBeNull();
});

test("a running run is followed instead of offering a second start", async () => {
  const setJob = vi.fn();
  stubRoutes({
    "/api/v1/import/runs": () =>
      json({ items: [run({ state: "running", step: "pass1", job_id: JOB, finished_at: null })] }),
    [`/api/v1/import/jobs/${JOB}`]: () =>
      json({
        job_id: JOB,
        status: "running",
        attempts: 1,
        run: run({ state: "running", step: "pass1", job_id: JOB, finished_at: null }),
      }),
  });
  renderScreen({ setJob });
  expect(await screen.findByRole("heading", { name: "Importing" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Start import" })).toBeNull();
});

test("a finished job is put away with New import", async () => {
  const setJob = vi.fn();
  stubRoutes({
    "/api/v1/import/runs": () => json({ items: [run({ job_id: JOB })] }),
    [`/api/v1/import/jobs/${JOB}`]: () =>
      json({ job_id: JOB, status: "done", attempts: 1, run: run({ job_id: JOB }) }),
  });
  renderScreen({ job: JOB, setJob });
  expect(await screen.findByRole("heading", { name: "Import finished" })).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "New import" }));
  expect(setJob).toHaveBeenCalledWith(undefined);
});

function renderScreen(props: { job?: string; setJob?: (job: string | undefined) => void } = {}) {
  renderWithRouter(<ImportScreen job={props.job} setJob={props.setJob ?? vi.fn()} />);
}
