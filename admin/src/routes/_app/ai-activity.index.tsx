// /admin/ai-activity: what AI assistants changed over MCP (M6-5, design 17 §11;
// `ai-activity/`). Every role with `read_admin` sees their own sets; `manage_users` adds
// every user's. `?agent=<grant or token id>` narrows to one connection (the links of
// Connected apps and API tokens).

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { type ActivitySearch, ActivityList } from "@/ai-activity/activity-list";
import { useMe } from "@/auth/session";

export const Route = createFileRoute("/_app/ai-activity/")({
  validateSearch: (raw: Record<string, unknown>): ActivitySearch => ({
    ...(typeof raw.agent === "string" && /^[0-9a-f-]{36}$/i.test(raw.agent)
      ? { agent: raw.agent }
      : {}),
    ...(raw.all === true || raw.all === "true" ? { all: true as const } : {}),
    ...(raw.open === true || raw.open === "true" ? { open: true } : {}),
  }),
  staticData: { crumb: (t) => t("nav.aiActivity") },
  component: ActivityRoute,
});

function ActivityRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  return (
    <ActivityList
      search={search}
      admin={me.capabilities.includes("manage_users")}
      setSearch={(next) => void navigate({ to: "/ai-activity", search: next, replace: true })}
    />
  );
}
