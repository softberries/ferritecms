// /admin/users: the users (M2b-3 Task 7; `users/`). Needs `manage_users` (admins; the
// server refuses anyone else). The URL holds the search, the role and status chips, the
// user open in the sheet (`?id=`) and the New user dialog (`?new=true`, the palette's
// "New user"); changing them replaces the history entry.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { UsersScreen } from "@/users/users-list";
import { type UsersSearch, parseUsersSearch } from "@/users/users-search";

export const Route = createFileRoute("/_app/users/")({
  validateSearch: (raw: Record<string, unknown>): UsersSearch => parseUsersSearch(raw),
  staticData: { crumb: (t) => t("nav.users") },
  component: UsersRoute,
});

function UsersRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_users")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <UsersScreen
      search={search}
      me={me}
      setSearch={(patch) => {
        void navigate({
          to: "/users",
          search: (prev: Record<string, unknown>) => parseUsersSearch({ ...prev, ...patch }),
          replace: true,
        });
      }}
    />
  );
}
