import { describe, expect, test } from "vitest";
import { ApiError } from "@/api/problem";
import { UploadNetworkError } from "@/api/upload";
import { uploadFailure } from "./upload-errors";

const problem = (status: number, slug: string, detail = `${slug} detail`) =>
  new ApiError({ type: `https://ferritecms.dev/problems/${slug}`, title: slug, status, detail });

describe("upload_failure", () => {
  test("transient and unknown failures offer Retry", () => {
    expect(uploadFailure(new UploadNetworkError())).toEqual({ kind: "network", retry: true });
    expect(uploadFailure(problem(503, "busy"))).toMatchObject({ kind: "busy", retry: true });
    expect(uploadFailure(problem(429, "rate-limited"))).toMatchObject({
      kind: "busy",
      retry: true,
    });
    expect(uploadFailure(problem(500, "internal"))).toMatchObject({ kind: "server", retry: true });
    expect(uploadFailure(problem(502, "x"))).toMatchObject({ kind: "server", retry: true });
    expect(uploadFailure(problem(400, "validation"))).toMatchObject({
      kind: "unknown",
      retry: true,
    });
    expect(uploadFailure(new Error("odd"))).toEqual({ kind: "unknown", retry: true });
  });

  test("refusals do not, and keep the server's detail", () => {
    expect(uploadFailure(problem(415, "unsupported-media-type", "no exe"))).toEqual({
      kind: "type",
      retry: false,
      detail: "no exe",
    });
    expect(uploadFailure(problem(413, "too-large"))).toMatchObject({ kind: "size", retry: false });
    expect(uploadFailure(problem(422, "validation"))).toMatchObject({
      kind: "invalid",
      retry: false,
    });
    expect(uploadFailure(problem(403, "forbidden"))).toMatchObject({
      kind: "forbidden",
      retry: false,
    });
  });
});
