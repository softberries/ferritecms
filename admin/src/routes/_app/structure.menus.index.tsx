// /admin/structure/menus: the menus and the active theme's menu locations (M2b-2 Task 11;
// `structure/menus/`). Needs `manage_menus` (the server refuses anyone else's writes).
// `?new=true` opens the New menu dialog (the palette's "New menu").

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { MenusList } from "@/structure/menus/menus-list";

interface MenusSearch {
  new?: true;
}

export const Route = createFileRoute("/_app/structure/menus/")({
  validateSearch: (raw: Record<string, unknown>): MenusSearch =>
    raw.new === true || raw.new === "true" ? { new: true } : {},
  staticData: { crumb: (t) => t("nav.menus") },
  component: MenusRoute,
});

function MenusRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_menus")) {
    return (
      <div className="mx-auto w-full max-w-4xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <MenusList
      creating={search.new === true}
      setCreating={(open) => {
        void navigate({
          to: "/structure/menus",
          search: open ? { new: true } : {},
          replace: true,
        });
      }}
    />
  );
}
