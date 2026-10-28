// /admin/settings/$group: the site settings (M2b-3 Task 8; `settings/`). Needs
// `manage_settings` (admins; the server refuses anyone else). An unknown group is the
// not-found screen, like any unknown admin URL.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { type SettingsGroup, isSettingsGroup } from "@/settings/groups";
import { SettingsScreen } from "@/settings/settings-screen";

export const Route = createFileRoute("/_app/settings/$group")({
  staticData: { crumb: (t) => t("nav.settings") },
  loader: ({ params }): SettingsGroup => {
    if (!isSettingsGroup(params.group)) throw notFound();
    return params.group;
  },
  component: SettingsRoute,
});

function SettingsRoute() {
  const group = Route.useLoaderData();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_settings")) {
    return (
      <div className="mx-auto w-full max-w-5xl">
        <ForbiddenState />
      </div>
    );
  }
  return <SettingsScreen group={group} me={me} />;
}
