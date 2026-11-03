// Apply now (design 15 §4.4, §11.3): a confirm dialog listing what goes live (each entry:
// its changes, published or unpublished), with the override disclosure for admins. The
// apply runs in one transaction on the server; a refusal keeps the dialog open with the
// reason and, for conflicts or missing approvals, the per-entry report.

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ApiError } from "@/api/problem";
import {
  type EntryCheck,
  type ReleaseDetail,
  applyRelease,
  entryActions,
  problemEntries,
} from "@/api/releases";
import { Alert } from "@/auth/auth-layout";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { problemText } from "@/structure/taxonomies/terms-model";
import { CheckReport } from "./check-report";
import { NO_OVERRIDE, type OverrideState, overrideIncomplete, overrideReason } from "./override";
import { OverrideField } from "./override-field";

export interface ApplyDialogProps {
  release: ReleaseDetail;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Offer the override (the viewer is an admin). */
  canOverride: boolean;
  onApplied: (next: ReleaseDetail) => void;
  /** A `412`: the release changed meanwhile. */
  onStale: () => void;
}

export function ApplyDialog(props: ApplyDialogProps) {
  return (
    <Dialog open={props.open} onOpenChange={props.onOpenChange}>
      <DialogContent
        data-slot="apply-dialog"
        className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-lg"
      >
        {props.open ? <Body {...props} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function useEntryLiveText() {
  const { t } = useTranslation("releases");
  return (e: Parameters<typeof entryActions>[0]): string => {
    const a = entryActions(e);
    if (a.changes && a.publish) return t("apply.changesAndPublish");
    if (a.changes && a.unpublish) return t("apply.changesAndUnpublish");
    if (a.publish) return t("apply.publish");
    if (a.unpublish) return t("apply.unpublish");
    return t("apply.changes");
  };
}

function Body({ release, onOpenChange, canOverride, onApplied, onStale }: ApplyDialogProps) {
  const { t } = useTranslation("releases");
  const live = useEntryLiveText();
  const [override, setOverride] = useState<OverrideState>(NO_OVERRIDE);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<EntryCheck[]>([]);
  const titles = new Map(release.entries.map((e) => [e.item_id, e.title]));

  const apply = async () => {
    setBusy(true);
    setError(null);
    setReport([]);
    try {
      const next = await applyRelease(release.id, release.version, overrideReason(override));
      toast.success(t("apply.applied", { title: release.title }), { id: "release-apply" });
      onApplied(next);
      onOpenChange(false);
    } catch (e) {
      if (e instanceof ApiError && e.status === 412) {
        onOpenChange(false);
        onStale();
        return;
      }
      const entries = e instanceof ApiError ? problemEntries(e.problem) : [];
      setReport(entries);
      setError(
        entries.length > 0 ? t("apply.conflicts") : t("apply.failed", { detail: problemText(e) }),
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>{t("apply.title", { title: release.title })}</DialogTitle>
        <DialogDescription>{t("apply.description")}</DialogDescription>
      </DialogHeader>
      <div className="flex min-w-0 flex-col gap-4">
        <ul
          aria-label={t("apply.live")}
          className="flex flex-col divide-y rounded-lg border bg-muted/30 text-sm"
        >
          {release.entries.map((e) => (
            <li key={e.item_id} className="flex items-baseline justify-between gap-3 px-3 py-2">
              <span className="min-w-0 truncate font-medium text-foreground">
                {e.title || t("entries.untitled")}
              </span>
              <span className="shrink-0 text-muted-foreground">{live(e)}</span>
            </li>
          ))}
        </ul>
        {canOverride ? (
          <OverrideField value={override} onChange={setOverride} disabled={busy} />
        ) : null}
        {error ? <Alert tone="error">{error}</Alert> : null}
        {report.length > 0 ? (
          <CheckReport entries={report} titles={titles} heading={false} problemsOnly />
        ) : null}
      </div>
      <DialogFooter>
        <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          {t("apply.cancel")}
        </Button>
        <Button
          type="button"
          data-slot="apply-confirm"
          disabled={busy || overrideIncomplete(override) || release.entries.length === 0}
          onClick={() => void apply()}
        >
          {busy ? t("apply.applying") : t("apply.confirm")}
        </Button>
      </DialogFooter>
    </>
  );
}
