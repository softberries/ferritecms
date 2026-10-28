// A plugin's permission report (05 §3, 07 §1 H-P1): what it may do, as the server words
// it (`items[].text`, each with its `detail` lines: types, keys, hosts, routes), the
// high-risk items first in a warning style, on an update the items the installed grant
// does not cover marked "New" (`added`), the limits it asks for with those above the site
// default marked, and the facts that identify the upload (plugin, version, artifact
// hash). Every string from the report or the manifest is the plugin author's text and
// renders as text.

import { cn } from "cn";
import { ShieldAlertIcon, ShieldCheckIcon, ShieldIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PermissionReportDto } from "@/api/plugins";
import type { SchemaGrantReportDto, SchemaLimitItemDto } from "@/api/schema";
import { Badge } from "@/components/ui/badge";
import { uiLocale } from "@/lib/locale";
import { type ShownItem, formatMemory, shownItems } from "./plugin-model";

export interface PermissionReportProps {
  report: SchemaGrantReportDto | PermissionReportDto;
  /**
   * Mark the `added` items "New"; by default for an upload's report that is an update
   * (a new plugin's items are all added, so marking them says nothing).
   */
  markAdded?: boolean;
  className?: string;
}

export function PermissionReport({ report, markAdded, className }: PermissionReportProps) {
  const { t } = useTranslation("admin");
  const shown = shownItems(report, markAdded ?? ("update" in report && report.update));
  const publisher = "publisher" in report ? report.publisher : null;
  return (
    <section data-slot="permission-report" className={cn("flex min-w-0 flex-col gap-4", className)}>
      <div
        role="group"
        aria-label={t("plugins.report.factsLabel")}
        data-slot="report-facts"
        className="rounded-lg border bg-muted/40 px-3 py-2.5"
      >
        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
          <dt className="text-muted-foreground">{t("plugins.report.plugin")}</dt>
          <dd className="font-medium break-words text-foreground">{report.plugin}</dd>
          <dt className="text-muted-foreground">{t("plugins.report.version")}</dt>
          <dd className="text-foreground tabular-nums">{report.version}</dd>
          {publisher ? (
            <>
              <dt className="text-muted-foreground">{t("plugins.report.publisher")}</dt>
              <dd className="break-words text-foreground">
                {t("plugins.report.unverified", { publisher })}
              </dd>
            </>
          ) : null}
          <dt className="text-muted-foreground">{t("plugins.report.artifact")}</dt>
          <dd className="font-mono text-xs leading-5 break-all text-foreground">
            {report.artifact}
          </dd>
        </dl>
      </div>

      <div className="flex flex-col gap-2">
        <h3 className="text-sm font-medium text-foreground">{t("plugins.report.canTitle")}</h3>
        {shown.length === 0 ? (
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <ShieldCheckIcon aria-hidden="true" className="size-4 shrink-0 text-success" />
            {t("plugins.report.nothing")}
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {shown.map((s, i) => (
              <PermissionRow key={`${s.item.key}:${i}`} shown={s} />
            ))}
          </ul>
        )}
      </div>

      {report.limits.length > 0 ? <Limits limits={report.limits} /> : null}
    </section>
  );
}

function PermissionRow({ shown }: { shown: ShownItem }) {
  const { t } = useTranslation("admin");
  const { item, isNew, newDetail } = shown;
  const high = item.high_risk;
  const medium = !high && item.risk === "medium";
  const Icon = high ? ShieldAlertIcon : ShieldIcon;
  return (
    <li
      data-slot="permission"
      data-key={item.key}
      data-risk={high ? "high" : item.risk}
      data-new={isNew ? "true" : "false"}
      className={cn(
        "flex min-w-0 items-start gap-3 rounded-lg border px-3 py-2.5",
        high ? "border-warning/50 bg-warning/8" : "bg-card",
      )}
    >
      <Icon
        aria-hidden="true"
        className={cn(
          "mt-0.5 size-4 shrink-0",
          high ? "text-warning" : medium ? "text-info" : "text-muted-foreground",
        )}
      />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span className="min-w-0 text-sm break-words text-foreground">{item.text}</span>
          {high ? (
            <Badge variant="outline" className="border-warning/60 text-foreground">
              {t("plugins.report.highRisk")}
            </Badge>
          ) : medium ? (
            <Badge variant="outline">{t("plugins.report.mediumRisk")}</Badge>
          ) : null}
          {isNew ? <Badge>{t("plugins.report.new")}</Badge> : null}
        </div>
        {item.detail.length > 0 ? (
          <ul aria-label={t("plugins.report.detail")} className="flex min-w-0 flex-wrap gap-1.5">
            {item.detail.map((d, i) => {
              const fresh = newDetail.has(d);
              return (
                <li
                  key={`${d}:${i}`}
                  {...(fresh ? { "data-new": "true" } : {})}
                  className={cn(
                    "max-w-full rounded-md border px-1.5 py-0.5 font-mono text-xs break-all",
                    fresh
                      ? "border-primary/50 bg-primary/6 text-foreground"
                      : "bg-muted/60 text-muted-foreground",
                  )}
                >
                  {d}
                  {fresh ? <span className="sr-only"> {t("plugins.report.newDetail")}</span> : null}
                </li>
              );
            })}
          </ul>
        ) : null}
      </div>
    </li>
  );
}

function Limits({ limits }: { limits: readonly SchemaLimitItemDto[] }) {
  const { t } = useTranslation("admin");
  const number = new Intl.NumberFormat(uiLocale());
  const show = (l: SchemaLimitItemDto, v: number) =>
    l.name === "memory" ? formatMemory(v) : number.format(v);
  return (
    <div className="flex flex-col gap-2">
      <h3 className="text-sm font-medium text-foreground">{t("plugins.report.limitsTitle")}</h3>
      <div
        tabIndex={0}
        role="group"
        aria-label={t("plugins.report.limitsTitle")}
        className="overflow-x-auto rounded-lg border focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        <table className="w-full text-sm">
          <thead className="bg-muted/40 text-left text-xs text-muted-foreground">
            <tr>
              <th scope="col" className="px-3 py-2 font-medium">
                {t("plugins.report.limit")}
              </th>
              <th scope="col" className="px-3 py-2 text-right font-medium">
                {t("plugins.report.requested")}
              </th>
              <th scope="col" className="px-3 py-2 text-right font-medium">
                {t("plugins.report.siteDefault")}
              </th>
            </tr>
          </thead>
          <tbody>
            {limits.map((l) => (
              <tr
                key={l.name}
                data-slot="limit"
                data-above={l.above_default ? "true" : "false"}
                className="border-t"
              >
                <th scope="row" className="px-3 py-2 text-left font-normal text-foreground">
                  <span className="flex flex-wrap items-center gap-2">
                    {l.name === "memory" || l.name === "fuel"
                      ? t(`plugins.report.limitName.${l.name}`)
                      : l.name}
                    {l.above_default ? (
                      <Badge variant="outline" className="border-warning/60 text-foreground">
                        {t("plugins.report.aboveDefault")}
                      </Badge>
                    ) : null}
                  </span>
                </th>
                <td className="px-3 py-2 text-right whitespace-nowrap text-foreground tabular-nums">
                  {show(l, l.requested)}
                </td>
                <td className="px-3 py-2 text-right whitespace-nowrap text-muted-foreground tabular-nums">
                  {show(l, l.default)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
