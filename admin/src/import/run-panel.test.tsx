import { act, cleanup, screen, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { type ImportJobDto, type ImportRunDto, IMPORT_POLL_MS } from "@/api/import";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { importPhase, stepStates } from "./import-model";
import { RunPanel } from "./run-panel";

const JOB = "11111111-1111-4111-8111-111111111111";
const RUN = "22222222-2222-4222-8222-222222222222";

function run(over: Partial<ImportRunDto> = {}): ImportRunDto {
  const now = new Date().toISOString();
  return {
    id: RUN,
    source: "http://localhost:8099/",
    state: "running",
    step: "pass2",
    dry_run: false,
    started_by: "33333333-3333-4333-8333-333333333333",
    started_at: now,
    heartbeat_at: now,
    finished_at: null,
    counts: {},
    error: null,
    job_id: JOB,
    report: false,
    ...over,
  };
}

function job(over: Partial<ImportJobDto> = {}): ImportJobDto {
  return { job_id: JOB, status: "queued", attempts: 0, last_error: null, run: null, ...over };
}

/** Moves the fake clock by `ms`, then lets the fetch it started answer and render. */
async function tick(ms: number) {
  await act(() => vi.advanceTimersByTimeAsync(ms));
}

/** Waits for the panel's heading (`vi.waitFor` moves a faked clock 50 ms per check). */
async function heading(name: string) {
  await vi.waitFor(() => screen.getByRole("heading", { name }));
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

test("polling runs while queued/running and stops at done/dead", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  const answers = [
    job(),
    job({ status: "running", attempts: 1, run: run({ step: "pass3" }) }),
    job({
      status: "done",
      attempts: 1,
      run: run({ state: "done", step: "report", report: true, finished_at: run().started_at }),
    }),
  ];
  let calls = 0;
  stubRoutes({
    [`/api/v1/import/jobs/${JOB}`]: () => json(answers[Math.min(calls++, answers.length - 1)]),
  });
  const onSettled = vi.fn();
  renderWithRouter(<RunPanel jobId={JOB} onSettled={onSettled} onDismiss={vi.fn()} />);
  await tick(0);
  expect(calls).toBe(1);
  await heading("Waiting for a worker");

  await tick(IMPORT_POLL_MS);
  expect(calls).toBe(2);
  await heading("Importing");
  const steps = screen.getByRole("list", { name: "Steps" });
  const current = within(steps)
    .getAllByRole("listitem")
    .find((li) => li.ariaCurrent === "step");
  expect(current).toHaveTextContent("Content");
  expect(current).toHaveTextContent("in progress");
  expect(within(steps).getAllByText("done")).toHaveLength(3);

  await tick(IMPORT_POLL_MS);
  expect(calls).toBe(3);
  await heading("Import finished");
  expect(onSettled).toHaveBeenCalledTimes(1);

  // Done: no more requests.
  await tick(IMPORT_POLL_MS * 5);
  expect(calls).toBe(3);
  expect(onSettled).toHaveBeenCalledTimes(1);
});

test("a dead job stops polling too", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  let calls = 0;
  stubRoutes({
    [`/api/v1/import/jobs/${JOB}`]: () => {
      calls++;
      return json(job({ status: "dead", attempts: 3, last_error: "the export is not WXR" }));
    },
  });
  renderWithRouter(<RunPanel jobId={JOB} onDismiss={vi.fn()} />);
  await heading("Import failed");
  expect(screen.getByText("the export is not WXR")).toBeInTheDocument();
  await tick(IMPORT_POLL_MS * 5);
  expect(calls).toBe(1);
});

test("polling stops on unmount", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  let calls = 0;
  stubRoutes({
    [`/api/v1/import/jobs/${JOB}`]: () => {
      calls++;
      return json(job({ status: "running", run: run() }));
    },
  });
  renderWithRouter(<RunPanel jobId={JOB} onDismiss={vi.fn()} />);
  await heading("Importing");
  await tick(IMPORT_POLL_MS);
  expect(calls).toBe(2);
  cleanup();
  await tick(IMPORT_POLL_MS * 5);
  expect(calls).toBe(2);
});

test("polling stops after a 404", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  let calls = 0;
  stubRoutes({
    [`/api/v1/import/jobs/${JOB}`]: () => {
      calls++;
      return new Response(
        JSON.stringify({ type: "/api/v1/problems/not-found", title: "N", status: 404, detail: "" }),
        { status: 404, headers: { "Content-Type": "application/problem+json" } },
      );
    },
  });
  renderWithRouter(<RunPanel jobId={JOB} onDismiss={vi.fn()} />);
  await vi.waitFor(() => screen.getByText("This import job no longer exists."));
  await tick(IMPORT_POLL_MS * 5);
  expect(calls).toBe(1);
});

