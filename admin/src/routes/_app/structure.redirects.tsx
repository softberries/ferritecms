// /admin/structure/redirects: the redirects (M2b-2 Task 13; `structure/redirects/`).
// Needs `manage_redirects` (admins; the server refuses anyone else). `?q=` holds the
// search (trimmed, at most 200 characters, the API's limit); `?new=true` opens the New
// redirect dialog (the palette's "New redirect"). Both replace the history entry.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { parseRedirectsSearch } from "@/structure/redirects/redirects-search";
import { RedirectsScreen } from "@/structure/redirects/redirects-table";

export const Route = createFileRoute("/_app/structure/redirects")({
  validateSearch: parseRedirectsSearch,
  staticData: { crumb: (t) => t("nav.redirects") },
  component: RedirectsRoute,
});

function RedirectsRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_redirects")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <RedirectsScreen
      search={search}
      setSearch={(patch) => {
        void navigate({
          to: "/structure/redirects",
          search: (prev: Record<string, unknown>) => parseRedirectsSearch({ ...prev, ...patch }),
          replace: true,
        });
      }}
    />
  );
}
