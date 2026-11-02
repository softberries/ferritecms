// "Manage this site with AI" (12 §8, §9; every role): until the viewer has connected an
// assistant, the MCP URL with Copy, the three-point security model and the way to the
// setup steps (Settings → AI assistants for administrators, the guide for everyone);
// once they have, how many apps are connected and a link to Account. With `mcp.enabled`
// off it says so (administrators get the way to turn it on).

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { cn } from "cn";
import {
  ArrowRightIcon,
  ArrowUpRightIcon,
  CircleUserRoundIcon,
  PlugZapIcon,
  PowerOffIcon,
  ScrollTextIcon,
  ShieldCheckIcon,
  SparklesIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { metaQuery } from "@/api/meta";
import { grantsQuery, mcpUrl } from "@/api/oauth";
import { settingsQuery } from "@/api/settings";
import { AI_GUIDE_URL } from "@/oauth/model";
import { CopyField } from "@/components/kit/copy-field";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Panel } from "./panel";

export interface AiCardProps {
  me: string;
  /** `manage_settings`: the settings group is theirs to open. */
  canSettings: boolean;
  className?: string;
}

export function AiCard({ me, canSettings, className }: AiCardProps) {
  const { t } = useTranslation("ai");
  const settings = useQuery(settingsQuery);
  const site = useQuery(metaQuery);
  // A failed read shows the invitation (the URL still works); no toast for it.
  const grants = useQuery({ ...grantsQuery(me, true), meta: INLINE_ERRORS });
  const off = settings.data?.["mcp.enabled"] === false;
  const count = grants.data?.length ?? 0;
  const wide = cn("lg:col-span-12", className);

  if (off) {
    return (
      <Panel
        title={t("dashboard.offTitle")}
        description={t("dashboard.offBody")}
        className={wide}
        action={
          canSettings ? (
            <Button asChild variant="outline">
              <Link to="/settings/$group" params={{ group: "ai-assistants" }}>
                <PowerOffIcon data-icon="inline-start" />
                {t("dashboard.offAction")}
              </Link>
            </Button>
          ) : undefined
        }
      >
        <p className="text-sm text-muted-foreground">{t("dashboard.offPaused")}</p>
      </Panel>
    );
  }

  if (grants.isPending) {
    return (
      <Panel title={t("dashboard.title")} busy className={wide}>
        <Skeleton className="h-16 w-full" />
      </Panel>
    );
  }

  if (count > 0) {
    return (
      <Panel
        title={t("dashboard.connectedTitle")}
        className={wide}
        action={
          <Button asChild variant="outline">
            <Link to="/account">
              {t("dashboard.manage")}
              <ArrowRightIcon data-icon="inline-end" />
            </Link>
          </Button>
        }
      >
        <div data-slot="ai-connected" className="flex items-center gap-4">
          <span className="flex size-11 shrink-0 items-center justify-center rounded-xl border bg-primary/8 text-primary">
            <SparklesIcon aria-hidden="true" className="size-5" />
          </span>
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="text-lg font-semibold text-foreground tabular-nums">
              {t("dashboard.connected", { count })}
            </span>
            <span className="text-sm text-muted-foreground">{t("dashboard.connectedBody")}</span>
          </div>
        </div>
      </Panel>
    );
  }

  return (
    <Panel
      title={t("dashboard.title")}
      description={t("dashboard.description")}
      className={cn(wide, "overflow-hidden")}
      action={
        <a
          href={AI_GUIDE_URL}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex min-h-11 items-center gap-1 rounded-sm text-sm font-medium text-primary underline-offset-4 hover:underline"
        >
          {t("dashboard.guide")}
          <ArrowUpRightIcon aria-hidden="true" className="size-4" />
        </a>
      }
    >
      <div
        data-slot="ai-invite"
        className="grid min-w-0 gap-5 md:grid-cols-[minmax(0,1fr)_minmax(0,26rem)] md:items-end"
      >
        <ul className="flex flex-wrap gap-2">
          {(
            [
              [CircleUserRoundIcon, "login"],
              [ShieldCheckIcon, "role"],
              [ScrollTextIcon, "audited"],
            ] as const
          ).map(([Icon, key]) => (
            <li
              key={key}
              title={t(`settings.safety.${key}.body`)}
              className="inline-flex h-7 items-center gap-1.5 rounded-full border bg-muted/50 px-2.5 text-xs font-medium text-foreground"
            >
              <Icon aria-hidden="true" className="size-3.5 text-primary" />
              <span className="sr-only">{t(`settings.safety.${key}.body`)}</span>
              <span aria-hidden="true">{t(`settings.safety.${key}.short`)}</span>
            </li>
          ))}
        </ul>
        <div className="flex min-w-0 flex-col gap-3">
          {site.data ? (
            <CopyField value={mcpUrl(site.data.site_url)} label={t("dashboard.url")} />
          ) : (
            <Skeleton className="h-16 w-full" />
          )}
          {canSettings ? (
            <Button asChild className="self-start">
              <Link to="/settings/$group" params={{ group: "ai-assistants" }}>
                <PlugZapIcon data-icon="inline-start" />
                {t("dashboard.setup")}
              </Link>
            </Button>
          ) : null}
        </div>
      </div>
    </Panel>
  );
}
