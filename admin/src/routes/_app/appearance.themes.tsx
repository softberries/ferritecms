// /admin/appearance/themes: the themes (M2b-3 Task 9; `appearance/`). Needs
// `manage_themes` (admins; the server refuses anyone else).

import { createFileRoute } from "@tanstack/react-router";
import { ThemesScreen } from "@/appearance/themes-screen";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";

export const Route = createFileRoute("/_app/appearance/themes")({
  staticData: { crumb: (t) => t("nav.appearance") },
  component: ThemesRoute,
});

function ThemesRoute() {
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_themes")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return <ThemesScreen />;
}
