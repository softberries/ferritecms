// Schedule… a release (design 15 §4.6): the editor's schedule picker (a calendar and a
// 24-hour time on the browser's clock), then `POST …/schedule`. The server checks the
// release first; a refusal (`release-not-ready`) shows the per-entry report. Admins get
// the override disclosure (a scheduled apply then needs no approvals).

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ApiError } from "@/api/problem";
import {
  type EntryCheck,
  type ReleaseDetail,
  problemEntries,
  scheduleRelease,
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
import {
  type WallClock,
  browserTimeZone,
  defaultScheduleWall,
  formatWhen,
  isPast,
  wallClockToUtc,
} from "@/editor/panel/publishing";
import { SchedulePicker } from "@/editor/panel/schedule-picker";
import { problemText } from "@/structure/taxonomies/terms-model";
import { CheckReport } from "./check-report";
import { NO_OVERRIDE, type OverrideState, overrideIncomplete, overrideReason } from "./override";
import { OverrideField } from "./override-field";

export interface ScheduleDialogProps {
  release: ReleaseDetail;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  canOverride: boolean;
  onScheduled: (next: ReleaseDetail) => void;
  onStale: () => void;
}

export function ScheduleDialog(props: ScheduleDialogProps) {
  return (
    <Dialog open={props.open} onOpenChange={props.onOpenChange}>
      <DialogContent
        data-slot="schedule-release-dialog"
        className="max-h-[calc(100dvh-1rem)] overflow-y-auto sm:max-w-xl"
      >
        {props.open ? <Body {...props} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function Body({ release, onOpenChange, canOverride, onScheduled, onStale }: ScheduleDialogProps) {
  const { t } = useTranslation("releases");
  const { t: te } = useTranslation("editor");
  const [zone] = useState(browserTimeZone);
  const [wall, setWall] = useState<WallClock>(() => defaultScheduleWall(zone));
  const [override, setOverride] = useState<OverrideState>(NO_OVERRIDE);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<EntryCheck[]>([]);
  const titles = new Map(release.entries.map((e) => [e.item_id, e.title]));
  const at = wallClockToUtc(wall, zone);
  const pickError = at === null ? te("schedule.invalid") : isPast(at) ? te("schedule.past") : null;

  const schedule = async () => {
    if (!at || pickError) return;
    setBusy(true);
    setError(null);
    setReport([]);
    try {
      const next = await scheduleRelease(release.id, release.version, at, overrideReason(override));
      toast.success(t("scheduleDialog.scheduled", { when: formatWhen(at) }), {
        id: "release-schedule",
      });
      onScheduled(next);
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
        entries.length > 0
          ? t("scheduleDialog.notReady")
          : t("detail.actionFailed", { detail: problemText(e) }),
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>{t("scheduleDialog.title", { title: release.title })}</DialogTitle>
        <DialogDescription>{t("scheduleDialog.description")}</DialogDescription>
      </DialogHeader>
      <div className="flex min-w-0 flex-col gap-4">
        <SchedulePicker
          value={wall}
          onChange={(v) => {
            setWall(v);
            setError(null);
          }}
          timeZone={zone}
          error={pickError}
          disabled={busy}
        />
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
          {t("scheduleDialog.cancel")}
        </Button>
        <Button
          type="button"
          data-slot="schedule-release-confirm"
          disabled={busy || pickError !== null || overrideIncomplete(override)}
          onClick={() => void schedule()}
        >
          {busy
            ? t("scheduleDialog.scheduling")
            : t("scheduleDialog.confirm", { when: at ? formatWhen(at) : "" })}
        </Button>
      </DialogFooter>
    </>
  );
}
