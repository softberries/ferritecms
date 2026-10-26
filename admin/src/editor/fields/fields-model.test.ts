// The custom-field model mirrors core's rules (`crates/ferrite-core/src/fields.rs`):
// stale keys after a schema change, what a save sends, and every kind's validation.

import { describe, expect, test } from "vitest";
import { ApiError } from "@/api/problem";
import type { ContentTypeDto } from "@/api/types";
import {
  type FieldDef,
  emptyRow,
  emptyValue,
  fieldIssues,
  fieldsFor,
  sameValue,
  serverFieldErrors,
  staleKeys,
  toSave,
  validateFields,
} from "./fields-model";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";

const def = (name: string, kind: FieldDef["kind"], extra: Partial<FieldDef> = {}): FieldDef => ({
  name,
  label: name,
  kind,
  required: false,
  options: [],
  fields: [],
  ...extra,
});

const type = (fields: FieldDef[] | null | undefined): ContentTypeDto => ({
  name: "book",
  label: "Books",
  built_in: false,
  has_archive: false,
  hierarchical: false,
  ...(fields === undefined ? {} : { fields }),
});

describe("fields_model_test", () => {
  test("fields_for_is_null_without_a_schema", () => {
    expect(fieldsFor(type(null))).toBeNull();
    expect(fieldsFor(type(undefined))).toBeNull();
    expect(fieldsFor(type([]))).toEqual([]);
    expect(fieldsFor(type([def("a", "text")]))?.map((d) => d.name)).toEqual(["a"]);
  });

  test("stale_keys_are_top_level_keys_not_in_the_schema", () => {
    const defs = [def("subtitle", "text"), def("pages", "number")];
    expect(staleKeys(defs, { subtitle: "x", isbn: "1", featured_image: ID, old: null })).toEqual([
      "isbn",
      "old",
    ]);
    expect(staleKeys(defs, {})).toEqual([]);
  });

  test("to_save_drops_exactly_the_stale_keys_and_keeps_nulls", () => {
    const defs = [
      def("subtitle", "text"),
      def("chapters", "repeater", { fields: [def("t", "text")] }),
    ];
    const value = {
      subtitle: null,
      chapters: [{ t: "One", extra: 1 }],
      featured_image: null,
      gone: "x",
    };
    expect(toSave(defs, value)).toEqual({
      subtitle: null,
      chapters: [{ t: "One", extra: 1 }],
      featured_image: null,
    });
    // Nothing stale: the same content.
    expect(toSave(defs, { subtitle: "a" })).toEqual({ subtitle: "a" });
  });

  test("same_value_tells_null_from_absent_and_ignores_key_order", () => {
    expect(sameValue({ a: 1, b: [1, { c: null }] }, { b: [1, { c: null }], a: 1 })).toBe(true);
    expect(sameValue({ a: null }, {})).toBe(false);
    expect(sameValue({ a: [1, 2] }, { a: [2, 1] })).toBe(false);
  });

  test("empty_values", () => {
    expect(emptyValue(def("a", "text"))).toBeNull();
    expect(emptyValue(def("a", "bool"))).toBeNull();
    expect(emptyValue(def("a", "repeater", { fields: [def("b", "text")] }))).toEqual([]);
    expect(
      emptyRow([def("b", "text"), def("c", "repeater", { fields: [def("d", "bool")] })]),
    ).toEqual({ b: null, c: [] });
  });

  test("required_and_nulls", () => {
    const defs = [def("need", "text", { required: true }), def("maybe", "text")];
    expect(validateFields(defs, {})).toEqual({ "fields.need": "Required" });
    expect(validateFields(defs, { need: null, maybe: null })).toEqual({
      "fields.need": "Required",
    });
    expect(validateFields(defs, { need: "x", maybe: null })).toEqual({});
    expect(fieldIssues(defs, {})["fields.need"]?.code).toBe("required");
  });

  test("every_kind_is_checked_like_core", () => {
    const defs = [
      def("text", "text"),
      def("rich", "rich_text"),
      def("num", "number"),
      def("when", "date"),
      def("flag", "bool"),
      def("pick", "select", { options: ["a", "b"] }),
      def("rel", "relation"),
      def("file", "media"),
    ];
    const good = {
      text: "Hello",
      rich: { type: "doc", content: [] },
      num: 1.5,
      when: "2026-10-26T10:00:00Z",
      flag: false,
      pick: "b",
      rel: ID,
      file: ID.toUpperCase(),
    };
    expect(validateFields(defs, good)).toEqual({});
    const bad = fieldIssues(defs, {
      text: 3,
      rich: [],
      num: "1.5",
      when: "26 Oct 2026",
      flag: "yes",
      pick: "c",
      rel: "not-a-uuid",
      file: 7,
    });
    expect(Object.fromEntries(Object.entries(bad).map(([k, v]) => [k, v.code]))).toEqual({
      "fields.text": "text",
      "fields.rich": "richText",
      "fields.num": "number",
      "fields.when": "date",
      "fields.flag": "bool",
      "fields.pick": "select",
      "fields.rel": "uuid",
      "fields.file": "uuid",
    });
    // Offsets and fractions are RFC 3339 too; a date alone is not.
    expect(validateFields([def("d", "date")], { d: "2026-10-26T10:00:00.5+02:00" })).toEqual({});
    expect(fieldIssues([def("d", "date")], { d: "2026-10-26" })["fields.d"]?.code).toBe("date");
    // Not a finite number.
    expect(fieldIssues([def("n", "number")], { n: Number.NaN })["fields.n"]?.code).toBe("number");
  });

  test("text_is_at_most_65536_characters", () => {
    const defs = [def("t", "text")];
    expect(validateFields(defs, { t: "a".repeat(65_536) })).toEqual({});
    expect(fieldIssues(defs, { t: "a".repeat(65_537) })["fields.t"]?.code).toBe("textLong");
    // Characters, not UTF-16 units.
    expect(validateFields(defs, { t: "😀".repeat(65_536) })).toEqual({});
  });

  test("featured_image_is_a_uuid_or_null", () => {
    expect(validateFields([], { featured_image: null })).toEqual({});
    expect(validateFields([], { featured_image: ID })).toEqual({});
    expect(fieldIssues([], { featured_image: "x" })["fields.featured_image"]?.code).toBe("uuid");
  });

  test("repeater_rows_and_depth", () => {
    const inner = def("steps", "repeater", { fields: [def("s", "text", { required: true })] });
    const defs = [
      def("chapters", "repeater", { fields: [def("title", "text", { required: true }), inner] }),
    ];
    const value = {
      chapters: [
        { title: "One", steps: [{ s: "a" }, { s: null }] },
        { title: null, steps: [] },
        "nope",
        { title: "Four", stray: 1 },
      ],
    };
    expect(validateFields(defs, value)).toEqual({
      "fields.chapters[0].steps[1].s": "Required",
      "fields.chapters[1].title": "Required",
      "fields.chapters[2]": "Must be a row of fields",
      "fields.chapters[3].stray": "Not a field of this row",
    });
    expect(fieldIssues(defs, { chapters: {} })["fields.chapters"]?.code).toBe("array");
    const rows = Array.from({ length: 201 }, () => ({ title: "x", steps: [] }));
    expect(fieldIssues(defs, { chapters: rows })["fields.chapters"]?.code).toBe("rows");
    expect(validateFields(defs, { chapters: rows.slice(0, 200) })).toEqual({});
  });

  test("server_422_paths_map_onto_inputs", () => {
    const error = new ApiError({
      type: "https://ferrite.dev/problems/validation",
      title: "Invalid",
      status: 422,
      detail: "fields.chapters[2].title: required",
      errors: [
        { field: "fields", detail: "fields.chapters[2].title: required" },
        { field: "seo.title", detail: "seo.title: control characters are not allowed" },
        { field: "fields", detail: "fields must be an object" },
      ],
    });
    expect(serverFieldErrors(error)).toEqual({
      "fields.chapters[2].title": "required",
      "seo.title": "seo.title: control characters are not allowed",
      fields: "fields must be an object",
    });
    expect(serverFieldErrors(new Error("x"))).toEqual({});
  });
});
