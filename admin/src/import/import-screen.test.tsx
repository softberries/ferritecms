import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, expect, test, vi } from "vitest";
import type { ImportRunDto } from "@/api/import";
import { qk } from "@/api/keys";
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
  // The links show in their column, and in the source cell below 640 px (CSS picks one).
  const links = within(done).getAllByRole("link", {
    name: "Open the report of the import from http://old.example/ (opens in a new tab)",
  });
  expect(links).toHaveLength(2);
  for (const html of links) {
    expect(html).toHaveAttribute("target", "_blank");
    expect(html).toHaveAttribute("href", `/api/v1/import/runs/${run().id}/report.html`);
  }
  for (const json of within(done).getAllByRole("link", {
    name: "Download the JSON report of the import from http://old.example/",
  })) {
    expect(json).toHaveAttribute("download", `import-${run().id}.json`);
  }
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

/** The screen with `?job=` kept in state, as the route keeps it in the URL. */
function Harness({ initial, onJob }: { initial?: string; onJob?: (job?: string) => void }) {
  const [job, setJob] = useState(initial);
  return (
    <ImportScreen
      job={job}
      setJob={(next) => {
        onJob?.(next);
        setJob(next);
      }}
    />
  );
}

const running = () => run({ state: "running", step: "pass1", job_id: JOB, finished_at: null });

test("an auto-followed run stays on show when it finishes", async () => {
  const onJob = vi.fn();
  let runsCalls = 0;
  let jobCalls = 0;
  stubRoutes({
    "/api/v1/import/runs": () =>
      json({ items: [runsCalls++ === 0 ? running() : run({ job_id: JOB })] }),
    [`/api/v1/import/jobs/${JOB}`]: () =>
      json(
        jobCalls++ === 0
          ? { job_id: JOB, status: "running", attempts: 1, run: running() }
          : { job_id: JOB, status: "done", attempts: 1, run: run({ job_id: JOB }) },
      ),
  });
  renderWithRouter(<Harness onJob={onJob} />);
  expect(await screen.findByRole("heading", { name: "Importing" })).toBeInTheDocument();
  // Followed on its own, the job is pinned in the URL.
  expect(onJob).toHaveBeenCalledWith(JOB);
  expect(
    await screen.findByRole("heading", { name: "Import finished" }, { timeout: 5_000 }),
  ).toBeInTheDocument();
  // The runs list refreshes and says the run is done: the panel and its report stay.
  await vi.waitFor(() => expect(runsCalls).toBeGreaterThanOrEqual(2));
  await vi.waitFor(() => expect(screen.getAllByText("Done").length).toBeGreaterThan(0));
  expect(screen.getByRole("heading", { name: "Import finished" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /Open report/ })).toBeInTheDocument();
}, 10_000);

test("New import moves focus to the start card's heading", async () => {
  stubRoutes({
    "/api/v1/import/runs": () => json({ items: [run({ job_id: JOB })] }),
    [`/api/v1/import/jobs/${JOB}`]: () =>
      json({ job_id: JOB, status: "done", attempts: 1, run: run({ job_id: JOB }) }),
  });
  renderWithRouter(<Harness initial={JOB} />);
  await screen.findByRole("heading", { name: "Import finished" });
  await userEvent.click(screen.getByRole("button", { name: "New import" }));
  const heading = await screen.findByRole("heading", { name: "New import" });
  await vi.waitFor(() => expect(document.activeElement).toBe(heading));
});

/** An XHR that answers every send with `status` and `body` on the next task. */
function stubXhr(status: number, body: unknown) {
  class Xhr {
    status = 0;
    responseText = "";
    withCredentials = false;
    upload = { onprogress: null };
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    onabort: (() => void) | null = null;
    open() {}
    setRequestHeader() {}
    getResponseHeader(k: string) {
      return k.toLowerCase() === "content-type"
        ? status >= 400
          ? "application/problem+json"
          : "application/json"
        : null;
    }
    send() {
      setTimeout(() => {
        this.status = status;
        this.responseText = JSON.stringify(body);
        this.onload?.();
      }, 0);
    }
    abort() {}
  }
  vi.stubGlobal("XMLHttpRequest", Xhr);
}

