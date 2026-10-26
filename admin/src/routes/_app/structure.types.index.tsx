// /admin/structure/types: the content types (M2b-2 Task 9; `structure/types/`). Only for
// `manage_settings` (the nav shows it to no one else; the server refuses their writes).
// `?new=true` opens the New type dialog (the palette's "New content type").

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { TypesList } from "@/structure/types/types-list";

interface TypesSearch {
  new?: true;
}

export const Route = createFileRoute("/_app/structure/types/")({
  validateSearch: (raw: Record<string, unknown>): TypesSearch =>
    raw.new === true || raw.new === "true" ? { new: true } : {},
  staticData: { crumb: (t) => t("nav.contentTypes") },
  component: TypesRoute,
});

function TypesRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_settings")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <TypesList
      creating={search.new === true}
      setCreating={(open) => {
        void navigate({
          to: "/structure/types",
          search: open ? { new: true } : {},
          replace: true,
        });
      }}
    />
  );
}
