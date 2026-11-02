// /admin/settings/<group> (`manage_settings`; 07 §3 Settings): one page with a rail of
// groups (a scrolling row of tabs on phones). General, Reading, Permalinks and Comments
// edit `GET/PATCH /settings`, each saving only its own keys; Media sizes and Mail are
// read-only (P50, P49). AI assistants (12 §8) saves `mcp.enabled` and shows how to connect.

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { settingsQuery } from "@/api/settings";
import type { MeDto } from "@/auth/session";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { TAB_LIST, tabLinkClass } from "@/components/kit/tab-styles";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "cn";
import { AiAssistantsGroup } from "./ai-assistants";
import { CommentsGroup } from "./comments";
import { GeneralGroup } from "./general";
import { GROUP_ICONS, GROUP_KEYS, SETTINGS_GROUPS, type SettingsGroup } from "./groups";
import { MailGroup } from "./mail";
import { MediaSizesGroup } from "./media-sizes";
import { PermalinksGroup } from "./permalinks";
import { ReadingGroup } from "./reading";

export function SettingsScreen({ group, me }: { group: SettingsGroup; me: MeDto }) {
  const { t } = useTranslation("admin");
  return (
    <div data-slot="settings-screen" className="mx-auto flex w-full max-w-5xl flex-col gap-6">
      <PageHeader title={t("settings.title")} description={t("settings.description")} />
      <div className="grid grid-cols-[minmax(0,1fr)] gap-6 lg:grid-cols-[12rem_minmax(0,1fr)] lg:gap-10">
        <GroupRail current={group} />
        <section
          data-slot="settings-group"
          data-group={group}
          aria-labelledby="settings-group-title"
          className="min-w-0"
        >
          <GroupBody key={group} group={group} me={me} />
        </section>
      </div>
    </div>
  );
}

function GroupBody({ group, me }: { group: SettingsGroup; me: MeDto }) {
  const settings = useQuery({ ...settingsQuery, meta: INLINE_ERRORS });
  if (group === "media-sizes") {
    return <MediaSizesGroup canThemes={me.capabilities.includes("manage_themes")} />;
  }
  if (group === "mail") return <MailGroup me={me} />;
  if (settings.isError) {
    return <ErrorState error={settings.error} retry={() => void settings.refetch()} />;
  }
  if (!settings.data) return <GroupSkeleton />;
  switch (group) {
    case "general":
      return <GeneralGroup settings={settings.data} />;
    case "reading":
      return <ReadingGroup settings={settings.data} />;
    case "permalinks":
      return <PermalinksGroup settings={settings.data} />;
    case "comments":
      return <CommentsGroup settings={settings.data} />;
    case "ai-assistants":
      return <AiAssistantsGroup settings={settings.data} />;
  }
}

function GroupSkeleton() {
  return (
    <div aria-busy="true" className="flex flex-col gap-5">
      <div className="flex flex-col gap-2">
        <Skeleton className="h-6 w-32" />
        <Skeleton className="h-4 w-72 max-w-full" />
      </div>
      <Card>
        <CardContent className="flex flex-col gap-6">
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
        </CardContent>
      </Card>
    </div>
  );
}

/** The groups: a vertical rail from `lg`, a scrolling row of tabs below it. */
function GroupRail({ current }: { current: SettingsGroup }) {
  const { t } = useTranslation("admin");
  const label = (g: SettingsGroup) => t(`settings.groups.${GROUP_KEYS[g]}.title`);
  return (
    <>
      <nav aria-label={t("settings.rail")} className="min-w-0 lg:hidden">
        <ScrollFade activeSelector={`[data-group-link="${current}"]`} activeKey={current}>
          <ul className={TAB_LIST}>
            {SETTINGS_GROUPS.map((g) => (
              <li key={g}>
                <Link
                  to="/settings/$group"
                  params={{ group: g }}
                  aria-current={g === current ? "page" : undefined}
                  data-group-link={g}
                  className={tabLinkClass(g === current)}
                >
                  {label(g)}
                </Link>
              </li>
            ))}
          </ul>
        </ScrollFade>
      </nav>
      <nav aria-label={t("settings.rail")} className="max-lg:hidden">
        <ul className="sticky top-[calc(var(--sticky-top,0px)+1.5rem)] flex flex-col gap-0.5">
          {SETTINGS_GROUPS.map((g) => {
            const Icon = GROUP_ICONS[g];
            const active = g === current;
            return (
              <li key={g}>
                <Link
                  to="/settings/$group"
                  params={{ group: g }}
                  aria-current={active ? "page" : undefined}
                  className={cn(
                    "flex h-control items-center gap-2.5 rounded-md px-2.5 text-sm font-medium transition-colors",
                    active
                      ? "bg-muted text-foreground [&>svg]:text-primary"
                      : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
                  )}
                >
                  <Icon aria-hidden="true" className="size-4 shrink-0" />
                  {label(g)}
                </Link>
              </li>
            );
          })}
        </ul>
      </nav>
    </>
  );
}
