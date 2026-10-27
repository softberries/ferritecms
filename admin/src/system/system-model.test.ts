import { expect, test } from "vitest";
import { i18n } from "@/i18n";
import {
  SKEW_LIMIT_MS,
  isSystemTab,
  parseSystemSearch,
  schemaVerdict,
  skewFlag,
  themeOriginText,
} from "./system-model";

const t = i18n.getFixedT(null, "admin");
const schema = (verdict: string, db: number, binary: number, pending: number[] = []) =>
  ({ verdict, db_version: db, binary_version: binary, pending }) as Parameters<
    typeof schemaVerdict
  >[1];

test("schema_verdict_wording", () => {
  const ok = schemaVerdict(t, schema("up_to_date", 8, 8));
  expect(ok.tone).toBe("ok");
  expect(ok.title).toBe("Up to date");
  expect(ok.body).toBe("The database is at version 8, the version this binary expects.");

  const one = schemaVerdict(t, schema("needs_up", 7, 8, [8]));
  expect(one.tone).toBe("warn");
  expect(one.title).toBe("1 migration to apply");
  expect(one.body).toBe(
    "The database is at version 7 and this binary expects 8. Apply it with ferrite migrate up.",
  );
  const two = schemaVerdict(t, schema("needs_up", 6, 8, [7, 8]));
  expect(two.title).toBe("2 migrations to apply");
  expect(two.body).toBe(
    "The database is at version 6 and this binary expects 8. Apply them with ferrite migrate up.",
  );

  const newer = schemaVerdict(t, schema("newer_schema_ok", 9, 8));
  expect(newer.tone).toBe("warn");
  expect(newer.title).toBe("Newer database, still compatible");
  expect(newer.body).toBe(
    "A newer FerriteCMS moved the database to version 9; this binary knows up to 8 and can still run on it. Upgrade this binary when you can.",
  );

  const bad = schemaVerdict(t, schema("incompatible", 12, 8));
  expect(bad.tone).toBe("bad");
  expect(bad.title).toBe("Incompatible");
  expect(bad.body).toBe(
    "The database is at version 12, which this binary (version 8) cannot run on. Upgrade this binary.",
  );

  // A verdict this admin does not know (a newer server) is shown as it is.
  const odd = schemaVerdict(t, schema("sideways", 8, 8));
  expect(odd.tone).toBe("warn");
  expect(odd.title).toBe("sideways");
});

test("search_keeps_only_valid_members", () => {
  expect(isSystemTab("status")).toBe(true);
  expect(isSystemTab("backups")).toBe(false);
  expect(
    parseSystemSearch({
      status: "dead",
      kind: "media.variants",
      actor: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
      action: "user.",
      target_kind: "plugin",
      target_id: 42,
    }),
  ).toEqual({
    status: "dead",
    kind: "media.variants",
    actor: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
    action: "user.",
    target_kind: "plugin",
    target_id: "42",
  });
  expect(
    parseSystemSearch({
      status: "zombie",
      kind: "Media Variants",
      actor: "me",
      action: "<b>",
      target_id: "orphan",
    }),
  ).toEqual({});
  // `target_id` needs `target_kind` (the API's rule).
  expect(parseSystemSearch({ target_kind: "user", target_id: "" })).toEqual({
    target_kind: "user",
  });
});

test("theme_origin_and_skew", () => {
  expect(themeOriginText(t, "embedded")).toBe("Built in");
  expect(themeOriginText(t, "folder")).toBe("Folder");
  expect(themeOriginText(t, { artifact: "ab12cd34ef56" })).toBe("Installed bundle");
  expect(themeOriginText(t, "mystery")).toBe("mystery");
  expect(themeOriginText(t, null)).toBeNull();

  expect(SKEW_LIMIT_MS).toBe(1000);
  expect(skewFlag(null)).toBe(false);
  expect(skewFlag(undefined)).toBe(false);
  expect(skewFlag(1000)).toBe(false);
  expect(skewFlag(1001)).toBe(true);
  expect(skewFlag(-1500)).toBe(true);
});
