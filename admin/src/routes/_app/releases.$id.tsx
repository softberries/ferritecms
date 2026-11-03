// /admin/releases/$id: one release (M6-3, design 15 §11.3; `releases/release-detail.tsx`).
// An unknown id, or one the viewer may not see, is the not-found screen (the server
// answers 404 for both).

import { createFileRoute, notFound } from "@tanstack/react-router";
import { ApiError } from "@/api/problem";
import { type ReleaseDetail, releaseQuery } from "@/api/releases";
import { ReleaseDetailScreen } from "@/releases/release-detail";

export const Route = createFileRoute("/_app/releases/$id")({
  staticData: {
    crumb: (t, data) => (data as ReleaseDetail | null | undefined)?.title ?? t("nav.releases"),
  },
  loader: async ({ params, context }): Promise<ReleaseDetail | null> => {
    try {
      return await context.queryClient.ensureQueryData(releaseQuery(params.id));
    } catch (e) {
      if (e instanceof ApiError && (e.status === 404 || e.status === 400)) throw notFound();
      if (e instanceof ApiError && e.status === 403) return null;
      throw e;
    }
  },
  component: ReleaseRoute,
});

function ReleaseRoute() {
  const { id } = Route.useParams();
  return <ReleaseDetailScreen key={id} id={id} />;
}
