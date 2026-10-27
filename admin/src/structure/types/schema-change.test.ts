// What a schema change does to stored items (Review Focus 2): fields removed or renamed
// (their values stay stored until each item's next save, which drops them), fields that
// became required (an item without a value cannot be saved until it gets one), and kind
// changes (stored values may no longer be valid).

import { expect, test } from "vitest";
import type { FieldDef, FieldKind } from "@/editor/fields/fields-model";
import { describeChange, isEmptyChange } from "./schema-change";

const def = (name: string, kind: FieldKind = "text", extra: Partial<FieldDef> = {}): FieldDef => ({
  name,
  label: name,
  kind,
  required: false,
  options: [],
  fields: [],
  ...extra,
});

const NONE = {
  removed: [],
  renamed: [],
  newlyRequired: [],
  kindChanged: [],
  optionsRemoved: [],
  targetsNarrowed: [],
};

test("no_change", () => {
  const schema = [def("a"), def("b", "number")];
  expect(describeChange(schema, schema)).toEqual(NONE);
  expect(isEmptyChange(describeChange(schema, schema))).toBe(true);
  // Labels, options and order alone change nothing for stored values.
  expect(
    describeChange(
      [def("a"), def("s", "select", { options: ["x"] })],
      [def("s", "select", { options: ["x", "y"], label: "S" }), def("a")],
    ),
  ).toEqual(NONE);
});

test("added_optional_fields_are_no_change", () => {
  expect(describeChange([def("a")], [def("a"), def("b")])).toEqual(NONE);
  expect(describeChange([], [def("a")])).toEqual(NONE);
});

test("removed", () => {
  const out = describeChange([def("a"), def("b"), def("c")], [def("a"), def("c")]);
  expect(out).toEqual({ ...NONE, removed: ["b"] });
  expect(isEmptyChange(out)).toBe(false);
});

test("renamed_is_same_position_and_kind_with_a_new_name", () => {
  expect(describeChange([def("a"), def("b")], [def("a"), def("bee")])).toEqual({
    ...NONE,
    renamed: [["b", "bee"]],
  });
  // Another kind at that position: the old one is removed (and the new one is just new).
  expect(describeChange([def("a"), def("b")], [def("a"), def("bee", "number")])).toEqual({
    ...NONE,
    removed: ["b"],
  });
  // A different position: removed, not renamed.
  expect(describeChange([def("a"), def("b")], [def("bee"), def("a")])).toEqual({
    ...NONE,
    removed: ["b"],
  });
  // The new name was already a field (two swapped names): no rename either.
  expect(describeChange([def("a"), def("b")], [def("b"), def("a")])).toEqual(NONE);
});

test("newly_required", () => {
  // Made required, and a new required field.
  expect(
    describeChange(
      [def("a"), def("b")],
      [def("a", "text", { required: true }), def("b"), def("c", "bool", { required: true })],
    ),
  ).toEqual({ ...NONE, newlyRequired: ["a", "c"] });
  // Already required: nothing new.
  const req = def("a", "text", { required: true });
  expect(describeChange([req], [req])).toEqual(NONE);
  // A renamed field keeps what it was.
  expect(
    describeChange([def("a", "text", { required: true })], [def("aa", "text", { required: true })]),
  ).toEqual({ ...NONE, renamed: [["a", "aa"]] });
  expect(describeChange([def("a")], [def("aa", "text", { required: true })])).toEqual({
    ...NONE,
    renamed: [["a", "aa"]],
    newlyRequired: ["aa"],
  });
});

test("kind_changed", () => {
  expect(describeChange([def("a"), def("b")], [def("a", "number"), def("b")])).toEqual({
    ...NONE,
    kindChanged: ["a"],
  });
});

test("repeater_sub_fields_use_the_editors_paths", () => {
  const before = [
    def("chapters", "repeater", {
      fields: [def("title"), def("notes", "repeater", { fields: [def("note"), def("by")] })],
    }),
  ];
  const after = [
    def("chapters", "repeater", {
      fields: [
        def("heading", "text", { required: true }),
        def("notes", "repeater", { fields: [def("note", "number")] }),
      ],
    }),
  ];
  expect(describeChange(before, after)).toEqual({
    ...NONE,
    removed: ["chapters[].notes[].by"],
    renamed: [["chapters[].title", "chapters[].heading"]],
    newlyRequired: ["chapters[].heading"],
    kindChanged: ["chapters[].notes[].note"],
  });
  // A whole repeater removed is one entry, not one per sub-field.
  expect(describeChange(before, [])).toEqual({ ...NONE, removed: ["chapters"] });
});

test("removed_select_options_are_named", () => {
  // M2b-2 type builder minor: an item holding a removed option cannot be saved until
  // another is chosen (core checks the value against the options).
  const before = [def("genre", "select", { options: ["fiction", "essay", "poetry"] })];
  const after = [def("genre", "select", { options: ["fiction", "poetry", "drama"] })];
  const out = describeChange(before, after);
  expect(out).toEqual({ ...NONE, optionsRemoved: [["genre", ["essay"]]] });
  expect(isEmptyChange(out)).toBe(false);
  // Inside a repeater, with the editor's path; a kind change is named as that instead.
  expect(
    describeChange(
      [def("rows", "repeater", { fields: [def("s", "select", { options: ["a", "b"] })] })],
      [def("rows", "repeater", { fields: [def("s", "select", { options: ["a"] })] })],
    ),
  ).toEqual({ ...NONE, optionsRemoved: [["rows[].s", ["b"]]] });
  expect(describeChange(before, [def("genre")])).toEqual({ ...NONE, kindChanged: ["genre"] });
});

test("narrowed_relation_targets_are_named", () => {
  // A relation that points at fewer types: an item whose value is of a type no longer
  // listed cannot be saved until it is changed (core refuses it at the field).
  const rel = (types: string[]) => [def("related", "relation", types.length ? { types } : {})];
  expect(describeChange(rel([]), rel(["post"]))).toEqual({
    ...NONE,
    targetsNarrowed: ["related"],
  });
  expect(describeChange(rel(["post", "page"]), rel(["post"]))).toEqual({
    ...NONE,
    targetsNarrowed: ["related"],
  });
  // Wider (another type, or any type) is no change for stored values.
  expect(describeChange(rel(["post"]), rel(["post", "page"]))).toEqual(NONE);
  expect(describeChange(rel(["post"]), rel([]))).toEqual(NONE);
});
