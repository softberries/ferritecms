// /admin/plugins: the installed plugins, the install flow and retained data (M2b-3
// Task 10; `plugins/`). Needs `manage_plugins` (admins; the server refuses anyone else).

import { createFileRoute } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { PluginsScreen } from "@/plugins/plugins-list";

/** `?tab=registry` opens the Registry tab (M6-2); anything else the installed list. */
export interface PluginsSearch {
  tab?: "registry";
}

export const Route = createFileRoute("/_app/plugins/")({
  staticData: { crumb: (t) => t("nav.plugins") },
  validateSearch: (search: Record<string, unknown>): PluginsSearch =>
    search.tab === "registry" ? { tab: "registry" } : {},
  component: PluginsRoute,
});

function PluginsRoute() {
  const { me: guarded } = Route.useRouteContext();
  const { tab } = Route.useSearch();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_plugins")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return <PluginsScreen tab={tab === "registry" ? "registry" : "installed"} />;
}
