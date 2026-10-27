// The field builder's model: rows to and from the stored schema, names derived from
// labels, and `checkBuilder`, which mirrors core's schema rules
// (`crates/ferrite-core/src/fields.rs`: `check_level`, `check_options`, `valid_name`).

import { describe, expect, test } from "vitest";
import type { FieldDef, FieldKind } from "@/editor/fields/fields-model";
import {
  type BuilderRow,
  MAX_FIELDS,
  MAX_LABEL,
  MAX_NAME,
  MAX_OPTIONS,
  checkBuilder,
  errorKey,
  fromSchema,
  serverFieldError,
  nameFromLabel,
  newRow,
  toSchema,
} from "./builder-model";

const def = (name: string, kind: FieldKind, extra: Partial<FieldDef> = {}): FieldDef => ({
  name,
  label: name.toUpperCase(),
  kind,
  required: false,
  options: [],
  fields: [],
  ...extra,
});

/** Every kind, a select with options and a repeater holding a repeater. */
const EVERY_KIND: FieldDef[] = [
  def("subtitle", "text", { required: true }),
  def("blurb", "rich_text"),
  def("price", "number"),
  def("release", "date"),
  def("in_print", "bool"),
  def("genre", "select", { options: ["fiction", "essay"] }),
  def("related", "relation"),
  def("cover", "media"),
  def("chapters", "repeater", {
    fields: [
      def("title", "text", { required: true }),
      def("notes", "repeater", { fields: [def("note", "text")] }),
    ],
  }),
];

const row = (name: string, kind: FieldKind, extra: Partial<BuilderRow> = {}): BuilderRow => ({
  ...newRow(),
  name,
  label: name,
  kind,
  ...extra,
});

const options = (...values: string[]) => values.map((value, i) => ({ key: `o${i}`, value }));

