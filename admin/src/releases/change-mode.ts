// Change mode (`/content/$type/$id?release=<rid>`, design 15 §11.2): the editor edits a
// release entry's staged change. `itemOfChange` is the item as the staged change makes it
// (what the screen opens with, and what a save is compared against); its `allowed` lets
// the screen save the change and nothing else. `changeSaver` saves a snapshot through the
// release (`PUT …/items/{item}`, the entry's transition kept), following the release's
// version; a `412` (someone changed the release) reads it again and retries once.

import { ApiError } from "@/api/problem";
import {
  type ReleaseDetail,
  type ReleaseEntry,
  type StagedSource,
  getRelease,
  putEntry,
  stagedChangeOf,
} from "@/api/releases";
import type { SchemaContentDto, SchemaSeoDto } from "@/api/schema";

const NONE = {
  edit: false,
  delete: false,
  publish: false,
  restore: false,
  schedule: false,
  submit: false,
  trash: false,
  trash_undoable: false,
  unpublish: false,
  needs_approval: false,
  stage: false,
  override: false,
};

/** The item as `entry`'s staged change makes it; `editable`: the release takes changes. */
export function itemOfChange(
  item: SchemaContentDto,
  entry: Pick<ReleaseEntry, "changes">,
  editable: boolean,
): SchemaContentDto {
  const c = entry.changes;
  const { releases: _releases, review: _review, ...rest } = item;
  void _releases;
  void _review;
  const base: SchemaContentDto = { ...rest, allowed: { ...NONE, edit: editable } };
  if (!c) return base;
  const seo: Partial<Record<keyof SchemaSeoDto, unknown>> | null =
    typeof c.seo === "object" && c.seo !== null ? c.seo : null;
  return {
    ...base,
    title: c.title,
    body: c.body,
    excerpt: c.excerpt ?? "",
    slug: c.slug ?? item.slug,
    parent_id: c.parent_id ?? null,
    menu_order: c.menu_order ?? item.menu_order,
    terms: c.terms ? [...c.terms] : item.terms,
    fields: c.fields ?? item.fields,
    seo: seo
      ? {
          ...item.seo,
          ...(seo.title !== undefined ? { title: (seo.title as string | null) ?? null } : {}),
          ...(seo.description !== undefined
            ? { description: (seo.description as string | null) ?? null }
            : {}),
          ...(seo.canonical !== undefined
            ? { canonical: (seo.canonical as string | null) ?? null }
            : {}),
          ...(seo.robots !== undefined
            ? { robots: (seo.robots as SchemaSeoDto["robots"] | null) ?? [] }
            : {}),
        }
      : item.seo,
  };
}

/** Saves snapshots of `entry`'s change in `release`; `onSaved` gets each answer. */
export function changeSaver(
  release: Pick<ReleaseDetail, "id" | "version">,
  entry: Pick<ReleaseEntry, "item_id" | "transition">,
  onSaved?: (next: ReleaseDetail) => void,
): (s: StagedSource) => Promise<void> {
  let version = release.version;
  return async (s) => {
    const body = { changes: stagedChangeOf(s), transition: entry.transition };
    let next: ReleaseDetail;
    try {
      next = await putEntry(release.id, entry.item_id, version, body);
    } catch (e) {
      if (!(e instanceof ApiError && e.status === 412)) throw e;
      version = (await getRelease(release.id)).version;
      next = await putEntry(release.id, entry.item_id, version, body);
    }
    version = next.version;
    onSaved?.(next);
  };
}
