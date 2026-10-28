// Permalink structures as core reads them (`ferrite-core/src/permalink.rs`): the checks
// `Structure::parse`, `PermalinkRules::from_options` and the constant half of
// `check_types` make (a first segment that is a route word or a year), and the path a
// structure gives a sample item. Pure functions: the Permalinks settings preview them as
// the structure is typed. Core also refuses a first segment that names a registered
// content type or taxonomy; those names are not known here, so the server's `422` says
// so at the field.

/** The tokens core knows; a segment is one token or a literal. */
export const PERMALINK_TOKENS = [
  "%slug%",
  "%year%",
  "%month%",
  "%category%",
  "%parent_path%",
] as const;
export type PermalinkToken = (typeof PERMALINK_TOKENS)[number];

/** Core's defaults (`DEFAULT_POST`, `DEFAULT_PAGE`). */
export const DEFAULT_POST_STRUCTURE = "/%slug%/";
export const DEFAULT_PAGE_STRUCTURE = "/%parent_path%/%slug%/";

/** `slug::RESERVED`: words a first literal segment may not be. */
const RESERVED = [
  "admin",
  "api",
  "feed",
  "_media",
  "_theme",
  "_preview",
  "_p",
  "_comment",
  "sitemap.xml",
  "robots.txt",
  "healthz",
  "readyz",
  "metrics",
];

/** `slug::ROUTE_WORDS`: author archives, pagination, search. */
const ROUTE_WORDS = ["author", "page", "search"];

/** `%category%` of an item without a category (core's `UNCATEGORIZED`). */
const UNCATEGORIZED = "uncategorized";

export type StructureKind = "post" | "page";

export type StructureProblem =
  | { problem: "slashes" }
  | { problem: "noSlug" }
  | { problem: "emptySegment" }
  | { problem: "unknownToken"; segment: string }
  | { problem: "dotSegment" }
  | { problem: "uppercase"; segment: string }
  | { problem: "badLiteral"; segment: string }
  | { problem: "reserved"; segment: string }
  | { problem: "routeWord"; segment: string }
  | { problem: "year"; segment: string }
  | { problem: "noParentPath" };

const isToken = (s: string): s is PermalinkToken =>
  (PERMALINK_TOKENS as readonly string[]).includes(s);

/** Why core would refuse `structure` for `kind`, or `null` when it takes it. */
export function checkStructure(structure: string, kind: StructureKind): StructureProblem | null {
  // `/` alone too: core strips both slashes from one character and finds none left.
  if (!structure.startsWith("/") || !structure.endsWith("/") || structure.length < 2) {
    return { problem: "slashes" };
  }
  const inner = structure.slice(1, -1);
  if (inner === "") return { problem: "noSlug" };
  const parts = inner.split("/");
  for (const part of parts) {
    if (part === "") return { problem: "emptySegment" };
    if (isToken(part)) continue;
    if (part.includes("%")) return { problem: "unknownToken", segment: part };
    if (part === "." || part === "..") return { problem: "dotSegment" };
    if (/[A-Z]/.test(part)) return { problem: "uppercase", segment: part };
    if (!/^[a-z0-9._~-]+$/.test(part)) return { problem: "badLiteral", segment: part };
  }
  if (!parts.includes("%slug%")) return { problem: "noSlug" };
  const first = parts[0] ?? "";
  if (!isToken(first)) {
    // Core compares these case-insensitively; literals are lower case by now.
    if (RESERVED.includes(first)) return { problem: "reserved", segment: first };
    if (ROUTE_WORDS.includes(first)) return { problem: "routeWord", segment: first };
    if (/^[0-9]{4}$/.test(first)) return { problem: "year", segment: first };
  }
  if (kind === "page" && !parts.includes("%parent_path%")) return { problem: "noParentPath" };
  return null;
}

/** The item a preview shows. */
export interface PermalinkSample {
  slug: string;
  /** When it was published (ISO 8601); `%year%` and `%month%` are taken in UTC. */
  date: string;
  /** The primary category's slug; `null`: none. */
  category: string | null;
  /** The ancestors' slugs, root first. */
  parents: readonly string[];
}

/** A segment as core encodes it: UTF-8, uppercase hex, RFC 3986 unreserved kept. */
function encodeSegment(raw: string): string {
  return encodeURIComponent(raw).replace(
    /[!'()*]/g,
    (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

/**
 * The path `structure` gives `sample` (always ending in `/`), or `null` when core would
 * refuse the structure for `kind`.
 */
export function permalinkPreview(
  structure: string,
  sample: PermalinkSample,
  kind: StructureKind = "post",
): string | null {
  if (checkStructure(structure, kind) !== null) return null;
  const date = new Date(sample.date);
  const segments: string[] = [];
  for (const part of structure.slice(1, -1).split("/")) {
    switch (part) {
      case "%slug%":
        segments.push(sample.slug);
        break;
      case "%year%":
        segments.push(String(date.getUTCFullYear()).padStart(4, "0"));
        break;
      case "%month%":
        segments.push(String(date.getUTCMonth() + 1).padStart(2, "0"));
        break;
      case "%category%":
        segments.push(sample.category ?? UNCATEGORIZED);
        break;
      case "%parent_path%":
        segments.push(...sample.parents);
        break;
      default:
        segments.push(part);
    }
  }
  return `/${segments.map((s) => `${encodeSegment(s)}/`).join("")}`;
}

/** `structure` with `token` added as its last segment. */
export function appendToken(structure: string, token: PermalinkToken): string {
  const base =
    structure.trim() === "" ? "/" : structure.endsWith("/") ? structure : `${structure}/`;
  return `${base.startsWith("/") ? "" : "/"}${base}${token}/`;
}
