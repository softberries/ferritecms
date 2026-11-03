// Cells of the content list that are components of their own (fast refresh needs them in
// a module that exports only components).

import { cn } from "cn";
import { format } from "date-fns";
import { useTranslation } from "react-i18next";
import { isContentStatus } from "@/api/content";
import type { SchemaContentSummaryDto } from "@/api/schema";
import { StatusBadge } from "@/components/kit/status-badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { SiteLocales } from "@/lib/locales";

/** The status badge, with the scheduled time under it for scheduled items. */
export function StatusCell({ item }: { item: SchemaContentSummaryDto }) {
  const { t } = useTranslation("content");
  if (!isContentStatus(item.status)) return <span>{item.status}</span>;
  return (
    <div className="flex flex-col items-start gap-0.5 py-1.5">
      <StatusBadge status={item.status} />
      {item.status === "scheduled" && item.scheduled_at ? (
        <time
          dateTime={item.scheduled_at}
          className="text-xs text-muted-foreground tabular-nums"
          title={format(new Date(item.scheduled_at), "PPpp")}
        >
          {t("row.scheduledFor", { at: format(new Date(item.scheduled_at), "PP p") })}
        </time>
      ) : null}
    </div>
  );
}

/**
 * A parent the viewer may not read (someone else's draft above a published child): a
 * muted label, with why in a tooltip (and for screen readers in the label itself).
 */
export function UnavailableParent({ className }: { className?: string }) {
  const { t } = useTranslation("content");
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          tabIndex={0}
          data-slot="parent-unavailable"
          className={className ?? "text-muted-foreground italic"}
        >
          {t("row.parentUnavailable")}
          <span className="sr-only">. {t("row.parentUnavailableWhy")}</span>
        </span>
      </TooltipTrigger>
      <TooltipContent>{t("row.parentUnavailableWhy")}</TooltipContent>
    </Tooltip>
  );
}

/**
 * A row's languages (M6-1): its own locale first, then every other locale of the site
 * with its translation's status, or dashed when there is none.
 */
export function LocaleChips({
  item,
  locales,
}: {
  item: SchemaContentSummaryDto;
  locales: SiteLocales;
}) {
  const { t } = useTranslation("content");
  const { t: tc } = useTranslation();
  const others = new Map(item.translations.map((tr) => [tr.locale, tr.status]));
  return (
    <ul data-slot="locale-chips" className="flex flex-wrap items-center gap-1">
      <li
        className="rounded-sm border border-border bg-muted px-1.5 font-mono text-xs font-semibold"
        title={locales.label(item.locale)}
      >
        {item.locale}
      </li>
      {locales.all
        .filter((l) => l.tag !== item.locale)
        .map((l) => {
          const status = others.get(l.tag);
          const label = status
            ? t("locale.chip", {
                locale: l.label,
                status: isContentStatus(status) ? tc(`status.${status}`) : status,
              })
            : t("locale.missing", { locale: l.label });
          return (
            <li
              key={l.tag}
              title={label}
              aria-label={label}
              className={cn(
                "rounded-sm border px-1.5 font-mono text-xs",
                status
                  ? "border-border text-foreground"
                  : "border-dashed border-border text-muted-foreground",
              )}
            >
              {l.tag}
            </li>
          );
        })}
    </ul>
  );
}
