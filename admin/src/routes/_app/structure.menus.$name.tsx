// /admin/structure/menus/$name: one menu's editor (M2b-2 Task 11; `structure/menus/`).
// The loader reads the server's current copy: the base the save's stale guard compares
// against. Needs `manage_menus`.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { type MenuDto, getMenu } from "@/api/menus";
import { ApiError } from "@/api/problem";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { MenuEditor } from "@/structure/menus/menu-editor";

export const Route = createFileRoute("/_app/structure/menus/$name")({
  staticData: {
    crumb: (t, data) => {
      const menu = data as MenuDto | undefined;
      return menu?.label || menu?.name || t("nav.menus");
    },
  },
  staleTime: 0,
  gcTime: 0,
  shouldReload: false,
  loader: async ({ params }): Promise<MenuDto> => {
    try {
      return await getMenu(params.name);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) throw notFound();
      throw e;
    }
  },
  component: MenuRoute,
});

function MenuRoute() {
  const menu = Route.useLoaderData();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_menus")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return <MenuEditor key={menu.name} initial={menu} />;
}
