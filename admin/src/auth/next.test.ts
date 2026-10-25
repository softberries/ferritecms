import { describe, expect, it } from "vitest";
import { DEFAULT_NEXT, routerHref, safeNext } from "./next";

describe("safe_next_table", () => {
  const accepted = [
    "/admin/",
    "/admin/content/post?status=draft",
    "/admin/content/post/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
    "/admin/_styleguide?section=colour",
    "/admin/change-password",
  ];
  const refused = [
    "//evil.com",
    "//evil.com/admin/",
    "/\\evil",
    "/admin/\\..\\api",
    "https://evil",
    "https://evil.example/admin/",
    `${globalThis.location.origin}/admin/`,
    "/admin/../api/v1",
    "/admin/%2e%2e/x",
    "/admin/%2E%2E/api/v1",
    "/admin/%252e%252e/x",
    "/admin/./x",
    "/%2fevil.com",
    "/%5cevil.com",
    "javascript:alert(1)",
    "JavaScript:alert(1)",
    " /admin/",
    "/admin/\tx",
    "/api/v1/auth/logout",
    "/admin",
    "/administrator/",
    "/admin/login",
    "/admin/login?next=/admin/",
    "/admin/login/totp",
    "/admin/%zz",
    "admin/",
    "",
  ];

  it.each(accepted)("accepts %s", (raw) => {
    expect(safeNext(raw)).toBe(raw);
  });

  it.each(refused)("refuses %j", (raw) => {
    expect(safeNext(raw)).toBe(DEFAULT_NEXT);
  });

  it("refuses null and undefined", () => {
    expect(safeNext(null)).toBe(DEFAULT_NEXT);
    expect(safeNext(undefined)).toBe(DEFAULT_NEXT);
  });

  it("maps to a router href under the basepath", () => {
    expect(routerHref("/admin/")).toBe("/");
    expect(routerHref("/admin/content/post?status=draft")).toBe("/content/post?status=draft");
  });
});
