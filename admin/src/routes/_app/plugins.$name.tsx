// /admin/plugins/$name: one plugin, its metrics, settings and permissions (M2b-3 Task 10;
// `plugins/plugin-detail.tsx`). Needs `manage_plugins`; an unknown name is the not-found
// screen.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { type PluginDto, pluginQuery } from "@/api/plugins";
import { ApiError } from "@/api/problem";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { PluginDetail } from "@/plugins/plugin-detail";

export const Route = createFileRoute("/_app/plugins/$name")({
  staticData: {
    crumb: (t, data) => (data as PluginDto | null | undefined)?.name ?? t("nav.plugins"),
  },
  loader: async ({ params, context }): Promise<PluginDto | null> => {
    try {
      return await context.queryClient.ensureQueryData(pluginQuery(params.name));
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) throw notFound();
      // Without `manage_plugins` the screen says so (the server refused the read).
      if (e instanceof ApiError && e.status === 403) return null;
      throw e;
    }
  },
  component: PluginRoute,
});

function PluginRoute() {
  const { name } = Route.useParams();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_plugins")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return <PluginDetail key={name} name={name} />;
}
