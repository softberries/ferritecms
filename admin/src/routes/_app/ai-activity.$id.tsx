// /admin/ai-activity/$id: one change set with its changes and diffs, and revert (M6-5,
// design 17 §11; `ai-activity/change-set-screen.tsx`). An unknown id, or another user's
// set without `manage_users`, is the not-found screen.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { ChangeSetScreen } from "@/ai-activity/change-set-screen";
import { type ChangeSetDetailDto, changeSetQuery } from "@/api/change-sets";
import { ApiError } from "@/api/problem";

export const Route = createFileRoute("/_app/ai-activity/$id")({
  staticData: {
    crumb: (t, data) =>
      (data as ChangeSetDetailDto | null | undefined)?.set.agent.client ?? t("nav.aiActivity"),
  },
  loader: async ({ params, context }): Promise<ChangeSetDetailDto | null> => {
    try {
      return await context.queryClient.ensureQueryData(changeSetQuery(params.id));
    } catch (e) {
      if (e instanceof ApiError && (e.status === 404 || e.status === 400)) throw notFound();
      if (e instanceof ApiError && e.status === 403) return null;
      throw e;
    }
  },
  component: ChangeSetRoute,
});

function ChangeSetRoute() {
  const { id } = Route.useParams();
  return <ChangeSetScreen key={id} id={id} />;
}
