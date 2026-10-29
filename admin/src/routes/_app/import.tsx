// /admin/import: the WordPress import (M3-2 P32; `import/`). Needs `import` (admins; the
// server refuses anyone else). `?job=` holds the job the run panel follows, so a reload
// keeps following it; starting an import or dismissing a finished one replaces the entry.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { ImportScreen } from "@/import/import-screen";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export interface ImportSearch {
  job?: string;
}

export const Route = createFileRoute("/_app/import")({
  validateSearch: (raw: Record<string, unknown>): ImportSearch =>
    typeof raw.job === "string" && UUID.test(raw.job) ? { job: raw.job } : {},
  staticData: { crumb: (t) => t("nav.import") },
  component: ImportRoute,
});

function ImportRoute() {
  const { job } = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("import")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <ImportScreen
      job={job}
      setJob={(next) => {
        void navigate({ to: "/import", search: next ? { job: next } : {}, replace: true });
      }}
    />
  );
}