describe("builder_model_test", () => {
  test("round_trip_of_every_kind_and_a_nested_repeater", () => {
    const rows = fromSchema(EVERY_KIND);
    expect(toSchema(rows)).toEqual(EVERY_KIND);
    // Saved fields remember their name and kind; every row has its own key.
    expect(rows[0]?.saved).toEqual({ name: "subtitle", kind: "text" });
    const keys = [
      ...rows.map((r) => r.key),
      ...(rows[8]?.fields ?? []).map((r) => r.key),
      ...(rows[8]?.fields[1]?.fields ?? []).map((r) => r.key),
    ];
    expect(new Set(keys).size).toBe(keys.length);
    expect(rows[5]?.options.map((o) => o.value)).toEqual(["fiction", "essay"]);
  });

  test("no_schema_and_missing_members", () => {
    expect(fromSchema(null)).toEqual([]);
    expect(fromSchema(undefined)).toEqual([]);
    // The DTO may leave out `label`, `required`, `options` and `fields`.
    expect(toSchema(fromSchema([{ name: "a", kind: "text" }]))).toEqual([
      def("a", "text", { label: "" }),
    ]);
  });

  test("to_schema_keeps_options_and_sub_fields_only_where_they_belong", () => {
    // A select turned into text keeps its options in the builder (switching back brings
    // them again), but the schema sends none: core refuses options on other kinds.
    const rows = [
      row("a", "text", { options: options("x") }),
      row("b", "select", { options: options("x", "y") }),
      row("c", "bool", { fields: [row("d", "text")] }),
      row("e", "repeater", { fields: [row("f", "text")] }),
    ];
    const out = toSchema(rows);
    expect(out.map((f) => [f.name, f.options, f.fields?.map((s) => s.name)])).toEqual([
      ["a", [], []],
      ["b", ["x", "y"], []],
      ["c", [], []],
      ["e", [], ["f"]],
    ]);
  });

  test("to_schema_trims_labels", () => {
    expect(toSchema([row("a", "text", { label: "  Price  " })])[0]?.label).toBe("Price");
  });

  test.each([
    ["Chapter title", "chapter_title"],
    ["Price", "price"],
    ["  In print? ", "in_print"],
    ["Café crème", "cafe_creme"],
    ["2nd edition", "nd_edition"],
    ["__Hidden__ value", "hidden_value"],
    ["a--b  c", "a_b_c"],
    ["Ångström", "angstrom"],
    ["日本", ""],
    ["123", ""],
    ["", ""],
  ])("name_from_label %j", (label, name) => {
    expect(nameFromLabel(label)).toBe(name);
  });

  test("name_from_label_is_at_most_the_limit", () => {
    const long = nameFromLabel("word ".repeat(40));
    expect(long.length).toBeLessThanOrEqual(MAX_NAME);
    expect(long).toMatch(/^[a-z][a-z0-9_]*[a-z0-9]$/);
    expect(nameFromLabel("abcd efgh", 5)).toBe("abcd");
  });

  test("a_valid_schema_has_no_errors", () => {
    expect(checkBuilder(fromSchema(EVERY_KIND))).toEqual({});
    expect(checkBuilder([])).toEqual({});
  });

  test("names_follow_the_pattern", () => {
    for (const bad of ["", "A", "1a", "_a", "a-b", "a b", "ä", "a".repeat(MAX_NAME + 1)]) {
      const r = row(bad, "text");
      expect(checkBuilder([r]), JSON.stringify(bad)).toHaveProperty([errorKey(r.key, "name")]);
    }
    expect(checkBuilder([row("a".repeat(MAX_NAME), "text")])).toEqual({});
    expect(checkBuilder([row("a_1", "text")])).toEqual({});
  });

  test("duplicates_per_level_only", () => {
    const a = row("a", "text");
    const b = row("a", "bool");
    const out = checkBuilder([a, b]);
    // The second one is the duplicate (core reports the later field).
    expect(out).toHaveProperty([errorKey(b.key, "name")]);
    expect(out).not.toHaveProperty([errorKey(a.key, "name")]);
    expect(out[errorKey(b.key, "name")]).toMatch(/already/i);
    // The same name on another level is fine.
    expect(
      checkBuilder([row("a", "text"), row("r", "repeater", { fields: [row("a", "text")] })]),
    ).toEqual({});
  });

  test("featured_image_is_reserved_at_the_top_only", () => {
    const top = row("featured_image", "media");
    expect(checkBuilder([top])[errorKey(top.key, "name")]).toMatch(/featured image/i);
    expect(
      checkBuilder([row("r", "repeater", { fields: [row("featured_image", "media")] })]),
    ).toEqual({});
  });

  test("at_most_max_fields_per_level", () => {
    const many = Array.from({ length: MAX_FIELDS + 1 }, (_, i) => row(`f${i}`, "text"));
    expect(checkBuilder(many)).toHaveProperty([errorKey(null, "fields")]);
    expect(checkBuilder(many.slice(0, MAX_FIELDS))).toEqual({});
    const r = row("r", "repeater", { fields: many });
    expect(checkBuilder([r])).toHaveProperty([errorKey(r.key, "fields")]);
  });

  test("labels_at_most_max_label_characters", () => {
    const r = row("a", "text", { label: "ż".repeat(MAX_LABEL + 1) });
    expect(checkBuilder([r])).toHaveProperty([errorKey(r.key, "label")]);
    expect(checkBuilder([row("a", "text", { label: "ż".repeat(MAX_LABEL) })])).toEqual({});
    // Core takes an empty label (the editor shows the name instead).
    expect(checkBuilder([row("a", "text", { label: "" })])).toEqual({});
  });

  test("select_needs_one_to_max_distinct_non_empty_options", () => {
    const none = row("s", "select");
    expect(checkBuilder([none])).toHaveProperty([errorKey(none.key, "options")]);
    const values = Array.from({ length: MAX_OPTIONS + 1 }, (_, i) => `o${i}`);
    const many = row("s", "select", { options: options(...values) });
    expect(checkBuilder([many])).toHaveProperty([errorKey(many.key, "options")]);
    expect(
      checkBuilder([row("s", "select", { options: options(...values.slice(0, MAX_OPTIONS)) })]),
    ).toEqual({});
    const dup = row("s", "select", { options: options("a", "a") });
    const dupOut = checkBuilder([dup]);
    expect(dupOut).toHaveProperty([errorKey(dup.key, "option", "o1")]);
    expect(dupOut).not.toHaveProperty([errorKey(dup.key, "option", "o0")]);
    const empty = row("s", "select", { options: options("a", "") });
    expect(checkBuilder([empty])).toHaveProperty([errorKey(empty.key, "option", "o1")]);
    const long = row("s", "select", { options: options("x".repeat(MAX_LABEL + 1)) });
    expect(checkBuilder([long])).toHaveProperty([errorKey(long.key, "option", "o0")]);
    // Options of a kind that has none are not checked (the schema leaves them out).
    expect(checkBuilder([row("t", "text", { options: options("", "") })])).toEqual({});
  });

  test("repeater_needs_sub_fields_and_nests_at_most_two_deep", () => {
    const empty = row("r", "repeater");
    expect(checkBuilder([empty])).toHaveProperty([errorKey(empty.key, "fields")]);
    // Sub-fields of a kind that has none are not checked (the schema leaves them out).
    expect(checkBuilder([row("t", "text", { fields: [row("", "text")] })])).toEqual({});
    const two = row("r1", "repeater", {
      fields: [row("r2", "repeater", { fields: [row("x", "text")] })],
    });
    expect(checkBuilder([two])).toEqual({});
    const third = row("r3", "repeater", { fields: [row("y", "text")] });
    const three = row("r1", "repeater", {
      fields: [row("r2", "repeater", { fields: [third] })],
    });
    expect(checkBuilder([three])[errorKey(third.key, "kind")]).toMatch(/2/);
  });

  test("nested_errors_carry_the_sub_field_key", () => {
    const sub = row("Bad", "text");
    const r = row("r", "repeater", { fields: [sub] });
    expect(Object.keys(checkBuilder([r]))).toEqual([errorKey(sub.key, "name")]);
  });

  test("relation_types_round_trip_and_go_only_with_a_relation", () => {
    // P52: a relation's target types; empty (any type) is left out, as the server does.
    const schema = [
      def("related", "relation", { types: ["post", "page"] }),
      def("other", "relation"),
    ];
    const rows = fromSchema(schema);
    expect(rows[0]?.types).toEqual(["post", "page"]);
    expect(toSchema(rows)).toEqual(schema);
    // A relation turned into text keeps its types in the builder, the schema sends none.
    const text = { ...rows[0]!, kind: "text" as const };
    expect(toSchema([text])[0]).not.toHaveProperty("types");
    expect(toSchema([{ ...text, kind: "relation" }])[0]?.types).toEqual(["post", "page"]);
  });

  test("relation_types_must_be_registered_and_not_media", () => {
    const registered = ["post", "page", "book", "media"];
    const ok = row("rel", "relation", { types: ["post", "book"] });
    expect(checkBuilder([ok], registered)).toEqual({});
    const unknown = row("rel", "relation", { types: ["post", "gone"] });
    expect(checkBuilder([unknown], registered)[errorKey(unknown.key, "types")]).toMatch(/gone/);
    const media = row("rel", "relation", { types: ["media"] });
    expect(checkBuilder([media], registered)[errorKey(media.key, "types")]).toMatch(/media/i);
    // Nested too; without the registry (not loaded yet) the server decides.
    const sub = row("rel", "relation", { types: ["gone"] });
    expect(checkBuilder([row("r", "repeater", { fields: [sub] })], registered)).toHaveProperty([
      errorKey(sub.key, "types"),
    ]);
    expect(checkBuilder([unknown])).toEqual({});
    // Types of another kind are not checked (the schema leaves them out).
    expect(checkBuilder([row("t", "text", { types: ["gone"] })], registered)).toEqual({});
  });

  test("a_server_types_refusal_lands_at_its_field", () => {
    const sub = row("related", "relation", { types: ["gone"] });
    const rows = [row("title", "text"), row("chapters", "repeater", { fields: [sub] })];
    expect(
      serverFieldError(rows, 'fields.chapters.related: type "gone" is not a content type'),
    ).toEqual({
      key: errorKey(sub.key, "types"),
      message: 'type "gone" is not a content type',
    });
    // Anything else (another member, an unknown path) stays a message for the whole form.
    expect(serverFieldError(rows, "fields.title: a select needs 1 to 100 options")).toBeNull();
    expect(serverFieldError(rows, "fields.nope: type x is not a content type")).toBeNull();
    expect(serverFieldError(rows, "label is too long")).toBeNull();
  });
});
