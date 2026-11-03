// What the side panel's components share: whether the screen is wide enough for the
// panel to sit beside the writing column (else it is a sheet, 11 §4), and the
// taxonomies that apply to the item's type with their terms.

import { useQueries, useQuery } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";
import { type TaxonomyDto, type TermDto, taxonomiesQuery, termsQuery } from "@/api/taxonomies";

/** At this width and above the panel is a column beside the text; below, a sheet. */
export const WIDE_QUERY = "(min-width: 1024px)";
/**
 * At this width and above the live preview sits beside the text (a split view, design 15
 * §3.3); below, the bar's "Write | Preview" control swaps one for the other.
 */
export const SPLIT_QUERY = "(min-width: 1280px)";

function subscribeTo(query: string) {
  return (onChange: () => void): (() => void) => {
    if (typeof window.matchMedia !== "function") return () => {};
    const mql = window.matchMedia(query);
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  };
}

const matchesNow = (query: string) => () =>
  typeof window.matchMedia === "function" && window.matchMedia(query).matches;

const subscribeWide = subscribeTo(WIDE_QUERY);
const wideNow = matchesNow(WIDE_QUERY);
const subscribeSplit = subscribeTo(SPLIT_QUERY);
const splitNow = matchesNow(SPLIT_QUERY);

/** Whether the panel sits beside the text (`false` where `matchMedia` is missing). */
export function useWide(): boolean {
  return useSyncExternalStore(subscribeWide, wideNow, () => false);
}

/** Whether the live preview can sit beside the text (`SPLIT_QUERY`). */
export function useSplit(): boolean {
  return useSyncExternalStore(subscribeSplit, splitNow, () => false);
}

/** The taxonomies whose `content_types` include `type`, in the server's order. */
export function useTypeTaxonomies(type: string, enabled = true): TaxonomyDto[] {
  const all = useQuery({ ...taxonomiesQuery, enabled });
  return (all.data ?? []).filter((tax) => tax.content_types.includes(type));
}

/** Every term of each of `taxonomies`, by id (for the publish summary). */
export function useTermsById(taxonomies: readonly TaxonomyDto[]): Map<string, TermDto> {
  const lists = useQueries({ queries: taxonomies.map((tax) => termsQuery(tax.name)) });
  const out = new Map<string, TermDto>();
  for (const list of lists) for (const term of list.data ?? []) out.set(term.id, term);
  return out;
}