async function chooseAndStart() {
  const user = userEvent.setup();
  const input = await vi.waitFor(() => {
    const el = document.querySelector<HTMLInputElement>("[data-slot=import-wxr] input[type=file]");
    if (!el) throw new Error("no input yet");
    return el;
  });
  await user.upload(input, new File(["<rss/>"], "a.xml", { type: "text/xml" }));
  await user.click(screen.getByRole("button", { name: "Start import" }));
}

test("starting moves focus to the run panel's heading", async () => {
  stubXhr(202, { job_id: JOB, status: "queued", attempts: 0 });
  stubRoutes({
    "/api/v1/import/runs": () => json({ items: [] }),
    [`/api/v1/import/jobs/${JOB}`]: () => json({ job_id: JOB, status: "queued", attempts: 0 }),
  });
  renderWithRouter(<Harness />);
  await chooseAndStart();
  const heading = await screen.findByRole("heading", { name: "Waiting for a worker" });
  await vi.waitFor(() => expect(document.activeElement).toBe(heading));
});

test("a start refreshes the System screen's jobs and status", async () => {
  stubXhr(202, { job_id: JOB, status: "queued", attempts: 0 });
  stubRoutes({
    "/api/v1/import/runs": () => json({ items: [] }),
    [`/api/v1/import/jobs/${JOB}`]: () => json({ job_id: JOB, status: "queued", attempts: 0 }),
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(qk.system.status, {});
  client.setQueryData(qk.system.jobs({}), { items: [] });
  client.setQueryData(qk.types, []);
  renderWithRouter(<Harness />, client);
  await chooseAndStart();
  await screen.findByRole("heading", { name: "Waiting for a worker" });
  expect(client.getQueryState(qk.system.status)?.isInvalidated).toBe(true);
  expect(client.getQueryState(qk.system.jobs({}))?.isInvalidated).toBe(true);
  // Nothing else yet: the import has not written anything.
  expect(client.getQueryState(qk.types)?.isInvalidated).toBe(false);
});

/** Active queries on `client` (as screens hold them) counting their fetches. */
function observe(client: QueryClient, keys: Record<string, readonly unknown[]>) {
  const fetched: Record<string, number> = {};
  for (const [name, queryKey] of Object.entries(keys)) {
    fetched[name] = 0;
    new QueryObserver(client, {
      queryKey,
      staleTime: Infinity,
      queryFn: () => {
        fetched[name] = (fetched[name] ?? 0) + 1;
        return Promise.resolve({});
      },
    }).subscribe(() => {});
  }
  return fetched;
}

const KEYS = { types: qk.types, settings: qk.settings, me: qk.me, meta: qk.meta };

test("a finished import refetches every other screen's data, not me or meta", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const fetched = observe(client, KEYS);
  await vi.waitFor(() => expect(fetched).toEqual({ types: 1, settings: 1, me: 1, meta: 1 }));
  stubRoutes({
    "/api/v1/import/runs": () => json({ items: [run({ job_id: JOB })] }),
    [`/api/v1/import/jobs/${JOB}`]: () =>
      json({ job_id: JOB, status: "done", attempts: 1, run: run({ job_id: JOB }) }),
  });
  renderWithRouter(<Harness initial={JOB} />, client);
  await screen.findByRole("heading", { name: "Import finished" });
  await vi.waitFor(() => expect(fetched).toEqual({ types: 2, settings: 2, me: 1, meta: 1 }));
});

test("a finished dry run or failed import refetches nothing else", async () => {
  for (const finished of [
    run({ job_id: JOB, dry_run: true }),
    run({ job_id: JOB, state: "failed", error: "boom" }),
  ]) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const fetched = observe(client, KEYS);
    await vi.waitFor(() => expect(fetched.types).toBe(1));
    stubRoutes({
      "/api/v1/import/runs": () => json({ items: [finished] }),
      [`/api/v1/import/jobs/${JOB}`]: () =>
        json({ job_id: JOB, status: "done", attempts: 1, run: finished }),
    });
    renderWithRouter(<Harness initial={JOB} />, client);
    await screen.findByRole("heading", { name: /Import (finished|failed)/ });
    // The runs list refreshes (the job settled), the rest does not.
    await vi.waitFor(() => expect(client.getQueryState(qk.import.runs)?.dataUpdateCount).toBe(2));
    expect(fetched).toEqual({ types: 1, settings: 1, me: 1, meta: 1 });
    cleanup();
  }
});

