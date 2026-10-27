import { getCsrfToken } from "./csrf";
import { reportApiError } from "./client";
import type { MediaDto } from "./media";
import type { PermissionReportDto, PluginDto, PluginFiles } from "./plugins";
import type { ThemeDto } from "./themes";
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

interface SendOptions {
  url: string;
  headers: Record<string, string>;
  body: XMLHttpRequestBodyInit;
  onProgress(loaded: number, total: number): void;
  signal: AbortSignal;
}

/**
 * A `POST` over `XMLHttpRequest` (`fetch` has no upload progress) with the session's
 * CSRF token. A problem response rejects with `ApiError` (a `401` also goes to the global
 * handler, as for the client); no response rejects with `UploadNetworkError`; `signal`
 * aborting rejects with an `AbortError`.
 */
function send<T>(opts: SendOptions): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    if (opts.signal.aborted) {
      reject(abortError());
      return;
    }
    const xhr = new XMLHttpRequest();
    xhr.open("POST", opts.url);
    xhr.withCredentials = true;
    xhr.setRequestHeader("Accept", "application/json");
    const csrf = getCsrfToken();
    if (csrf !== null) xhr.setRequestHeader("X-CSRF-Token", csrf);
    for (const [name, value] of Object.entries(opts.headers)) xhr.setRequestHeader(name, value);

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
          resolve(JSON.parse(xhr.responseText) as T);
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
    xhr.send(opts.body);
  });
}

/**
 * `POST /api/v1/media` (see `send`): parts `title`, `alt`, then `file` last, with the
 * file's `Idempotency-Key`.
 */
export function uploadMedia(file: File, opts: UploadOptions): Promise<MediaDto> {
  const form = new FormData();
  if (opts.title) form.append("title", opts.title);
  if (opts.alt) form.append("alt", opts.alt);
  form.append("file", file, file.name);
  return send<MediaDto>({
    url: "/api/v1/media",
    headers: { "Idempotency-Key": opts.key },
    body: form,
    onProgress: (loaded, total) => {
      opts.onProgress(loaded, total);
    },
    signal: opts.signal,
  });
}

/**
 * `POST /api/v1/themes` (see `send`): the raw bundle as `application/x-tar`, installed
 * without activating (a theme of the same name is replaced; an active one goes live at
 * once, H-T2). A theme that does not load is a `422` with one `errors[]` entry per
 * problem.
 */
export function uploadTheme(
  tar: Uint8Array | Blob,
  opts: Pick<UploadOptions, "onProgress" | "signal">,
): Promise<ThemeDto> {
  const body = tar instanceof Blob ? tar : new Blob([tar as Uint8Array<ArrayBuffer>]);
  return send<ThemeDto>({
    url: "/api/v1/themes?activate=false",
    headers: { "Content-Type": "application/x-tar" },
    body,
    onProgress: (loaded, total) => {
      opts.onProgress(loaded, total);
    },
    signal: opts.signal,
  });
}

function pluginParts(files: PluginFiles, approve?: string): FormData {
  const form = new FormData();
  form.append("manifest", files.manifest, fileName(files.manifest, "ferrite-plugin.toml"));
  form.append("component", files.component, fileName(files.component, "plugin.wasm"));
  if (approve !== undefined) form.append("approve", approve);
  return form;
}

function fileName(blob: Blob, fallback: string): string {
  return blob instanceof File && blob.name ? blob.name : fallback;
}

/**
 * `POST /api/v1/plugins/inspect` (see `send`): the two parts, `manifest` and
 * `component`; the permission report, nothing stored.
 */
export function inspectPluginUpload(
  files: PluginFiles,
  opts: Pick<UploadOptions, "onProgress" | "signal">,
): Promise<PermissionReportDto> {
  return send<PermissionReportDto>({
    url: "/api/v1/plugins/inspect",
    headers: {},
    body: pluginParts(files),
    onProgress: (loaded, total) => {
      opts.onProgress(loaded, total);
    },
    signal: opts.signal,
  });
}

/**
 * `POST /api/v1/plugins` (see `send`): the same two parts plus `approve`, the inspected
 * report's fingerprint. Files changed since: `422` field `approve` with the new `report`
 * in the problem. A new plugin arrives disabled.
 */
export function installPluginUpload(
  files: PluginFiles,
  approve: string,
  opts: Pick<UploadOptions, "onProgress" | "signal">,
): Promise<PluginDto> {
  return send<PluginDto>({
    url: "/api/v1/plugins",
    headers: {},
    body: pluginParts(files, approve),
    onProgress: (loaded, total) => {
      opts.onProgress(loaded, total);
    },
    signal: opts.signal,
  });
}
