// /admin/structure/taxonomies/$name: one taxonomy's terms (M2b-2 Task 10): a tree for a
// hierarchical taxonomy, a table for a flat one (`structure/taxonomies/`). The loader
// reads the server's current copy: the base the settings dialog's stale guard compares
// against. `manage_terms` edits the terms, `manage_settings` the taxonomy itself.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { ApiError } from "@/api/problem";
import { type TaxonomyDto, getTaxonomy } from "@/api/taxonomies";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { TaxonomyScreen } from "@/structure/taxonomies/taxonomy-screen";

export const Route = createFileRoute("/_app/structure/taxonomies/$name")({
  staticData: {
    crumb: (t, data) => (data as TaxonomyDto | undefined)?.label ?? t("nav.taxonomies"),
  },
  staleTime: 0,
  gcTime: 0,
  shouldReload: false,
  loader: async ({ params }): Promise<TaxonomyDto> => {
    try {
      return await getTaxonomy(params.name);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) throw notFound();
      throw e;
    }
  },
  component: TaxonomyRoute,
});

function TaxonomyRoute() {
  const taxonomy = Route.useLoaderData();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  const canSettings = me.capabilities.includes("manage_settings");
  const canTerms = me.capabilities.includes("manage_terms");
  if (!canSettings && !canTerms) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <TaxonomyScreen
      key={taxonomy.name}
      taxonomy={taxonomy}
      canSettings={canSettings}
      canTerms={canTerms}
    />
  );
}
