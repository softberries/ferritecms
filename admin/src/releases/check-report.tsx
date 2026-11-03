// A release check's report (design 15 §4.5): one line per entry with what would happen
// (ready, a conflict, waiting for approval, a missing right, an invalid value), and a
// summary. Also the per-entry part of a failed release and of a refused apply or schedule.

import { CircleAlertIcon, CircleCheckIcon } from "lucide-react";
import { cn } from "cn";
import { useTranslation } from "react-i18next";
import type { EntryCheck } from "@/api/releases";

export interface CheckReportProps {
  entries: EntryCheck[];
  /** Item titles by id (the release's entries). */
  titles: Map<string, string>;
  /** The region's heading is shown (the inline report), else only the lines. */
  heading?: boolean;
  /** Only the entries that need attention. */
  problemsOnly?: boolean;
  className?: string;
}

function useOutcomeText() {
  const { t } = useTranslation("releases");
  return (e: EntryCheck): string => {
    switch (e.outcome) {
      case "ok":
        return t("check.outcome.ok");
      case "conflict":
        return t("check.outcome.conflict", { version: e.current_version ?? "?" });
      case "needs_approval":
        return t("check.outcome.needs_approval", { step: e.step ? `: ${e.step}` : "" });
      case "forbidden":
        return t("check.outcome.forbidden", { capability: e.capability ?? "?" });
      default:
        return t("check.outcome.invalid", {
          message: [e.field, e.message].filter(Boolean).join(": ") || e.outcome,
        });
    }
  };
}

export function CheckReport({
  entries,
  titles,
  heading = true,
  problemsOnly = false,
  className,
}: CheckReportProps) {
  const { t } = useTranslation("releases");
  const text = useOutcomeText();
  const problems = entries.filter((e) => e.outcome !== "ok");
  const shown = problemsOnly ? problems : entries;
  const list = (
    <ul className="flex flex-col gap-1.5">
      {shown.map((e) => (
        <li key={e.item} data-outcome={e.outcome} className="flex items-start gap-2 text-sm">
          {e.outcome === "ok" ? (
            <CircleCheckIcon
              aria-hidden="true"
              className="mt-0.5 size-4 shrink-0 text-status-published"
            />
          ) : (
            <CircleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
          )}
          <span className="min-w-0">
            <span className="font-medium text-foreground">
              {titles.get(e.item) ?? t("entries.untitled")}
            </span>
            <span className="text-muted-foreground"> — {text(e)}</span>
          </span>
        </li>
      ))}
    </ul>
  );
  if (!heading) return <div className={className}>{list}</div>;
  return (
    <section
      aria-label={t("check.label")}
      data-slot="check-report"
      className={cn("flex flex-col gap-3 rounded-lg border bg-muted/30 p-4", className)}
    >
      <p className="text-sm font-medium text-foreground">
        {problems.length === 0
          ? t("check.clean")
          : t("check.problems", { count: problems.length, total: entries.length })}
      </p>
      {list}
    </section>
  );
}
