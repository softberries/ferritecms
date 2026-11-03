// /admin/webhooks: the outgoing webhooks (M6-6, design 18 §10.1; `webhooks/`). Needs
// `manage_webhooks` (admins; the server refuses anyone else). `?new=true` opens the New
// webhook dialog (the palette's "New webhook").

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { WebhooksScreen } from "@/webhooks/webhooks-list";

interface WebhooksSearch {
  new?: true;
}

export const Route = createFileRoute("/_app/webhooks/")({
  validateSearch: (raw: Record<string, unknown>): WebhooksSearch =>
    raw.new === true || raw.new === "true" ? { new: true } : {},
  staticData: { crumb: (t) => t("nav.webhooks") },
  component: WebhooksRoute,
});

function WebhooksRoute() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  if (!me.capabilities.includes("manage_webhooks")) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ForbiddenState />
      </div>
    );
  }
  return (
    <WebhooksScreen
      creating={search.new === true}
      setCreating={(open) => {
        void navigate({ to: "/webhooks", search: open ? { new: true } : {}, replace: true });
      }}
    />
  );
}
