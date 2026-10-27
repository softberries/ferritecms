// /admin/account: the signed-in user's own account, for every role (`account/`). Reached
// from the user menu and the palette ("Account"); not in the sidebar.

import { createFileRoute } from "@tanstack/react-router";
import { AccountScreen } from "@/account/account-screen";
import { useMe } from "@/auth/session";

export const Route = createFileRoute("/_app/account")({
  staticData: { crumb: (t) => t("nav.account") },
  component: AccountRoute,
});

function AccountRoute() {
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  return <AccountScreen me={me} />;
}
