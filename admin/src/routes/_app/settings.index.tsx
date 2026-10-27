// /admin/settings: the first group, General.

import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/_app/settings/")({
  beforeLoad: () => {
    throw redirect({ to: "/settings/$group", params: { group: "general" }, replace: true });
  },
});
