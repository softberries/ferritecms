// /admin/comments: the comment moderation queue (M2b-2 Task 12; `comments/queue.tsx`).
// Needs `moderate_comments` (the server refuses anyone else). The URL holds the status
// tab and the item filter (`comments/comment-search.ts`); changing them replaces the
// history entry.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { CommentsQueue } from "@/comments/queue";
import { type CommentSearch, parseCommentSearch } from "@/comments/comment-search";

export const Route = createFileRoute("/_app/comments/")({
  validateSearch: (raw: Record<string, unknown>): CommentSearch => parseCommentSearch(raw),
  staticData: { crumb: (t) => t("nav.comments") },
  component: CommentsRoute,
});

function CommentsRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("moderate_comments")) {
    return (
      <div className="mx-auto w-full max-w-5xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <CommentsQueue
      search={search}
      setSearch={(patch) => {
        void navigate({
          to: "/comments",
          search: (prev: Record<string, unknown>) => parseCommentSearch({ ...prev, ...patch }),
          replace: true,
        });
      }}
    />
  );
}
