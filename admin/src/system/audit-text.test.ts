import { expect, test } from "vitest";
import { i18n } from "@/i18n";
import {
  AUDIT_ACTIONS,
  auditActionText,
  auditAreaOf,
  detailEntries,
  isAuditPrefix,
  targetKindText,
} from "./audit-text";

const t = i18n.getFixedT(null, "admin");

test("audit_text_known_and_unknown", () => {
  // Known actions read as sentences.
  expect(auditActionText(t, "user.create")).toBe("Created a user");
  expect(auditActionText(t, "token.revoke")).toBe("Revoked an API token");
  expect(auditActionText(t, "user.totp_disable")).toBe("Turned off a user's two-factor sign-in");
  expect(auditActionText(t, "mail.test")).toBe("Sent a test e-mail");
  expect(auditActionText(t, "type.delete")).toBe("Deleted a content type");
  // Every known action has its own text (a missing key fails the test in setup).
  for (const action of AUDIT_ACTIONS) {
    const text = auditActionText(t, action);
    expect(text, action).not.toBe(action);
    expect(text.length, action).toBeGreaterThan(0);
  }
  expect(new Set(AUDIT_ACTIONS.map((a) => auditActionText(t, a))).size).toBe(AUDIT_ACTIONS.length);
  // Unknown ones (a newer node, a plugin's future action) show raw, never looked up.
  expect(auditActionText(t, "backup.create")).toBe("backup.create");
  expect(auditActionText(t, "user.levitate")).toBe("user.levitate");
  expect(auditActionText(t, "<img src=x onerror=alert(1)>")).toBe("<img src=x onerror=alert(1)>");
  expect(auditActionText(t, "constructor")).toBe("constructor");
});

test("audit_areas_and_targets", () => {
  expect(auditAreaOf("plugin.install")).toBe("plugin");
  expect(auditAreaOf("nothing")).toBeNull();
  expect(isAuditPrefix("user.")).toBe(true);
  expect(isAuditPrefix("user")).toBe(false);
  expect(isAuditPrefix("user.create")).toBe(false);
  expect(isAuditPrefix("User.")).toBe(false);
  expect(targetKindText(t, "api_token")).toBe("API token");
  expect(targetKindText(t, "spaceship")).toBe("spaceship");
});

test("details_are_text_pairs", () => {
  expect(
    detailEntries({
      name: "<b>bold</b>",
      keys: ["site.title", "site.tagline"],
      count: 3,
      purge: false,
      expires_at: null,
      nested: { a: 1 },
    }),
  ).toEqual([
    ["name", "<b>bold</b>"],
    ["keys", "site.title, site.tagline"],
    ["count", "3"],
    ["purge", "false"],
    ["expires_at", "—"],
    ["nested", '{"a":1}'],
  ]);
  expect(detailEntries({})).toEqual([]);
  expect(detailEntries(null)).toEqual([]);
  expect(detailEntries(["a"])).toEqual([["0", "a"]]);
});
