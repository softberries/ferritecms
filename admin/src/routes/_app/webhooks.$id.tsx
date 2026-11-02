// /admin/webhooks/$id: one webhook, its settings and its delivery log (M6-6, design 18
// §10.1; `webhooks/webhook-detail.tsx`). Needs `manage_webhooks`; an unknown id is the
// not-found screen.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { ApiError } from "@/api/problem";
import { type WebhookDto, webhookQuery } from "@/api/webhooks";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { WebhookDetail } from "@/webhooks/webhook-detail";

export const Route = createFileRoute("/_app/webhooks/$id")({
  staticData: {
    crumb: (t, data) => (data as WebhookDto | null | undefined)?.name ?? t("nav.webhooks"),
  },
  loader: async ({ params, context }): Promise<WebhookDto | null> => {
    try {
      return await context.queryClient.ensureQueryData(webhookQuery(params.id));
    } catch (e) {
      if (e instanceof ApiError && (e.status === 404 || e.status === 400)) throw notFound();
      // Without `manage_webhooks` the screen says so (the server refused the read).
      if (e instanceof ApiError && e.status === 403) return null;
      throw e;
    }
  },
  component: WebhookRoute,
});

function WebhookRoute() {
  const { id } = Route.useParams();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_webhooks")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return <WebhookDetail key={id} id={id} />;
}
