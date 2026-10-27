// The terms of a hierarchical taxonomy as a tree (07 §3 Taxonomies). Terms have no
// sibling order (P35): siblings sort by name with `Intl.Collator`, so the tree is the
// server's list sorted once and handed to `flatten` (which keeps array order). A term
// whose parent is missing is a root; a cycle in the data (two concurrent moves, plan
// ruling P2) is cut at its first term by name rather than lost.

import { defaultFilter } from "cmdk";
import { ApiError } from "@/api/problem";
import type { TermDto } from "@/api/taxonomies";
import { flatten, type TreeNode } from "@/components/kit/tree-model";
import { uiLocale } from "@/lib/locale";

/** By name (case and accents aside, numbers as numbers), then by slug. */
export function termCollator(locale: string = uiLocale()): (a: TermDto, b: TermDto) => number {
  const collator = new Intl.Collator(locale, { sensitivity: "base", numeric: true });
  return (a, b) => collator.compare(a.name, b.name) || collator.compare(a.slug, b.slug);
}

/** The terms as tree nodes, siblings by name. */
export function toTree(terms: readonly TermDto[], locale?: string): TreeNode<TermDto>[] {
  const ids = new Set(terms.map((t) => t.id));
  const nodes: TreeNode<TermDto>[] = [...terms].sort(termCollator(locale)).map((t) => ({
    id: t.id,
    parentId: t.parent_id && ids.has(t.parent_id) ? t.parent_id : null,
    data: t,
  }));
  // Nodes only reachable through a cycle: the first by name becomes a root, until none.
  let reached = new Set(flatten(nodes).map((n) => n.id));
  for (const node of nodes) {
    if (reached.has(node.id)) continue;
    node.parentId = null;
    reached = new Set(flatten(nodes).map((n) => n.id));
  }
  return nodes;
}

/** Every term below `id` (not `id` itself; empty for an unknown id). */
export function descendants(terms: readonly TermDto[], id: string): Set<string> {
  const out = new Set<string>();
  let frontier = [id];
  while (frontier.length > 0) {
    const next: string[] = [];
    for (const t of terms) {
      if (t.parent_id && frontier.includes(t.parent_id) && !out.has(t.id) && t.id !== id) {
        out.add(t.id);
        next.push(t.id);
      }
    }
    frontier = next;
  }
  return out;
}

/** How many terms have `id` as their parent. */
export function childCount(terms: readonly TermDto[], id: string): number {
  return terms.filter((t) => t.parent_id === id).length;
}

export interface ParentChoice {
  term: TermDto;
  depth: number;
  /** Names from the root down to the term itself. */
  path: string[];
}

/**
 * The terms `id` may move under, in tree order: every term but `id` and its descendants
 * (`id = null`: every term, for a new one).
 */
export function parentChoices(
  terms: readonly TermDto[],
  id: string | null,
  locale?: string,
): ParentChoice[] {
  const excluded = id === null ? new Set<string>() : new Set([id, ...descendants(terms, id)]);
  const path: string[] = [];
  return flatten(toTree(terms, locale)).flatMap((n) => {
    path.length = n.depth;
    path.push(n.data.name);
    return excluded.has(n.id) ? [] : [{ term: n.data, depth: n.depth, path: [...path] }];
  });
}

/** `terms` with term `id` under `parentId` (a new array; the other terms are kept). */
export function withParent(
  terms: readonly TermDto[],
  id: string,
  parentId: string | null,
): TermDto[] {
  return terms.map((t) => (t.id === id ? { ...t, parent_id: parentId } : t));
}

/** Lower case without accents, for the filter. */
const fold = (s: string) => s.normalize("NFD").replace(/\p{M}/gu, "").toLocaleLowerCase();

/** The terms whose name, slug or description holds every word of `query`, by name. */
export function filterTerms(
  terms: readonly TermDto[],
  query: string,
  locale: string = uiLocale(),
): TermDto[] {
  const words = fold(query).split(/\s+/).filter(Boolean);
  return terms
    .filter((term) => {
      const text = fold(`${term.name} ${term.slug} ${term.description}`);
      return words.every((w) => text.includes(w));
    })
    .sort(termCollator(locale));
}

/** The server's reason for a refused write, or the error's own message. */
export const problemText = (e: unknown) =>
  e instanceof ApiError
    ? e.problem.detail || e.problem.title
    : e instanceof Error
      ? e.message
      : String(e);

/**
 * The parent combobox's filter: cmdk's scoring over an option's keywords (name and
 * slug) only. Each option's `value` is the term's id, which a search must never match.
 */
export function parentFilter(_value: string, search: string, keywords?: string[]): number {
  return defaultFilter("", search, keywords);
}
