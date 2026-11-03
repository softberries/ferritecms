// The side panel's "Checks" (design 16 §11.1): counts by severity in a live region, then
// each finding with its message, "Show" (selects and scrolls to it in the text), "Add alt
// text" for images, and "Suppress…"; suppressed ones collapse under "Suppressed (n)".

import { AlertTriangleIcon, CircleAlertIcon, InfoIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { CheckResponse, FindingDto } from "@/api/quality";
import { Button } from "@/components/ui/button";
import { findingId, groupFindings, messageFor, titleFor, type Severity } from "../quality/findings";
import { SuppressDialog } from "../quality/suppress-dialog";

export interface QualitySectionProps {
  report: CheckResponse | null;
  failed: boolean;
  /** The item is saved (suppressions need its id). */
  canSuppress: boolean;
  onShow: (f: FindingDto) => void;
  onSuppress: (f: FindingDto, input: { key: string; reason: string }) => Promise<void>;
  onUnsuppress: (f: FindingDto) => Promise<void>;
  /** The finding to bring forward (`?check=<rule>:<key>`), if any. */
  focus?: string | null;
}

const ICON: Record<Severity, typeof InfoIcon> = {
  error: CircleAlertIcon,
  warning: AlertTriangleIcon,
  notice: InfoIcon,
};

export function QualitySection({
  report,
  failed,
  canSuppress,
  onShow,
  onSuppress,
  onUnsuppress,
  focus,
}: QualitySectionProps) {
  const { t } = useTranslation("quality");
  const [suppressing, setSuppressing] = useState<FindingDto | null>(null);
  const [showSuppressed, setShowSuppressed] = useState(false);
  const groups = report ? groupFindings(report.findings) : null;
  return (
    <section
      aria-labelledby="panel-quality"
      data-slot="panel-quality"
      className="flex flex-col gap-3"
    >
      <h3 id="panel-quality" className="text-sm font-semibold">
        {t("panel.title")}
      </h3>
      <p aria-live="polite" className="text-sm text-muted-foreground" data-slot="quality-counts">
        {failed
          ? t("panel.failed")
          : !report
            ? t("panel.waiting")
            : report.findings.length === 0
              ? t("panel.none")
              : t("panel.counts", report.counts)}
      </p>
      {groups?.bySeverity.map((g) => (
        <ul
          key={g.severity}
          className="flex flex-col gap-2"
          aria-label={t(`severity.${g.severity}`)}
        >
          {g.findings.map((f) => (
            <FindingRow
              key={findingId(f)}
              f={f}
              focused={focus === `${f.rule}:${f.key}`}
              onShow={() => onShow(f)}
              {...(canSuppress ? { onSuppress: () => setSuppressing(f) } : {})}
            />
          ))}
        </ul>
      ))}
      {groups && groups.suppressed.length > 0 ? (
        <div className="flex flex-col gap-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="min-h-11 justify-start px-0"
            aria-expanded={showSuppressed}
            onClick={() => setShowSuppressed((v) => !v)}
          >
            {t("panel.suppressed", { count: groups.suppressed.length })}
          </Button>
          {showSuppressed ? (
            <ul className="flex flex-col gap-2">
              {groups.suppressed.map((f) => (
                <li key={findingId(f)} className="rounded-lg border p-2 text-sm">
                  <p>{messageFor(t, f)}</p>
                  <p className="text-xs text-muted-foreground">
                    {t("panel.suppressedBy", { reason: f.suppressed?.reason ?? "" })}
                  </p>
                  {canSuppress ? (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="min-h-11 px-0"
                      onClick={() => void onUnsuppress(f)}
                    >
                      {t("panel.unsuppress")}
                    </Button>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
      <SuppressDialog
        finding={suppressing}
        onClose={() => setSuppressing(null)}
        onSubmit={async (input) => {
          if (suppressing) await onSuppress(suppressing, input);
        }}
      />
    </section>
  );
}

function FindingRow({
  f,
  focused,
  onShow,
  onSuppress,
}: {
  f: FindingDto;
  focused: boolean;
  onShow: () => void;
  onSuppress?: () => void;
}) {
  const { t } = useTranslation("quality");
  const Icon = ICON[f.severity];
  const where =
    f.location.kind === "field"
      ? t("panel.inField", { field: f.location.field ?? "" })
      : f.location.kind === "featured_image"
        ? t("panel.featured")
        : null;
  return (
    <li
      data-slot="quality-finding"
      data-rule={f.rule}
      data-focused={focused || undefined}
      className="flex flex-col gap-1 rounded-lg border p-2 text-sm data-focused:ring-2 data-focused:ring-ring"
    >
      <div className="flex items-start gap-2">
        <Icon
          aria-hidden="true"
          className={
            f.severity === "error"
              ? "mt-0.5 size-4 shrink-0 text-destructive"
              : f.severity === "warning"
                ? "mt-0.5 size-4 shrink-0 text-warning"
                : "mt-0.5 size-4 shrink-0 text-muted-foreground"
          }
        />
        <div className="min-w-0">
          <p className="font-medium">
            <span className="sr-only">{t(`severity.${f.severity}`)}: </span>
            {titleFor(t, f.rule)}
          </p>
          <p className="text-muted-foreground">{messageFor(t, f)}</p>
          {where ? <p className="text-xs text-muted-foreground">{where}</p> : null}
        </div>
      </div>
      <div className="flex flex-wrap gap-1">
        {f.location.kind === "body" ? (
          <Button type="button" variant="ghost" size="sm" className="min-h-11" onClick={onShow}>
            {f.rule.startsWith("image-alt") ? t("panel.fixAlt") : t("panel.show")}
          </Button>
        ) : null}
        {onSuppress ? (
          <Button type="button" variant="ghost" size="sm" className="min-h-11" onClick={onSuppress}>
            {t("panel.suppress")}
          </Button>
        ) : null}
      </div>
    </li>
  );
}