test("the live region mounts empty, then says the phase", async () => {
  stubRoutes({ [`/api/v1/import/jobs/${JOB}`]: () => json(job()) });
  renderWithRouter(<RunPanel jobId={JOB} onDismiss={vi.fn()} focusHeading />);
  const title = await screen.findByRole("heading", { name: "Waiting for a worker" });
  const live = document.querySelector<HTMLElement>("[data-slot=import-run-live]")!;
  expect(live).toHaveAttribute("aria-live", "polite");
  await vi.waitFor(() => expect(live).toHaveTextContent("Import queued."));
  // Asked to: the heading takes focus when the panel appears (start card → panel).
  expect(document.activeElement).toBe(title);
  expect(title).toHaveAttribute("tabindex", "-1");
});

test("failed run shows error and partial report link", async () => {
  const onDismiss = vi.fn();
  stubRoutes({
    [`/api/v1/import/jobs/${JOB}`]: () =>
      json(
        job({
          status: "dead",
          attempts: 3,
          last_error: "attempt error",
          run: run({
            state: "failed",
            step: "pass3",
            error: "pass3: <b>disk full</b>",
            report: true,
            finished_at: new Date().toISOString(),
            counts: {
              post: { created: 4, updated: 0, unchanged: 0, kept: 0, skipped: 1, failed: 2 },
              users: { created: 3, updated: 0, unchanged: 0, kept: 0, skipped: 0, failed: 0 },
              book: { created: 1, updated: 0, unchanged: 0, kept: 0, skipped: 0, failed: 0 },
            },
          }),
        }),
      ),
  });
  renderWithRouter(<RunPanel jobId={JOB} onDismiss={onDismiss} />);
  expect(await screen.findByRole("heading", { name: "Import failed" })).toBeInTheDocument();
  // The run's error, as text (never markup), over the job's.
  const error = document.querySelector<HTMLElement>("[data-slot=import-run-error]")!;
  // Announced once, by the phase's live region; the error box is not an alert as well.
  expect(screen.queryByRole("alert")).toBeNull();
  await vi.waitFor(() =>
    expect(document.querySelector("[data-slot=import-run-live]")).toHaveTextContent(
      "Import failed.",
    ),
  );
  expect(within(error).getByText("pass3: <b>disk full</b>")).toBeInTheDocument();
  expect(error.querySelector("b")).toBeNull();
  expect(screen.queryByText("attempt error")).toBeNull();
  // The step it failed in.
  const failed = screen.getByRole("list", { name: "Steps" }).querySelector("[data-state=failed]");
  expect(failed).toHaveTextContent("Content");
  expect(failed).toHaveTextContent("failed");

  const report = screen.getByRole("link", { name: /Open partial report/ });
  expect(report).toHaveAttribute("href", `/api/v1/import/runs/${RUN}/report.html`);
  expect(report).toHaveAttribute("target", "_blank");
  expect(report).toHaveAttribute("rel", "noopener noreferrer");
  const download = screen.getByRole("link", { name: "Download JSON" });
  expect(download).toHaveAttribute("href", `/api/v1/import/runs/${RUN}/report.json`);
  expect(download).toHaveAttribute("download", `import-${RUN}.json`);

  // Counts per kind in the passes' order, a content type by its name, then the totals.
  const table = screen.getByRole("table");
  const rows = within(table).getAllByRole("row");
  expect(rows.map((r) => r.querySelector("th")?.textContent)).toEqual([
    "Kind",
    "Users",
    "Posts",
    "book",
    "Total",
  ]);
  const totals = within(rows.at(-1)!)
    .getAllByRole("cell")
    .map((c) => c.textContent);
  expect(totals).toEqual(["8", "0", "0", "0", "1", "2"]);

  screen.getByRole("button", { name: "New import" }).click();
  expect(onDismiss).toHaveBeenCalledTimes(1);
});

test("a missing job says so", async () => {
  stubRoutes({
    [`/api/v1/import/jobs/${JOB}`]: () =>
      new Response(
        JSON.stringify({ type: "/api/v1/problems/not-found", title: "N", status: 404, detail: "" }),
        { status: 404, headers: { "Content-Type": "application/problem+json" } },
      ),
  });
  renderWithRouter(<RunPanel jobId={JOB} onDismiss={vi.fn()} />);
  expect(await screen.findByText("This import job no longer exists.")).toBeInTheDocument();
});

test("phases and steps", () => {
  expect(importPhase(job())).toBe("queued");
  expect(importPhase(job({ attempts: 1 }))).toBe("retrying");
  expect(importPhase(job({ status: "running" }))).toBe("running");
  expect(importPhase(job({ status: "done", run: run({ state: "done" }) }))).toBe("done");
  expect(importPhase(job({ status: "done", run: run({ state: "abandoned" }) }))).toBe("failed");
  expect(importPhase(job({ status: "dead" }))).toBe("failed");
  expect(stepStates(null, "queued").every((s) => s.state === "pending")).toBe(true);
  expect(stepStates(run({ step: "report" }), "done").every((s) => s.state === "done")).toBe(true);
  expect(stepStates(run({ step: "parse" }), "running")[0]?.state).toBe("current");
});
