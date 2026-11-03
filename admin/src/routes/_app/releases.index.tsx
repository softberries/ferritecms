// /admin/releases: the releases (M6-3, design 15 §11.3; `releases/`). Every role that may
// write content may make one (`edit_own`); the server lists what the viewer may see.
// `?status=` filters; `?new=true` opens New release (the palette's "New release").

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { isReleaseStatus, type ReleaseStatus } from "@/api/releases";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { ReleasesScreen } from "@/releases/releases-list";

interface ReleasesSearch {
  status?: ReleaseStatus;
  new?: true;
}

export const Route = createFileRoute("/_app/releases/")({
  validateSearch: (raw: Record<string, unknown>): ReleasesSearch => ({
    ...(typeof raw.status === "string" && isReleaseStatus(raw.status)
      ? { status: raw.status }
      : {}),
    ...(raw.new === true || raw.new === "true" ? { new: true as const } : {}),
  }),
  staticData: { crumb: (t) => t("nav.releases") },
  component: ReleasesRoute,
});

function ReleasesRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("edit_own")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <ReleasesScreen
      filter={search.status ?? "all"}
      creating={search.new === true}
      setCreating={(open) => {
        void navigate({
          to: "/releases",
          search: {
            ...(search.status ? { status: search.status } : {}),
            ...(open ? { new: true } : {}),
          },
          replace: true,
        });
      }}
    />
  );
}
