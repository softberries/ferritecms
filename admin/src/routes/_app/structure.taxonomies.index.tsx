// /admin/structure/taxonomies: the taxonomies (M2b-2 Task 10; `structure/taxonomies/`).
// `manage_settings` creates and edits taxonomies; `manage_terms` alone gets the list as
// links to the terms (the server refuses anyone else's writes). `?new=true` opens the
// New taxonomy dialog (the palette's "New taxonomy").

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { TaxonomyList } from "@/structure/taxonomies/taxonomy-list";

interface TaxonomiesSearch {
  new?: true;
}

export const Route = createFileRoute("/_app/structure/taxonomies/")({
  validateSearch: (raw: Record<string, unknown>): TaxonomiesSearch =>
    raw.new === true || raw.new === "true" ? { new: true } : {},
  staticData: { crumb: (t) => t("nav.taxonomies") },
  component: TaxonomiesRoute,
});

function TaxonomiesRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  const canSettings = me.capabilities.includes("manage_settings");
  if (!canSettings && !me.capabilities.includes("manage_terms")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <TaxonomyList
      canManage={canSettings}
      creating={canSettings && search.new === true}
      setCreating={(open) => {
        void navigate({
          to: "/structure/taxonomies",
          search: open ? { new: true } : {},
          replace: true,
        });
      }}
    />
  );
}
