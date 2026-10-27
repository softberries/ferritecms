// /admin/system/$tab: status, nodes, jobs and the audit log (M2b-3 Task 11; `system/`).
// Needs `manage_system` (admins; the server refuses anyone else). An unknown tab is the
// not-found screen, like any unknown admin URL. The jobs and audit filters live in the
// search (`system-model.ts`); changing them replaces the history entry.

import { createFileRoute, notFound, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import {
  type SystemSearch,
  type SystemTab,
  isSystemTab,
  parseSystemSearch,
} from "@/system/system-model";
import { SystemScreen } from "@/system/system-screen";

export const Route = createFileRoute("/_app/system/$tab")({
  validateSearch: (raw: Record<string, unknown>): SystemSearch => parseSystemSearch(raw),
  staticData: { crumb: (t) => t("nav.system") },
  loader: ({ params }): SystemTab => {
    if (!isSystemTab(params.tab)) throw notFound();
    return params.tab;
  },
  component: SystemRoute,
});

function SystemRoute() {
  const tab = Route.useLoaderData();
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_system")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <SystemScreen
      tab={tab}
      search={search}
      setSearch={(patch) => {
        void navigate({
          to: "/system/$tab",
          params: { tab },
          search: (prev: Record<string, unknown>) => parseSystemSearch({ ...prev, ...patch }),
          replace: true,
        });
      }}
    />
  );
}
