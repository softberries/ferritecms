// /admin/structure/types/$name: one content type, its settings and its field builder
// (M2b-2 Task 9; `structure/types/type-form.tsx`). The loader always reads the server's
// current copy: it is the base the stale guard compares against before a save. `media`
// has its own screen and is not edited here.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { ApiError } from "@/api/problem";
import { type ContentTypeDto, getType } from "@/api/types";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { TypeForm } from "@/structure/types/type-form";
import { WorkflowEditor } from "@/structure/types/workflow-editor";

const NOT_EDITED = new Set(["media"]);

export const Route = createFileRoute("/_app/structure/types/$name")({
  staticData: {
    crumb: (t, data) => (data as ContentTypeDto | undefined)?.label ?? t("nav.contentTypes"),
  },
  staleTime: 0,
  gcTime: 0,
  shouldReload: false,
  loader: async ({ params }): Promise<ContentTypeDto> => {
    if (NOT_EDITED.has(params.name)) throw notFound();
    try {
      return await getType(params.name);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) throw notFound();
      throw e;
    }
  },
  component: TypeRoute,
});

function TypeRoute() {
  const type = Route.useLoaderData();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_settings")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  // A new copy (another type, or the same one loaded again) starts the form afresh. The
  // approval workflow (M6-3) is saved on its own, under the type's settings.
  return (
    <div className="flex flex-col gap-6">
      <TypeForm key={type.name} initial={type} />
      <div className="mx-auto w-full max-w-6xl">
        <WorkflowEditor key={type.name} type={type.name} />
      </div>
    </div>
  );
}
