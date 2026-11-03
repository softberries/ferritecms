// /admin/plugins/publishers: the publisher keys and revocation (M6-2, design 14 §10;
// `plugins/publishers.tsx`). Needs `manage_plugins`. `publishers` is a reserved plugin
// name, so this static route never hides a plugin's page.

import { createFileRoute } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { PublishersScreen } from "@/plugins/publishers";

export const Route = createFileRoute("/_app/plugins/publishers")({
  staticData: { crumb: (t) => t("crumbs.publishers") },
  component: PublishersRoute,
});

function PublishersRoute() {
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_plugins")) {
    return (
      <div className="mx-auto w-full max-w-4xl">
        <ForbiddenState />
      </div>
    );
  }
  return <PublishersScreen />;
}
