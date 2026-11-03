// /admin/reviews: the review queue (M6-3, design 15 §11.4; `reviews/review-queue.tsx`).
// `?tab=assigned|open|mine`; without `edit_others` the server lists the viewer's own
// submissions only, and "All open" is not offered.

import { createFileRoute } from "@tanstack/react-router";
import type { QueueTab } from "@/api/reviews";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { ReviewQueue } from "@/reviews/review-queue";

const TABS: readonly QueueTab[] = ["assigned", "open", "mine"];

interface ReviewsSearch {
  tab?: QueueTab;
}

export const Route = createFileRoute("/_app/reviews/")({
  validateSearch: (raw: Record<string, unknown>): ReviewsSearch => {
    const tab = TABS.find((x) => x === raw.tab);
    return tab ? { tab } : {};
  },
  staticData: { crumb: (t) => t("nav.reviews") },
  component: ReviewsRoute,
});

function ReviewsRoute() {
  const search = Route.useSearch();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("edit_own")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  const fallback: QueueTab = me.capabilities.includes("edit_others") ? "assigned" : "mine";
  return <ReviewQueue tab={search.tab ?? fallback} />;
}
