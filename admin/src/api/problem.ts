import type { SchemaFieldError, SchemaProblemDocument } from "./schema";

/** An RFC 9457 problem document as the API sends it (07 §1). */
export type Problem = SchemaProblemDocument;

/** The `Retry-After` header in whole seconds (delta-seconds or an HTTP date), else `null`. */
export function parseRetryAfter(value: string | null, now: number = Date.now()): number | null {
  if (value === null) return null;
  const trimmed = value.trim();
  if (/^\d+$/.test(trimmed)) return Number(trimmed);
  const at = Date.parse(trimmed);
  if (Number.isNaN(at)) return null;
  return Math.max(0, Math.ceil((at - now) / 1000));
}

/** A failed API call: every non-2xx response the client sees becomes one. */
export class ApiError extends Error {
  readonly status: number;
  readonly problem: Problem;
  /** The last segment of `problem.type`, e.g. `precondition-failed`. */
  readonly slug: string;
  /** Seconds from `Retry-After` (`rate-limited`, `busy`, `conflict`), else `null`. */
  readonly retryAfter: number | null;

  constructor(problem: Problem, retryAfter: number | null = null) {
    super(problem.detail || problem.title);
    this.name = "ApiError";
    this.status = problem.status;
    this.problem = problem;
    this.slug = problem.type.slice(problem.type.lastIndexOf("/") + 1);
    this.retryAfter = retryAfter;
  }

  /** One message per field of a `validation` problem (`errors[]`; the first per field wins). */
  fieldErrors(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const e of this.problem.errors ?? ([] as SchemaFieldError[])) {
      if (e.field != null && !(e.field in out)) out[e.field] = e.detail;
    }
    return out;
  }

  /** From a response that carries a problem document (`application/problem+json`). */
  static async fromProblemResponse(response: Response): Promise<ApiError> {
    const retryAfter = parseRetryAfter(response.headers.get("Retry-After"));
    let body: unknown;
    try {
      body = await response.clone().json();
    } catch {
      return new ApiError(syntheticProblem(response), retryAfter);
    }
    return new ApiError(isProblemDocument(body) ? body : syntheticProblem(response), retryAfter);
  }

  /** From a failed response without a problem document (e.g. a proxy's HTML error page). */
  static fromResponse(response: Response): ApiError {
    return new ApiError(
      syntheticProblem(response),
      parseRetryAfter(response.headers.get("Retry-After")),
    );
  }
}

/** `true` when `e` is an `ApiError` whose problem type ends in `slug`. */
export function isProblem(e: unknown, slug: string): e is ApiError {
  return e instanceof ApiError && e.slug === slug;
}

/** `true` for an object with a problem document's required members. */
export function isProblemDocument(v: unknown): v is Problem {
  if (typeof v !== "object" || v === null) return false;
  const p = v as Record<string, unknown>;
  return (
    typeof p.type === "string" &&
    typeof p.status === "number" &&
    typeof p.title === "string" &&
    typeof p.detail === "string"
  );
}

function syntheticProblem(response: Response): Problem {
  return {
    type: "about:blank",
    title: response.statusText || `HTTP ${response.status}`,
    status: response.status,
    detail: "",
  };
}
