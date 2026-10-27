import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { setCsrfToken } from "./csrf";
import { ApiError } from "./problem";
import { UploadNetworkError, uploadMedia, uploadTheme } from "./upload";

class FakeXhr {
  static last: FakeXhr;
  method = "";
  url = "";
  withCredentials = false;
  headers: Record<string, string> = {};
  body: FormData | Blob | null = null;
  status = 0;
  statusText = "";
  responseText = "";
  aborted = false;
  responseHeaders: Record<string, string> = {};
  upload: { onprogress: ((e: ProgressEvent) => void) | null } = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  constructor() {
    FakeXhr.last = this;
  }
  open(method: string, url: string) {
    this.method = method;
    this.url = url;
  }
  setRequestHeader(k: string, v: string) {
    this.headers[k] = v;
  }
  getResponseHeader(k: string) {
    return this.responseHeaders[k] ?? this.responseHeaders[k.toLowerCase()] ?? null;
  }
  send(body: FormData | Blob) {
    this.body = body;
  }
  abort() {
    this.aborted = true;
    this.onabort?.();
  }
  respond(status: number, body: unknown, headers: Record<string, string> = {}) {
    this.status = status;
    this.responseText = JSON.stringify(body);
    this.responseHeaders = headers;
    this.onload?.();
  }
}

const FILE = new File(["abc"], "a.png", { type: "image/png" });
const noop = () => {};

function start(extra: Partial<Parameters<typeof uploadMedia>[1]> = {}) {
  const ctl = new AbortController();
  const promise = uploadMedia(FILE, {
    key: "key-1",
    title: "T",
    alt: "A",
    onProgress: noop,
    signal: ctl.signal,
    ...extra,
  });
  return { promise, ctl, xhr: FakeXhr.last };
}

beforeEach(() => {
  vi.stubGlobal("XMLHttpRequest", FakeXhr);
  setCsrfToken("tok-9");
});
afterEach(() => {
  vi.unstubAllGlobals();
  setCsrfToken(null);
});

test("sends_headers_and_multipart_with_file_last", async () => {
  const { promise, xhr } = start();
  expect(xhr.method).toBe("POST");
  expect(xhr.url).toBe("/api/v1/media");
  expect(xhr.withCredentials).toBe(true);
  expect(xhr.headers["X-CSRF-Token"]).toBe("tok-9");
  expect(xhr.headers["Idempotency-Key"]).toBe("key-1");
  expect(xhr.headers.Accept).toBe("application/json");
  const form = xhr.body as FormData;
  expect([...form.keys()]).toEqual(["title", "alt", "file"]);
  expect(form.get("title")).toBe("T");
  xhr.respond(201, { id: "m1" });
  await expect(promise).resolves.toEqual({ id: "m1" });
});

test("omits_blank_title_and_alt", async () => {
  const { promise, xhr } = start({ title: undefined, alt: undefined });
  expect([...(xhr.body as FormData).keys()]).toEqual(["file"]);
  xhr.respond(201, {});
  await promise;
});

test("progress_is_reported", async () => {
  const onProgress = vi.fn();
  const { promise, xhr } = start({ onProgress });
  xhr.upload.onprogress?.({ lengthComputable: true, loaded: 10, total: 30 } as ProgressEvent);
  xhr.upload.onprogress?.({ lengthComputable: false, loaded: 5, total: 0 } as ProgressEvent);
  expect(onProgress).toHaveBeenCalledTimes(1);
  expect(onProgress).toHaveBeenCalledWith(10, 30);
  xhr.respond(201, {});
  await promise;
});

test("problem_415_is_an_api_error_with_slug", async () => {
  const { promise, xhr } = start();
  xhr.respond(
    415,
    { type: "/api/v1/problems/unsupported-media-type", title: "u", status: 415, detail: "no" },
    { "Content-Type": "application/problem+json" },
  );
  const e = await promise.catch((x: unknown) => x);
  expect(e).toBeInstanceOf(ApiError);
  expect((e as ApiError).slug).toBe("unsupported-media-type");
  expect((e as ApiError).status).toBe(415);
});

test("503_keeps_retry_after", async () => {
  const { promise, xhr } = start();
  xhr.respond(
    503,
    { type: "/api/v1/problems/busy", title: "b", status: 503, detail: "" },
    { "Content-Type": "application/problem+json", "Retry-After": "7" },
  );
  const e = (await promise.catch((x: unknown) => x)) as ApiError;
  expect(e.slug).toBe("busy");
  expect(e.retryAfter).toBe(7);
});

test("non_problem_failure_is_an_api_error", async () => {
  const { promise, xhr } = start();
  xhr.respond(502, "<html>", {});
  const e = (await promise.catch((x: unknown) => x)) as ApiError;
  expect(e).toBeInstanceOf(ApiError);
  expect(e.status).toBe(502);
});

test("abort_rejects_with_abort_error", async () => {
  const { promise, ctl, xhr } = start();
  ctl.abort();
  expect(xhr.aborted).toBe(true);
  const e = await promise.catch((x: unknown) => x);
  expect(e).toBeInstanceOf(DOMException);
  expect((e as DOMException).name).toBe("AbortError");
});

test("already_aborted_never_sends", async () => {
  const ctl = new AbortController();
  ctl.abort();
  await expect(
    uploadMedia(FILE, { key: "k", onProgress: noop, signal: ctl.signal }),
  ).rejects.toMatchObject({ name: "AbortError" });
});

test("network_error_rejects_with_upload_network_error", async () => {
  const { promise, xhr } = start();
  xhr.onerror?.();
  await expect(promise).rejects.toBeInstanceOf(UploadNetworkError);
});

test("theme_upload_sends_the_raw_tar", async () => {
  const onProgress = vi.fn();
  const tar = new Uint8Array([1, 2, 3]);
  const promise = uploadTheme(tar, { onProgress, signal: new AbortController().signal });
  const xhr = FakeXhr.last;
  expect(xhr.method).toBe("POST");
  expect(xhr.url).toBe("/api/v1/themes?activate=false");
  expect(xhr.headers["Content-Type"]).toBe("application/x-tar");
  expect(xhr.headers["X-CSRF-Token"]).toBe("tok-9");
  expect(xhr.headers["Idempotency-Key"]).toBeUndefined();
  expect(xhr.body).toBeInstanceOf(Blob);
  expect((xhr.body as Blob).size).toBe(3);
  xhr.upload.onprogress?.({ lengthComputable: true, loaded: 3, total: 3 } as ProgressEvent);
  expect(onProgress).toHaveBeenCalledWith(3, 3);
  xhr.respond(201, { name: "t" });
  await expect(promise).resolves.toEqual({ name: "t" });
});

test("theme_upload_422_lists_problems", async () => {
  const promise = uploadTheme(new Blob(["x"]), {
    onProgress: noop,
    signal: new AbortController().signal,
  });
  FakeXhr.last.respond(
    422,
    {
      type: "/api/v1/problems/validation",
      title: "v",
      status: 422,
      detail: "the theme does not load",
      errors: [{ detail: "templates/index.html:3: syntax error" }],
    },
    { "Content-Type": "application/problem+json" },
  );
  const e = (await promise.catch((x: unknown) => x)) as ApiError;
  expect(e).toBeInstanceOf(ApiError);
  expect(e.problem.errors?.[0]?.detail).toBe("templates/index.html:3: syntax error");
});
