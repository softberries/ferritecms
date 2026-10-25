// The shell under the real server, on both backends (`*.smoke.spec.ts` also runs in
// `chromium-pg`; pre-flight F25). Login contexts of this file use spec number 1
// (`forwardedFor(1, n)`, F24).

import { expect, test } from "./support/fixtures";
import { mailbox } from "./support/smtp-sink";
import { account, forwardedFor } from "./support/users";

test("admin_loads_under_csp", async ({ as }) => {
  const page = await as("admin");
  const res = await page.goto("/admin/");
  expect(res?.status()).toBe(200);
  const csp = res?.headers()["content-security-policy"] ?? "";
  expect(csp).toContain("script-src 'self'");
  expect(csp).not.toContain("unsafe-eval");
  expect(csp).not.toMatch(/script-src[^;]*'unsafe-inline'/);
  await expect(page.getByRole("heading", { level: 1, name: "Dashboard" })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Main" })).toBeVisible();
  // The guard fixture fails the test on any CSP violation, page error or console error.
});

test("assets_are_immutable", async ({ as }) => {
  const page = await as("admin");
  const cacheControl = new Map<string, string>();
  page.on("response", (r) =>
    cacheControl.set(new URL(r.url()).pathname, r.headers()["cache-control"] ?? ""),
  );
  await page.goto("/admin/");
  await page.waitForLoadState("networkidle");
  const src = await page.locator('script[type="module"][src]').first().getAttribute("src");
  expect(src).toMatch(/^\/admin\/assets\/.+\.js$/);
  expect(cacheControl.get(src ?? "")).toContain("immutable");
});

test("smtp_sink_receives_a_reset_mail", async ({ request, site }) => {
  // `resetuser`, never a storage-state account: a confirmed reset ends sessions (F23).
  const user = account("resetuser");
  const after = Date.now();
  const res = await request.post("/api/v1/auth/password-reset", {
    headers: { Origin: site.url, "X-Forwarded-For": forwardedFor(1, 1) },
    data: { login: user.login },
  });
  expect(res.status()).toBe(202);
  const mail = await mailbox(site.mailDir).waitFor(user.email, { after });
  expect(mail.text).toContain(`${site.url}/admin/reset-password#token=`);
});
