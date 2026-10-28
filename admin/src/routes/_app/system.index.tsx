// /admin/system: the first tab, Status.

import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/_app/system/")({
  beforeLoad: () => {
    throw redirect({ to: "/system/$tab", params: { tab: "status" }, replace: true });
  },
});
