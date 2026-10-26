// /admin/media: the media library (M2b-2 Task 7; `media/library.tsx`). The URL holds the
// view, the search and filters, the Trash view and the item open in the detail sheet
// (`media/media-search.ts`); changing them replaces the history entry.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { MediaLibrary } from "@/media/library";
import { type MediaSearch, parseMediaSearch } from "@/media/media-search";

export const Route = createFileRoute("/_app/media/")({
  validateSearch: (raw: Record<string, unknown>): MediaSearch => parseMediaSearch(raw),
  staticData: { crumb: (t) => t("nav.media") },
  component: MediaRoute,
});

function MediaRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  return (
    <MediaLibrary
      search={search}
      me={me}
      setSearch={(patch) => {
        void navigate({
          to: "/media",
          search: (prev: MediaSearch) => parseMediaSearch({ ...prev, ...patch }),
          replace: true,
        });
      }}
    />
  );
}
