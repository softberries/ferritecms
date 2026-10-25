import { useQuery } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { ExternalLinkIcon, PenLineIcon, SparklesIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { metaQuery } from "@/api/meta";
import { settingsQuery } from "@/api/settings";
import { useMe } from "@/auth/session";
import { EmptyState } from "@/components/kit/empty-state";
import { PageHeader } from "@/components/kit/page-header";
import { Button } from "@/components/ui/button";

// Placeholder dashboard inside the shell (pre-flight F19); Task 12 replaces it. Its
// loader reads the settings (the site title), so a failing read shows the shell's error
// states.
export const Route = createFileRoute("/_app/")({
  staticData: { crumb: (t) => t("crumbs.dashboard") },
  loader: ({ context }) => context.queryClient.ensureQueryData(settingsQuery),
  component: IndexPage,
});

function IndexPage() {
  const { t } = useTranslation("shell");
  const me = useMe();
  const settings = Route.useLoaderData();
  const site = useQuery(metaQuery);
  const canWrite = me?.capabilities.includes("edit_own") ?? false;
  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-8">
      <PageHeader
        title={t("crumbs.dashboard")}
        description={me ? t("home.description", { name: me.display_name || me.login }) : undefined}
      />
      <EmptyState
        icon={SparklesIcon}
        title={
          settings["site.title"]
            ? `${settings["site.title"]} · ${t("home.soonTitle")}`
            : t("home.soonTitle")
        }
        body={t("home.soonBody")}
        action={
          <>
            {canWrite ? (
              <Button variant="outline" asChild>
                <Link to={"/content/post/new" as "/"}>
                  <PenLineIcon data-icon="inline-start" />
                  {t("home.write")}
                </Link>
              </Button>
            ) : null}
            {site.data ? (
              <Button variant="ghost" asChild>
                <a href={site.data.site_url} target="_blank" rel="noopener noreferrer">
                  <ExternalLinkIcon data-icon="inline-start" />
                  {t("home.viewSite")}
                </a>
              </Button>
            ) : null}
          </>
        }
      />
    </div>
  );
}
