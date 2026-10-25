// The dashboard (07 §3, M2b P16/P17): post count tiles, publishing activity, the actor's
// recent drafts, what needs attention and site health. Every card loads and fails on
// its own; a card the actor may not see is not rendered, so its data is never
// requested. The loader reads only the settings (the site title in the header, shared
// with the shell's sidebar), so a failing settings read shows the shell's route states.

import { createFileRoute } from "@tanstack/react-router";
import { ExternalLinkIcon } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { metaQuery } from "@/api/meta";
import { settingsQuery } from "@/api/settings";
import { useMe } from "@/auth/session";
import { PageHeader } from "@/components/kit/page-header";
import { Button } from "@/components/ui/button";
import { ActivityChart } from "@/dashboard/activity-chart";
import { NeedsAttention } from "@/dashboard/needs-attention";
import { attentionAccess, hasAttention } from "@/dashboard/queries";
import { RecentDrafts } from "@/dashboard/recent-drafts";
import { SiteHealth } from "@/dashboard/site-health";
import { Tiles } from "@/dashboard/tiles";

export const Route = createFileRoute("/_app/")({
  staticData: { crumb: (t) => t("crumbs.dashboard") },
  loader: ({ context }) => context.queryClient.ensureQueryData(settingsQuery),
  component: Dashboard,
});

function Dashboard() {
  const { t } = useTranslation("dashboard");
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  const settings = Route.useLoaderData();
  const site = useQuery(metaQuery);
  const caps = me.capabilities;
  const canWrite = caps.includes("edit_own");
  const access = attentionAccess(caps);
  const attention = hasAttention(access);
  const health = caps.includes("manage_system");
  const name = me.display_name || me.login;
  const title = settings["site.title"];

  return (
    <div className="mx-auto flex w-full max-w-7xl flex-col gap-6">
      <PageHeader
        title={t("title")}
        description={title ? t("greetingSite", { name, site: title }) : t("greeting", { name })}
        actions={
          site.data ? (
            <Button variant="outline" asChild>
              <a href={site.data.site_url} target="_blank" rel="noopener noreferrer">
                <ExternalLinkIcon data-icon="inline-start" />
                {t("viewSite")}
              </a>
            </Button>
          ) : null
        }
      />
      <Tiles />
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-12">
        <ActivityChart canWrite={canWrite} />
        <RecentDrafts me={me.id} canWrite={canWrite} />
        {attention ? (
          <NeedsAttention access={access} className={health ? "lg:col-span-5" : "lg:col-span-12"} />
        ) : null}
        {health ? (
          <SiteHealth
            plugins={caps.includes("manage_plugins")}
            className={attention ? "lg:col-span-7" : "lg:col-span-12"}
          />
        ) : null}
      </div>
    </div>
  );
}
