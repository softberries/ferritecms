import { expect, test } from "vitest";
import { i18n } from "@/i18n";
import { missingKeys } from "./missing-keys";

test("missing_key_fails", () => {
  // @ts-expect-error -- the key is deliberately not in the resources
  expect(() => i18n.t("nope")).toThrow("missing i18n key: common:nope");
  expect(missingKeys).toEqual(["common:nope"]);
  missingKeys.length = 0;
});

test("known_key_translates", () => {
  expect(i18n.t("app.name")).toBe("FerriteCMS");
});
