// Change mode (design 15 §11.2): the editor opens a release entry's staged change as if
// it were the item, and saves the whole snapshot with `PUT /releases/{id}/items/{item}`,
// keeping the entry's transition; a release changed meanwhile (`412`) is read again and
// the save retried once with its new version.

import { afterEach, expect, test, vi } from "vitest";
import type { SchemaContentDto } from "@/api/schema";
import { json, stubRoutes } from "@/test/render";
import { changeSaver, itemOfChange } from "./change-mode";
import { ITEM_A, REL, entry, release } from "./fixtures";

afterEach(() => {
  vi.unstubAllGlobals();
});

const SOURCE = {
  title: "Staged",
  body: { type: "doc", content: [] },
  excerpt: "",
  slug: "about",
  parent_id: null,
  menu_order: 0,
  terms: [],
  fields: {},
  seo: { title: null, description: null, canonical: null, robots: null },
};

test("saves_the_snapshot_with_the_entry_transition", async () => {
  const puts: { ifMatch: string | null; body: unknown }[] = [];
  stubRoutes({
    [`/api/v1/releases/${REL}/items/${ITEM_A}`]: async (_u, req) => {
      expect(req.method).toBe("PUT");
      puts.push({ ifMatch: req.headers.get("If-Match"), body: await req.json() });
      return json(release({ version: 5 + puts.length }));
    },
  });
  const save = changeSaver(release(), entry({ transition: "publish" }));
  await save(SOURCE);
  await save({ ...SOURCE, title: "Again" });
  expect(puts.map((p) => p.ifMatch)).toEqual(['"4"', '"6"']);
  expect(puts[0]?.body).toEqual({
    transition: "publish",
    changes: {
      title: "Staged",
      body: { type: "doc", content: [] },
      excerpt: null,
      slug: "about",
      parent_id: null,
      menu_order: 0,
      terms: [],
      fields: {},
      seo: { title: null, description: null, canonical: null, robots: null },
    },
  });
});

test("a_412_reads_the_release_again_and_retries_once", async () => {
  const ifMatch: (string | null)[] = [];
  stubRoutes({
    [`/api/v1/releases/${REL}`]: () => json(release({ version: 9 })),
    [`/api/v1/releases/${REL}/items/${ITEM_A}`]: (_u, req) => {
      ifMatch.push(req.headers.get("If-Match"));
      return ifMatch.length === 1
        ? json({ type: "/problems/precondition-failed", title: "Stale", status: 412 }, 412)
        : json(release({ version: 10 }));
    },
  });
  await changeSaver(release(), entry())(SOURCE);
  expect(ifMatch).toEqual(['"4"', '"9"']);
});

test("the_staged_change_stands_in_for_the_item", () => {
  const stored = {
    id: ITEM_A,
    title: "Live",
    slug: "about",
    excerpt: "Live excerpt",
    body: { type: "doc", content: [] },
    fields: { a: 1 },
    terms: ["t1"],
    menu_order: 2,
    parent_id: null,
    seo: { title: "Live SEO", description: null, canonical: null, robots: [] },
    status: "published",
    releases: [{ id: REL, title: "Spring launch", status: "open" }],
    allowed: {
      edit: false,
      stage: true,
      publish: false,
      unpublish: true,
      schedule: false,
      submit: false,
      trash: true,
      trash_undoable: true,
      restore: false,
      delete: false,
      needs_approval: false,
      override: false,
    },
  } as unknown as SchemaContentDto;
  const e = entry({
    changes: {
      title: "Staged title",
      body: { type: "doc", content: [{ type: "paragraph" }] },
      excerpt: null,
      slug: "about-us",
      terms: [],
      menu_order: 0,
      fields: { a: 2 },
      seo: { title: "Staged SEO", description: null, canonical: null, robots: null },
    },
  });
  const item = itemOfChange(stored, e, true);
  expect(item).toMatchObject({
    title: "Staged title",
    slug: "about-us",
    excerpt: "",
    fields: { a: 2 },
    terms: [],
    seo: { title: "Staged SEO", robots: [] },
  });
  // Only the change can be saved here: no lifecycle, no trash, no release banner.
  expect(item.allowed).toMatchObject({
    edit: true,
    publish: false,
    unpublish: false,
    trash: false,
  });
  expect(item.releases).toBeUndefined();
  expect(itemOfChange(stored, e, false).allowed.edit).toBe(false);
});
