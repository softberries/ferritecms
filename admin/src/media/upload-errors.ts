// What a failed upload means to the person who dropped it: a kind with a plain sentence
// (`media:tray.reason.<kind>`), whether **Retry** can help, and the server's own detail
// as secondary text. Retry is offered for transient or unknown failures (no response,
// `503`/`429`, a `409` for a key still in progress, other `5xx`, anything unrecognised); a refusal of the file itself (`413`
// too large, `415` type, `422` invalid, `403` not allowed) would only fail again.

import { ApiError } from "@/api/problem";
import { UploadNetworkError } from "@/api/upload";

export type UploadFailureKind =
  "network" | "busy" | "server" | "type" | "size" | "invalid" | "forbidden" | "unknown";

export interface UploadFailure {
  kind: UploadFailureKind;
  retry: boolean;
  /** The server's detail (a field error on `file` first), when it sent one. */
  detail?: string;
}

const REFUSALS: Record<number, UploadFailureKind> = {
  403: "forbidden",
  413: "size",
  415: "type",
  422: "invalid",
};

export function uploadFailure(error: unknown): UploadFailure {
  if (error instanceof UploadNetworkError) return { kind: "network", retry: true };
  if (!(error instanceof ApiError)) return { kind: "unknown", retry: true };
  const text = error.fieldErrors().file ?? (error.problem.detail || error.problem.title);
  const detail = text ? { detail: text } : {};
  const refusal = REFUSALS[error.status];
  if (refusal) return { kind: refusal, retry: false, ...detail };
  // `409`: the same key's earlier request still runs (the queue waits it out).
  if (error.status === 503 || error.status === 429 || error.status === 409) {
    return { kind: "busy", retry: true, ...detail };
  }
  if (error.status >= 500) return { kind: "server", retry: true, ...detail };
  return { kind: "unknown", retry: true, ...detail };
}
