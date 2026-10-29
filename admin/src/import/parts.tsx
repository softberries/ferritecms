// Pieces the run panel and the runs table share: the dry-run badge and a run's report
// links (the page in a new tab, the JSON as a download).

import { DownloadIcon, ExternalLinkIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { type ImportRunDto, reportUrls } from "@/api/import";
import { Button } from "@/components/ui/button";

export function DryRunBadge() {
  const { t } = useTranslation("import");
  return (
    <span
      data-slot="dry-run"
      className="inline-flex h-6 shrink-0 items-center rounded-full border border-dashed px-2 text-xs font-medium whitespace-nowrap text-muted-foreground"
    >
      {t("run.dryRun")}
    </span>
  );
}

/** The run panel's report actions: Download JSON, then "Open report" as the primary. */
export function ReportActions({ run, partial }: { run: ImportRunDto; partial: boolean }) {
  const { t } = useTranslation("import");
  const urls = reportUrls(run.id);
  return (
    <>
      <Button asChild variant="outline" className="pointer-coarse:h-11">
        <a href={urls.json} download={`import-${run.id}.json`}>
          <DownloadIcon data-icon="inline-start" />
          {t("run.downloadJson")}
        </a>
      </Button>
      <Button asChild className="pointer-coarse:h-11">
        <a href={urls.html} target="_blank" rel="noopener noreferrer">
          {partial ? t("run.openPartialReport") : t("run.openReport")}
          <ExternalLinkIcon data-icon="inline-end" />
          <span className="sr-only">{t("run.newTab")}</span>
        </a>
      </Button>
    </>
  );
}

/** A runs-table row's report links, named by the run's source. */
export function ReportLinks({ run }: { run: ImportRunDto }) {
  const { t } = useTranslation("import");
  const urls = reportUrls(run.id);
  return (
    <span className="flex items-center gap-1">
      <Button asChild variant="ghost" size="sm" className="pointer-coarse:h-11">
        <a
          href={urls.html}
          target="_blank"
          rel="noopener noreferrer"
          aria-label={t("runs.htmlOn", { source: run.source })}
        >
          {t("runs.html")}
          <ExternalLinkIcon data-icon="inline-end" />
        </a>
      </Button>
      <Button asChild variant="ghost" size="sm" className="pointer-coarse:h-11">
        <a
          href={urls.json}
          download={`import-${run.id}.json`}
          aria-label={t("runs.jsonOn", { source: run.source })}
        >
          <DownloadIcon data-icon="inline-start" />
          {t("runs.json")}
        </a>
      </Button>
    </span>
  );
}
