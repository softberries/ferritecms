import type { operations } from "./schema";

/** The filters of a content list (every query member but the page cursor). */
export type ContentFilters = Omit<
  NonNullable<operations["list_content"]["parameters"]["query"]>,
  "cursor"
>;

/** The filters of the job list (every query member but the page cursor). */
export type JobFilters = Omit<
  NonNullable<operations["list_jobs"]["parameters"]["query"]>,
  "cursor"
>;

/**
 * TanStack Query keys. Keys nest, so invalidating a prefix covers what is under it:
 * `["content", type]` covers that type's lists, counts and items, and an item's key
 * covers its revisions and its autosave. After a save, invalidate
 * `qk.content.lists(type)` and `qk.content.counts(type)`, never `qk.content.all(type)`.
 */
export const qk = {
  meta: ["meta"] as const,
  me: ["me"] as const,
  types: ["types"] as const,
  settings: ["settings"] as const,
  content: {
    all: (type: string) => ["content", type] as const,
    /** Every list of the type; a save invalidates `lists` + `counts` (never `all`, which
     * would refetch the open item and its autosave). */
    lists: (type: string) => ["content", type, "list"] as const,
    list: (type: string, filters: ContentFilters) => ["content", type, "list", filters] as const,
    item: (type: string, id: string) => ["content", type, "item", id] as const,
    counts: (type: string) => ["content", type, "counts"] as const,
    revisions: (type: string, id: string) => ["content", type, "item", id, "revisions"] as const,
    revision: (type: string, id: string, rid: string) =>
      ["content", type, "item", id, "revisions", rid] as const,
    autosave: (type: string, id: string) => ["content", type, "item", id, "autosave"] as const,
  },
  /** The palette's content search (`palette/sources.ts`): one entry per query. */
  paletteSearch: (q: string, types: readonly string[]) => ["palette-search", q, types] as const,
  taxonomies: ["taxonomies"] as const,
  terms: (taxonomy: string) => ["taxonomies", taxonomy, "terms"] as const,
  system: {
    status: ["system", "status"] as const,
    metrics: ["system", "metrics"] as const,
    jobs: (filters: JobFilters) => ["system", "jobs", filters] as const,
  },
  plugins: ["plugins"] as const,
  pluginMetrics: (name: string) => ["plugins", name, "metrics"] as const,
} as const;