test("a busy start offers the running import, never following it on its own", async () => {
  stubXhr(409, { type: "/api/v1/problems/conflict", title: "c", status: 409, detail: "" });
  let runsCalls = 0;
  stubRoutes({
    "/api/v1/import/runs": () => json({ items: runsCalls++ === 0 ? [] : [running()] }),
    [`/api/v1/import/jobs/${JOB}`]: () =>
      json({ job_id: JOB, status: "running", attempts: 1, run: running() }),
  });
  const onJob = vi.fn();
  renderWithRouter(<Harness onJob={onJob} />);
  await chooseAndStart();
  const show = await screen.findByRole("button", { name: "Show the running import" });
  expect(onJob).not.toHaveBeenCalled();
  await userEvent.click(show);
  expect(await screen.findByRole("heading", { name: "Importing" })).toBeInTheDocument();
});

test("a busy start without a running run links to the queued import jobs", async () => {
  stubXhr(409, { type: "/api/v1/problems/conflict", title: "c", status: 409, detail: "" });
  stubRoutes({ "/api/v1/import/runs": () => json({ items: [] }) });
  renderWithRouter(<Harness />);
  await chooseAndStart();
  const link = await screen.findByRole("link", { name: "See the queued import jobs" });
  expect(link.getAttribute("href")).toBe("/admin/system/jobs?status=queued&kind=import.wordpress");
});

test("the run's live region is in the page before the start, then says the phase", async () => {
  stubXhr(202, { job_id: JOB, status: "queued", attempts: 0 });
  stubRoutes({
    "/api/v1/import/runs": () => json({ items: [] }),
    [`/api/v1/import/jobs/${JOB}`]: () => json({ job_id: JOB, status: "queued", attempts: 0 }),
  });
  renderWithRouter(<Harness />);
  // The start puts the job in the cache, so the panel mounts knowing its phase: the
  // region must already be there, empty, for that first phase to be a change.
  await screen.findByRole("button", { name: "Start import" });
  const live = document.querySelector<HTMLElement>("[data-slot=import-run-live]")!;
  expect(live).toHaveAttribute("aria-live", "polite");
  expect(live).toBeEmptyDOMElement();
  await chooseAndStart();
  await screen.findByRole("heading", { name: "Waiting for a worker" });
  await vi.waitFor(() => expect(live).toHaveTextContent("Import queued."));
  expect(document.querySelector("[data-slot=import-run-live]")).toBe(live);
});

test("a job already cached at load is said once the region has settled", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const queued = { job_id: JOB, status: "queued" as const, attempts: 0 };
  client.setQueryData(qk.import.job(JOB), queued);
  stubRoutes({
    "/api/v1/import/runs": () => json({ items: [] }),
    [`/api/v1/import/jobs/${JOB}`]: () => json(queued),
  });
  renderWithRouter(<Harness initial={JOB} />, client);
  // The panel mounts with its phase in the same commit as the screen: the region is in
  // the page, still empty right after, and speaks a moment later in the same node.
  await screen.findByRole("heading", { name: "Waiting for a worker" });
  const live = document.querySelector<HTMLElement>("[data-slot=import-run-live]")!;
  expect(live).toBeEmptyDOMElement();
  await vi.waitFor(() => expect(live).toHaveTextContent("Import queued."));
  expect(document.querySelector("[data-slot=import-run-live]")).toBe(live);
});
