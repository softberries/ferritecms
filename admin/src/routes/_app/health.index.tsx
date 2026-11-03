// /admin/health: the content health report (M6-4, design 16 §11.2; `health/`). Needs
// `edit_others` (editors and admins; the server refuses anyone else).

import { createFileRoute } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { HealthScreen } from "@/health/health-screen";

export const Route = createFileRoute("/_app/health/")({
  staticData: { crumb: (t) => t("nav.health") },
  component: HealthRoute,
});

function HealthRoute() {
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("edit_others")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return <HealthScreen />;
}
