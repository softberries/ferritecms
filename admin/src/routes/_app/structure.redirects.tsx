// /admin/structure/redirects: the redirects (M2b-2 Task 13; `structure/redirects/`).
// Needs `manage_redirects` (admins; the server refuses anyone else). `?q=` holds the
// search (trimmed, at most 200 characters, the API's limit); `?new=true` opens the New
// redirect dialog (the palette's "New redirect"). Both replace the history entry.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { MAX_REDIRECT_QUERY } from "@/api/redirects";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { type RedirectsSearch, RedirectsScreen } from "@/structure/redirects/redirects-table";

/** `validateSearch`: valid members only, defaults omitted. The router JSON-decodes each
 * value, so `q=2026` arrives as a number. */
function parseRedirectsSearch(raw: Record<string, unknown>): RedirectsSearch {
  const out: RedirectsSearch = {};
  if (typeof raw.q === "string" || typeof raw.q === "number") {
    const q = Array.from(String(raw.q).trim()).slice(0, MAX_REDIRECT_QUERY).join("");
    if (q) out.q = q;
  }
  if (raw.new === true || raw.new === "true") out.new = true;
  return out;
}

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
      <div className="mx-auto w-full max-w-4xl">
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
