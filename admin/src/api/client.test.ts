import { MutationObserver, type QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { confirmReset, login } from "./auth";
import {
  NetworkError,
  createQueryClient,
  currentPath,
  shouldRetry,
  type ErrorRouting,
} from "./client";
import { getContent, ifMatch, lifecycle, patchContent } from "./content";
import { getCsrfToken, setCsrfToken } from "./csrf";
import { ApiError, isProblem } from "./problem";

const ITEM = { id: "0b0e2c43-6c0e-4b8f-9f39-1f2a3b4c5d6e", version: 3 };

function problem(status: number, slug: string, extra: Record<string, unknown> = {}, headers = {}) {
  return new Response(
    JSON.stringify({ type: `/api/v1/problems/${slug}`, title: slug, status, detail: "", ...extra }),
    { status, headers: { "Content-Type": "application/problem+json", ...headers } },
  );
}

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** Stubs `fetch` with `answer` and records every request it gets. */
function stubFetch(answer: (r: Request) => Response | Promise<Response>) {
  const requests: Request[] = [];
  const fetch = vi.fn((r: Request) => {
    requests.push(r);
    return Promise.resolve(answer(r));
  });
  vi.stubGlobal("fetch", fetch);
  return { requests, fetch };
}

/** Spy routing for the error handler; the current path is the real one. */
function routing() {
  const navigate = vi.fn<(href: string) => void>();
  const notify = vi.fn<(id: string, message: string) => void>();
  const r: ErrorRouting = { navigate, notify, currentPath };
  return { r, navigate, notify };
}

/** Runs `fn` as a mutation of `client` (so the mutation cache's handlers see it). */
function mutate(client: QueryClient, fn: () => Promise<unknown>) {
  return new MutationObserver(client, { mutationFn: fn }).mutate().catch(() => {});
}

beforeEach(() => {
  setCsrfToken(null);
  window.history.replaceState(null, "", "/admin/");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

test("csrf_header_on_unsafe_only", async () => {
  const { requests } = stubFetch(() => json(ITEM));
  setCsrfToken("tok-1");
  await getContent("post", ITEM.id);
  await patchContent("post", ITEM.id, 3, { title: "x" });
  await lifecycle("post", ITEM.id, "publish", 3);

  const [get, patch, publish] = requests;
  expect(get?.method).toBe("GET");
  expect(get?.headers.get("X-CSRF-Token")).toBeNull();
  expect(patch?.method).toBe("PATCH");
  expect(patch?.headers.get("X-CSRF-Token")).toBe("tok-1");
  expect(publish?.method).toBe("POST");
  expect(publish?.headers.get("X-CSRF-Token")).toBe("tok-1");
  for (const r of requests) {
    expect(r.headers.get("Accept")).toBe("application/json");
    expect(r.credentials).toBe("same-origin");
    expect(new URL(r.url).pathname.startsWith("/api/v1/content/post/")).toBe(true);
  }
});

test("problem_becomes_api_error", async () => {
  stubFetch(() =>
    problem(
      422,
      "validation",
      {
        detail: "the input is invalid",
        errors: [
          { field: "title", detail: "must not be empty" },
          { field: "title", detail: "second message is ignored" },
          { field: "slug", detail: "is taken" },
          { field: null, detail: "no field" },
        ],
      },
      { "Retry-After": "7" },
    ),
  );
  const error = await patchContent("post", ITEM.id, 3, { title: "" }).catch((e: unknown) => e);

  expect(error).toBeInstanceOf(ApiError);
  const e = error as ApiError;
  expect(e.status).toBe(422);
  expect(e.slug).toBe("validation");
  expect(e.message).toBe("the input is invalid");
  expect(e.fieldErrors()).toEqual({ title: "must not be empty", slug: "is taken" });
  expect(e.retryAfter).toBe(7);
  expect(isProblem(e, "validation")).toBe(true);
  expect(isProblem(e, "conflict")).toBe(false);
  expect(isProblem(new Error("x"), "validation")).toBe(false);
});

test("non_problem_failure_is_api_error", async () => {
  stubFetch(
    () => new Response("<html>bad gateway</html>", { status: 502, statusText: "Bad Gateway" }),
  );
  const error = await getContent("post", ITEM.id).catch((e: unknown) => e);
  expect(error).toBeInstanceOf(ApiError);
  expect((error as ApiError).status).toBe(502);
  expect((error as ApiError).retryAfter).toBeNull();
});

test("if_match_quotes_version", async () => {
  expect(ifMatch(12)).toEqual({ "If-Match": '"12"' });
  const { requests } = stubFetch(() => json(ITEM));
  await patchContent("post", ITEM.id, 12, { title: "x" });
  await lifecycle("post", ITEM.id, "schedule", 13, { at: "2030-01-01T00:00:00Z" });
  await lifecycle("post", ITEM.id, "trash", 14);
  expect(requests.map((r) => [r.method, r.headers.get("If-Match")])).toEqual([
    ["PATCH", '"12"'],
    ["POST", '"13"'],
    ["DELETE", '"14"'],
  ]);
});

test("unauthenticated_redirects_with_next", async () => {
  window.history.replaceState(null, "", "/admin/content/post?status=draft#frag");
  stubFetch(() => problem(401, "unauthenticated"));
  setCsrfToken("tok-1");
  const { r, navigate } = routing();
  const client = createQueryClient(r);

  const results = await Promise.allSettled([
    client.fetchQuery({ queryKey: ["a"], queryFn: () => getContent("post", "a") }),
    client.fetchQuery({ queryKey: ["b"], queryFn: () => getContent("post", "b") }),
    client.fetchQuery({ queryKey: ["c"], queryFn: () => getContent("post", "c") }),
  ]);

  expect(results.map((x) => x.status)).toEqual(["rejected", "rejected", "rejected"]);
  // Once for the three parallel failures; the fragment never goes into `next`.
  expect(navigate).toHaveBeenCalledTimes(1);
  expect(navigate).toHaveBeenCalledWith(
    `/login?next=${encodeURIComponent("/admin/content/post?status=draft")}`,
  );
  expect(getCsrfToken()).toBeNull();
});

test("unauthenticated_on_sign_in_pages_stays", async () => {
  window.history.replaceState(null, "", "/admin/login?next=%2Fadmin%2F");
  stubFetch(() => problem(401, "unauthenticated"));
  const { r, navigate } = routing();
  const client = createQueryClient(r);
  await client
    .fetchQuery({ queryKey: ["me"], queryFn: () => getContent("post", "a") })
    .catch(() => {});
  expect(navigate).not.toHaveBeenCalled();
});

test("redirects_again_after_a_success", async () => {
  let status = 401;
  stubFetch(() => (status === 200 ? json(ITEM) : problem(401, "unauthenticated")));
  const { r, navigate } = routing();
  const client = createQueryClient(r);
  const fail = (k: string) =>
    client.fetchQuery({ queryKey: [k], queryFn: () => getContent("post", k) }).catch(() => {});

  await fail("a");
  status = 200;
  await client.fetchQuery({ queryKey: ["ok"], queryFn: () => getContent("post", "ok") });
  status = 401;
  await fail("b");
  expect(navigate).toHaveBeenCalledTimes(2);
});

test("password_reset_required_goes_to_change_password", async () => {
  stubFetch(() => problem(403, "password-reset-required"));
  const { r, navigate } = routing();
  const client = createQueryClient(r);
  await client
    .fetchQuery({ queryKey: ["x"], queryFn: () => getContent("post", "x") })
    .catch(() => {});
  expect(navigate).toHaveBeenCalledWith("/change-password");
});

test("rate_limited_and_busy_toast_retry_after", async () => {
  let answer = () => problem(429, "rate-limited", {}, { "Retry-After": "42" });
  stubFetch(() => answer());
  const { r, navigate, notify } = routing();
  const client = createQueryClient(r);
  await mutate(client, () => patchContent("post", "x", 1, {}));
  expect(notify).toHaveBeenLastCalledWith("rate-limited", "Too many requests. Try again in 42 s.");

  answer = () => problem(503, "busy");
  await mutate(client, () => patchContent("post", "x", 1, {}));
  expect(notify).toHaveBeenLastCalledWith("busy", "The server is busy. Try again in a moment.");
  expect(navigate).not.toHaveBeenCalled();
});

test("no_retry_on_4xx", async () => {
  const { fetch } = stubFetch(() => problem(404, "not-found"));
  const client = createQueryClient(routing().r);
  await client
    .fetchQuery({ queryKey: ["404"], queryFn: () => getContent("post", "x") })
    .catch(() => {});
  expect(fetch).toHaveBeenCalledTimes(1);

  // `503 busy` is retried twice, after the server's `Retry-After`.
  fetch.mockClear();
  fetch.mockImplementation(() => Promise.resolve(problem(503, "busy", {}, { "Retry-After": "0" })));
  await client
    .fetchQuery({ queryKey: ["503"], queryFn: () => getContent("post", "x") })
    .catch(() => {});
  expect(fetch).toHaveBeenCalledTimes(3);

  // Mutations are never retried.
  fetch.mockClear();
  await mutate(client, () => patchContent("post", "x", 1, {}));
  expect(fetch).toHaveBeenCalledTimes(1);

  const e = (status: number, slug: string) =>
    new ApiError({ type: `/api/v1/problems/${slug}`, title: slug, status, detail: "" });
  expect(shouldRetry(0, e(400, "bad-request"))).toBe(false);
  expect(shouldRetry(0, e(401, "unauthenticated"))).toBe(false);
  expect(shouldRetry(0, e(412, "precondition-failed"))).toBe(false);
  expect(shouldRetry(0, e(429, "rate-limited"))).toBe(false);
  expect(shouldRetry(0, e(500, "internal"))).toBe(false);
  const offline = new NetworkError(new TypeError("Failed to fetch"));
  expect(shouldRetry(0, offline)).toBe(true);
  expect(shouldRetry(1, offline)).toBe(true);
  expect(shouldRetry(2, offline)).toBe(false);
  expect(shouldRetry(0, new TypeError("x is undefined"))).toBe(false);
  expect(shouldRetry(1, e(503, "busy"))).toBe(true);
  expect(shouldRetry(2, e(503, "busy"))).toBe(false);
});

test("only_fetch_failures_are_network_errors", async () => {
  // A rejected `fetch` becomes a `NetworkError` (retried) ...
  stubFetch(() => Promise.reject(new TypeError("Failed to fetch")));
  const error = await getContent("post", "x").catch((e: unknown) => e);
  expect(error).toBeInstanceOf(NetworkError);
  expect((error as NetworkError).cause).toBeInstanceOf(TypeError);
  expect(shouldRetry(0, error)).toBe(true);

  // ... while a `TypeError` from the code in a `queryFn` is not retried.
  const client = createQueryClient(routing().r);
  const queryFn = vi.fn(() => {
    throw new TypeError("Cannot read properties of undefined");
  });
  await client.fetchQuery({ queryKey: ["bug"], queryFn }).catch(() => {});
  expect(queryFn).toHaveBeenCalledTimes(1);
});

test("login_rearms_the_redirect", async () => {
  let answer: () => Response = () => problem(401, "unauthenticated");
  stubFetch(() => answer());
  const { r, navigate } = routing();
  const client = createQueryClient(r);
  const fail = (k: string) =>
    client.fetchQuery({ queryKey: [k], queryFn: () => getContent("post", k) }).catch(() => {});

  await fail("a");
  await fail("b");
  expect(navigate).toHaveBeenCalledTimes(1);

  // A direct `login()` (outside the query cache) starts a session and re-arms it.
  answer = () => json({ status: "session", csrf_token: "tok-2", user: {} });
  await login({ login: "me", password: "pw" });
  expect(getCsrfToken()).toBe("tok-2");

  answer = () => problem(401, "unauthenticated");
  await fail("c");
  expect(navigate).toHaveBeenCalledTimes(2);
});

test("confirm_reset_drops_the_csrf_token", async () => {
  stubFetch(() => new Response(null, { status: 204 }));
  setCsrfToken("tok-1");
  await confirmReset({ token: "t", password: "a long new password" });
  expect(getCsrfToken()).toBeNull();
});
