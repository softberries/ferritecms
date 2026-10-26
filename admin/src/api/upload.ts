import { getCsrfToken } from "./csrf";
import { reportApiError } from "./client";
import type { MediaDto } from "./media";
import { ApiError, parseRetryAfter, type Problem } from "./problem";

/** The upload got no response (offline, connection reset). */
export class UploadNetworkError extends Error {
  constructor() {
    super("upload failed: no response");
    this.name = "UploadNetworkError";
  }
}

export interface UploadOptions {
  /** `Idempotency-Key`: reuse it to retry the same upload without a duplicate. */
  key: string;
  title?: string | undefined;
  alt?: string | undefined;
  onProgress(loaded: number, total: number): void;
  signal: AbortSignal;
}

function abortError(): DOMException {
  return new DOMException("The upload was aborted", "AbortError");
}

function failure(xhr: XMLHttpRequest): ApiError {
  const retryAfter = parseRetryAfter(xhr.getResponseHeader("Retry-After"));
  const type = (xhr.getResponseHeader("Content-Type") ?? "").toLowerCase();
  if (type.startsWith("application/problem+json")) {
    try {
      const body = JSON.parse(xhr.responseText) as Partial<Problem> | null;
      if (
        body &&
        typeof body.type === "string" &&
        typeof body.status === "number" &&
        typeof body.title === "string" &&
        typeof body.detail === "string"
      ) {
        return new ApiError(body as Problem, retryAfter);
      }
    } catch {
      // falls through to the synthetic problem
    }
  }
  return new ApiError(
    {
      type: "about:blank",
      title: xhr.statusText || `HTTP ${xhr.status}`,
      status: xhr.status,
      detail: "",
    },
    retryAfter,
  );
}

/**
 * `POST /api/v1/media` over `XMLHttpRequest` (`fetch` has no upload progress). Parts
 * `title`, `alt`, then `file` last. A problem response rejects with `ApiError` (a `401`
 * also goes to the global handler, as for the client); no response rejects with
 * `UploadNetworkError`; `signal` aborting rejects with an `AbortError`.
 */
export function uploadMedia(file: File, opts: UploadOptions): Promise<MediaDto> {
  return new Promise<MediaDto>((resolve, reject) => {
    if (opts.signal.aborted) {
      reject(abortError());
      return;
    }
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "/api/v1/media");
    xhr.withCredentials = true;
    xhr.setRequestHeader("Accept", "application/json");
    const csrf = getCsrfToken();
    if (csrf !== null) xhr.setRequestHeader("X-CSRF-Token", csrf);
    xhr.setRequestHeader("Idempotency-Key", opts.key);

    const form = new FormData();
    if (opts.title) form.append("title", opts.title);
    if (opts.alt) form.append("alt", opts.alt);
    form.append("file", file, file.name);

    const onAbort = () => {
      xhr.abort();
    };
    opts.signal.addEventListener("abort", onAbort, { once: true });
    const done = () => {
      opts.signal.removeEventListener("abort", onAbort);
    };

    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) opts.onProgress(e.loaded, e.total);
    };
    xhr.onload = () => {
      done();
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          resolve(JSON.parse(xhr.responseText) as MediaDto);
        } catch {
          reject(failure(xhr));
        }
        return;
      }
      const error = failure(xhr);
      reportApiError(error);
      reject(error);
    };
    xhr.onerror = () => {
      done();
      reject(new UploadNetworkError());
    };
    xhr.onabort = () => {
      done();
      reject(abortError());
    };
    xhr.send(form);
  });
}
