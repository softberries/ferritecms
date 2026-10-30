import type { operations } from "./schema";

/** The filters of a media list (every query member but the page cursor and size). */
export type MediaListFilters = Omit<
  NonNullable<operations["list_media"]["parameters"]["query"]>,
  "cursor" | "limit"
>;

/** The filters of the comments queue (every query member but the page cursor and size). */
export type CommentFilters = Omit<
  NonNullable<operations["list_comments"]["parameters"]["query"]>,
  "cursor" | "limit"
>;

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

/** The filters of the users list (every query member but the page cursor and size). */
export type UserFilters = Omit<
  NonNullable<operations["list_users"]["parameters"]["query"]>,
  "cursor" | "limit"
>;

/** The filters of the audit log (every query member but the page cursor and size). */
export type AuditFilters = Omit<
  NonNullable<operations["list_audit"]["parameters"]["query"]>,
  "cursor" | "limit"
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
    /** The content list screen's pages (`useInfiniteQuery` over `next_cursor`). */
    infinite: (type: string, filters: ContentFilters) =>
      ["content", type, "list", "infinite", filters] as const,
    /** The dashboard's published items of the last `days` days (`fetchPublished`); under
     * `lists`, so a save refreshes the activity chart too. */
    published: (type: string, days: number) =>
      ["content", type, "list", "published-since", days] as const,
    item: (type: string, id: string) => ["content", type, "item", id] as const,
    counts: (type: string) => ["content", type, "counts"] as const,
    /** A parent's title and slug as the viewer may see them (`content/parent-ref.ts`);
     * `null` once the server said it is not readable. */
    parentRef: (type: string, id: string) => ["content", type, "parent-ref", id] as const,
    revisions: (type: string, id: string) => ["content", type, "item", id, "revisions"] as const,
    revision: (type: string, id: string, rid: string) =>
      ["content", type, "item", id, "revisions", rid] as const,
    autosave: (type: string, id: string) => ["content", type, "item", id, "autosave"] as const,
  },
  /** An item named by id alone (`content/find-item.ts`, `GET /content/refs`). */
  contentRef: (id: string) => ["content-ref", id] as const,
  /** The palette's content search (`palette/sources.ts`): one entry per query. */
  paletteSearch: (q: string, types: readonly string[]) => ["palette-search", q, types] as const,
  media: {
    all: ["media"] as const,
    /** The library's pages (`useInfiniteQuery` over `next_cursor`); under `lists`. */
    infinite: (filters: MediaListFilters) => ["media", "list", "infinite", filters] as const,
    /** Every media list (refetch after an upload, an edit or a trash). */
    lists: ["media", "list"] as const,
    item: (id: string) => ["media", "item", id] as const,
  },
  comments: {
    all: ["comments"] as const,
    list: (filters: CommentFilters) => ["comments", "list", filters] as const,
    /** The queue's pages (`useInfiniteQuery` over `next_cursor`); under `list`. */
    infinite: (filters: CommentFilters) => ["comments", "list", "infinite", filters] as const,
    /** Counts per status, of one content item when `item` is given. */
    counts: (item?: string) => ["comments", "counts", item ?? null] as const,
  },
  menus: {
    list: ["menus"] as const,
  },
  redirects: {
    all: ["redirects"] as const,
    list: (q: string) => ["redirects", "list", q] as const,
  },
  taxonomies: ["taxonomies"] as const,
  terms: (taxonomy: string) => ["taxonomies", taxonomy, "terms"] as const,
  system: {
    status: ["system", "status"] as const,
    metrics: ["system", "metrics"] as const,
    /** A dashboard range of `GET /system/metrics/series` (its window moves with the clock). */
    series: (range: string) => ["system", "series", range] as const,
    jobs: (filters: JobFilters) => ["system", "jobs", filters] as const,
    /** Every job list (the dashboard's and the Jobs tab's); refetch after a retry. */
    jobLists: ["system", "jobs"] as const,
    /** The Jobs tab's pages (`useInfiniteQuery` over `next_cursor`); under `jobLists`. */
    jobsInfinite: (filters: JobFilters) => ["system", "jobs", "infinite", filters] as const,
    nodes: ["system", "nodes"] as const,
    audit: (filters: AuditFilters) => ["system", "audit", filters] as const,
    /** The Audit tab's pages (`useInfiniteQuery` over `next_cursor`). */
    auditInfinite: (filters: AuditFilters) => ["system", "audit", "infinite", filters] as const,
  },
  /** Every users list and user (`["users"]` covers them all). */
  usersAll: ["users"] as const,
  users: (filters: UserFilters) => ["users", "list", filters] as const,
  /** Every users list (refetch after a create, an edit or a delete). */
  usersLists: ["users", "list"] as const,
  /** The users screen's pages (`useInfiniteQuery` over `next_cursor`); under `usersLists`. */
  usersInfinite: (filters: UserFilters) => ["users", "list", "infinite", filters] as const,
  user: (id: string) => ["users", "item", id] as const,
  /** Under the user, so invalidating `user(id)` refetches the tokens too. */
  tokens: (userId: string) => ["users", "item", userId, "tokens"] as const,
  /** The signed-in user's sessions. */
  sessions: ["sessions"] as const,
  themes: ["themes"] as const,
  themeSettings: (name: string) => ["themes", name, "settings"] as const,
  /** Every plugin key: the list, each plugin, its metrics and settings. */
  plugins: ["plugins"] as const,
  plugin: (name: string) => ["plugins", name, "item"] as const,
  pluginMetrics: (name: string) => ["plugins", name, "metrics"] as const,
  pluginSettings: (name: string) => ["plugins", name, "settings"] as const,
  /** Deleted plugins whose data is kept (its own root, apart from per-plugin keys). */
  pluginsRetained: ["pluginsRetained"] as const,
  import: {
    /** The runs table's pages (`useInfiniteQuery` over `next_cursor`). */
    runs: ["import", "runs"] as const,
    job: (id: string) => ["import", "job", id] as const,
  },
} as const;
