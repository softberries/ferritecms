// /admin/connect?request=<id>: the consent screen of an AI assistant's connection (12 §8;
// `oauth/consent-screen.tsx`). Outside the shell, in the sign-in frame: the browser comes
// here from `/oauth/authorize`. No session → sign in first and come back (`next`); a
// session that must change its password changes it first, then comes back too.

import { createFileRoute, redirect } from "@tanstack/react-router";
import { adminPath, loadSession, useMe } from "@/auth/session";
import { ConsentScreen } from "@/oauth/consent-screen";

interface ConnectSearch {
  request?: string;
}

export const Route = createFileRoute("/connect")({
  validateSearch: (search: Record<string, unknown>): ConnectSearch =>
    typeof search.request === "string" && search.request !== "" ? { request: search.request } : {},
  staticData: { crumb: (t) => t("crumbs.connect") },
  beforeLoad: async (ctx) => {
    const me = await loadSession(ctx);
    if (me.must_reset) {
      throw redirect({
        to: "/change-password",
        search: { next: adminPath(ctx.location) },
        replace: true,
      });
    }
    return { me };
  },
  component: ConnectPage,
});

function ConnectPage() {
  const { request } = Route.useSearch();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  return <ConsentScreen requestId={request} me={me} />;
}
