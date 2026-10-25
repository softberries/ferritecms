// /admin/_probe: a route with a real data loader (`GET /settings`), so the shell's
// route-level states (permission denied, failed load) have a deterministic trigger in
// the E2E suite (`shell.spec.ts` intercepts the read). Every other screen of M2b-1
// loads per card or has no loader; once a screen with a real loader exists (Task 13's
// content list), shell.spec moves there and this route goes. Behind the `_app` guard,
// linked from nowhere (like the styleguide).

import { createFileRoute } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { settingsQuery } from "@/api/settings";
import { PageHeader } from "@/components/kit/page-header";

export const Route = createFileRoute("/_app/_probe")({
  staticData: { crumb: (t) => t("crumbs.probe") },
  loader: ({ context }) => context.queryClient.ensureQueryData(settingsQuery),
  component: Probe,
});

function Probe() {
  const { t } = useTranslation("shell");
  const settings = Route.useLoaderData();
  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-6">
      <PageHeader
        title={t("probe.title")}
        description={t("probe.body", { site: settings["site.title"] ?? "" })}
      />
    </div>
  );
}
