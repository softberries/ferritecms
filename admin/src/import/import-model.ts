// What the import screen derives from the API: a job's phase (what the run panel says),
// each step's state in the stepper, the counts' totals, a stalled run, and the check of
// the category and tag bases the server makes (`check_import_options`).

import {
  IMPORT_STEPS,
  type ImportCountsDto,
  type ImportJobDto,
  type ImportRunDto,
  type ImportStep,
} from "@/api/import";

/**
 * What a job is doing, for people: `queued` (no node took it yet), `running`,
 * `retrying` (an attempt failed and the job waits for the next), `done`, `failed` (the
 * job is dead, or it finished with a failed or abandoned run).
 */
export type ImportPhase = "queued" | "running" | "retrying" | "done" | "failed";

export function importPhase(job: ImportJobDto): ImportPhase {
  const run = job.run ?? null;
  switch (job.status) {
    case "dead":
      return "failed";
    case "done":
      return run && run.state !== "done" ? "failed" : "done";
    case "running":
      return "running";
    default:
      return job.attempts > 0 ? "retrying" : "queued";
  }
}

export type StepState = "done" | "current" | "failed" | "pending";

/** The stepper's steps: the run's step is current (failed when the run failed). */
export function stepStates(
  run: ImportRunDto | null | undefined,
  phase: ImportPhase,
): { step: ImportStep; state: StepState }[] {
  const at = run?.step ? IMPORT_STEPS.indexOf(run.step as ImportStep) : -1;
  return IMPORT_STEPS.map((step, i) => {
    let state: StepState;
    if (phase === "done") state = "done";
    else if (at < 0) state = "pending";
    else if (i < at) state = "done";
    else if (i > at) state = "pending";
    else state = phase === "failed" ? "failed" : "current";
    return { step, state };
  });
}

/** The counts' columns, in the order the screen shows them. */
export const COUNT_COLUMNS = [
  "created",
  "updated",
  "unchanged",
  "kept",
  "skipped",
  "failed",
] as const satisfies readonly (keyof ImportCountsDto)[];
export type CountColumn = (typeof COUNT_COLUMNS)[number];

/** Each column summed over every kind. */
export function countTotals(counts: Record<string, ImportCountsDto>): ImportCountsDto {
  const out: ImportCountsDto = {
    created: 0,
    updated: 0,
    unchanged: 0,
    kept: 0,
    skipped: 0,
    failed: 0,
  };
  for (const c of Object.values(counts)) {
    for (const col of COUNT_COLUMNS) out[col] += c[col];
  }
  return out;
}

/** Kinds the importer counts before and after the content types (`post`, `page`, …). */
const FIRST_KINDS = ["users", "types", "taxonomies", "terms", "media", "post", "page"];
const LAST_KINDS = ["comments", "menus", "redirects", "options", "fixups"];
/** The kinds a label exists for (anything else is a content type's name). */
export const KNOWN_KINDS = [...FIRST_KINDS, ...LAST_KINDS] as const;

/** The kinds the run counted in the order of the passes; other content types by name. */
export function orderedKinds(counts: Record<string, ImportCountsDto>): string[] {
  const rank = (k: string) => {
    const first = FIRST_KINDS.indexOf(k);
    if (first >= 0) return first;
    const last = LAST_KINDS.indexOf(k);
    return last >= 0 ? FIRST_KINDS.length + 1 + last : FIRST_KINDS.length;
  };
  return Object.keys(counts).sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
}

/** A `running` run without a heartbeat for this long has stopped (08 §2). */
export const STALE_AFTER_MS = 120_000;

export function stalled(run: ImportRunDto | null | undefined, now: number = Date.now()): boolean {
  if (run?.state !== "running") return false;
  const beat = Date.parse(run.heartbeat_at);
  return !Number.isNaN(beat) && now - beat > STALE_AFTER_MS;
}

/** The longest category or tag base (`MAX_IMPORT_BASE`). */
export const MAX_BASE = 100;

/**
 * A category or tag base the server takes: a path of segments of `A-Z a-z 0-9 - _ . ~`
 * (surrounding slashes ignored), at most `MAX_BASE` characters, no empty, `.` or `..`
 * segment.
 */
export function baseIsValid(base: string): boolean {
  if (base.length > MAX_BASE) return false;
  const path = base.replace(/^\/+|\/+$/g, "");
  return path
    .split("/")
    .every((s) => s !== "" && s !== "." && s !== ".." && /^[A-Za-z0-9._~-]+$/.test(s));
}
