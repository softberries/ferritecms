import { randomBytes } from "node:crypto";

/**
 * A unique, slug-safe name for content a test creates (E2E data hygiene: tests never
 * depend on another test's writes, and two workers share one site).
 */
export function uid(prefix = "e2e"): string {
  return `${prefix}-${Date.now().toString(36)}-${randomBytes(4).toString("hex")}`;
}
