// What the side panel's components share: whether the screen is wide enough for the
// panel to sit beside the writing column (else it is a sheet, 11 §4), and the
// taxonomies that apply to the item's type with their terms.

import { useQueries, useQuery } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";
import { type TaxonomyDto, type TermDto, taxonomiesQuery, termsQuery } from "@/api/taxonomies";

/** At this width and above the panel is a column beside the text; below, a sheet. */
export const WIDE_QUERY = "(min-width: 1024px)";

function subscribeWide(onChange: () => void): () => void {
  if (typeof window.matchMedia !== "function") return () => {};
  const mql = window.matchMedia(WIDE_QUERY);
  mql.addEventListener("change", onChange);
  return () => mql.removeEventListener("change", onChange);
}

const wideNow = () =>
  typeof window.matchMedia === "function" && window.matchMedia(WIDE_QUERY).matches;

/** Whether the panel sits beside the text (`false` where `matchMedia` is missing). */
export function useWide(): boolean {
  return useSyncExternalStore(subscribeWide, wideNow, () => false);
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
