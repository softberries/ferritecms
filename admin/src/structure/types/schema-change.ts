// What a schema change means for the items already stored (Review Focus 2). Core refuses
// unknown keys and missing required values on every item write (`fields.rs`), and the
// editor drops keys the schema no longer has on the next save (P39). So before a save the
// builder names: fields removed, fields renamed (same position and kind, a new name; the
// old values are not carried over), fields that became required, fields whose kind
// changed, select options removed (an item holding one cannot be saved until another is
// chosen) and relations that point at fewer types than before (an item pointing at one
// no longer listed cannot be saved until it is changed, P52). Repeater sub-fields are
// named with the editor's paths (`chapters[].title`).

import type { FieldDef } from "@/editor/fields/fields-model";

export interface SchemaChange {
  removed: string[];
  /** `[old, new]`. */
  renamed: [string, string][];
  newlyRequired: string[];
  kindChanged: string[];
  /** `[field, removed options]`, for a select that stayed one. */
  optionsRemoved: [string, string[]][];
  /** Relations that stayed one and now point at fewer types (any type counts as all). */
  targetsNarrowed: string[];
}

export const isEmptyChange = (c: SchemaChange) =>
  c.removed.length +
    c.renamed.length +
    c.newlyRequired.length +
    c.kindChanged.length +
    c.optionsRemoved.length +
    c.targetsNarrowed.length ===
  0;

export function describeChange(
  before: readonly FieldDef[],
  after: readonly FieldDef[],
): SchemaChange {
  const out: SchemaChange = {
    removed: [],
    renamed: [],
    newlyRequired: [],
    kindChanged: [],
    optionsRemoved: [],
    targetsNarrowed: [],
  };
  walk(before, after, "", out);
  return out;
}

function walk(
  before: readonly FieldDef[],
  after: readonly FieldDef[],
  prefix: string,
  out: SchemaChange,
) {
  const beforeNames = new Set(before.map((d) => d.name));
  const afterNames = new Set(after.map((d) => d.name));
  // Renames: the same position and kind, a name neither list had before.
  const renamedTo = new Map<string, FieldDef>();
  before.forEach((b, i) => {
    const a = after[i];
    if (
      a &&
      a.kind === b.kind &&
      a.name !== b.name &&
      !afterNames.has(b.name) &&
      !beforeNames.has(a.name)
    ) {
      renamedTo.set(a.name, b);
      out.renamed.push([`${prefix}${b.name}`, `${prefix}${a.name}`]);
    }
  });
  const renamedFrom = new Set([...renamedTo.values()].map((b) => b.name));
  for (const b of before) {
    if (!afterNames.has(b.name) && !renamedFrom.has(b.name)) out.removed.push(`${prefix}${b.name}`);
  }
  for (const a of after) {
    const same = before.find((b) => b.name === a.name);
    const was = same ?? renamedTo.get(a.name);
    if (a.required === true && was?.required !== true) out.newlyRequired.push(`${prefix}${a.name}`);
    if (same && same.kind !== a.kind) out.kindChanged.push(`${prefix}${a.name}`);
    if (same?.kind === "select" && a.kind === "select") {
      const kept = new Set(a.options ?? []);
      const gone = (same.options ?? []).filter((o) => !kept.has(o));
      if (gone.length > 0) out.optionsRemoved.push([`${prefix}${a.name}`, gone]);
    }
    if (same?.kind === "relation" && a.kind === "relation") {
      const was = same.types ?? [];
      const now = a.types ?? [];
      // Empty is any type: narrowed when it now lists some, or drops one it listed.
      if (now.length > 0 && (was.length === 0 || was.some((t) => !now.includes(t)))) {
        out.targetsNarrowed.push(`${prefix}${a.name}`);
      }
    }
    if (same && same.kind === "repeater" && a.kind === "repeater") {
      walk(same.fields ?? [], a.fields ?? [], `${prefix}${a.name}[].`, out);
    }
  }
}
