// The revert confirm (design 17 §11): it runs the request as a dry run first and lists
// what will happen and every conflict with its reason; with `changed_since` conflicts
// the confirm stays disabled until "Overwrite later edits" is ticked (sends `force`);
// `gone`, `too_large` and `later_change` stay listed as not revertable. A 409 from a
// race checks again.

import { useQueryClient } from "@tanstack/react-query";
import { formatDistanceToNowStrict } from "date-fns";
import { TriangleAlertIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { type RevertConflictDto, type RevertReportDto, revertChangeSet } from "@/api/change-sets";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { Alert } from "@/auth/auth-layout";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { DESTRUCTIVE } from "@/lib/destructive";
import { problemText } from "@/structure/taxonomies/terms-model";
import { conflictText, forceable } from "./model";

export interface RevertTarget {
  setId: string;
  /** `undefined`: every change not reverted yet. */
  seqs?: number[];
}

type Check =
  | { state: "checking" }
  | { state: "ready"; report: RevertReportDto }
  | { state: "conflicts"; conflicts: RevertConflictDto[] }
  | { state: "failed"; message: string };

function conflictsOf(e: unknown): RevertConflictDto[] | null {
  if (!(e instanceof ApiError) || e.slug !== "change-conflict") return null;
  const report = (e.problem as { report?: { conflicts?: RevertConflictDto[] } }).report;
  return report?.conflicts ?? [];
}

function bodyOf(target: RevertTarget, dryRun: boolean, force: boolean) {
  return { changes: target.seqs, dry_run: dryRun, force };
}

/** The dry run of `target`: what it would revert, or its conflicts. */
async function dryRun(target: RevertTarget): Promise<Check> {
  try {
    const report = await revertChangeSet(target.setId, bodyOf(target, true, false));
    return { state: "ready", report };
  } catch (e) {
    const conflicts = conflictsOf(e);
    return conflicts
      ? { state: "conflicts", conflicts }
      : { state: "failed", message: problemText(e) };
  }
}

export function RevertDialog({
  target,
  names,
  onClose,
}: {
  target: RevertTarget | null;
  /** A user's display name by id (the set's people). */
  names: (id: string) => string | undefined;
  onClose: () => void;
}) {
  const { t } = useTranslation("ai");
  const queryClient = useQueryClient();
  // The check and the force tick belong to the target they were made for, so a new
  // target starts over without resetting state in an effect.
  const [checked, setChecked] = useState<{ for: RevertTarget; check: Check } | null>(null);
  const [forcedFor, setForcedFor] = useState<RevertTarget | null>(null);
  const [pending, setPending] = useState(false);
  const check: Check = checked && checked.for === target ? checked.check : { state: "checking" };
  const force = target !== null && forcedFor === target;

  const run = useCallback((of: RevertTarget) => {
    void dryRun(of).then((result) => setChecked({ for: of, check: result }));
  }, []);

  useEffect(() => {
    if (target) run(target);
  }, [target, run]);

  const ago = (at: string) => formatDistanceToNowStrict(new Date(at), { addSuffix: true });
  const conflicts = check.state === "conflicts" ? check.conflicts : [];
  const canForce = forceable(conflicts);
  const blocked = conflicts.filter((c) => c.reason !== "changed_since");
  const ready =
    !pending && (check.state === "ready" || (check.state === "conflicts" && canForce && force));

  const confirm = async () => {
    if (!target || !ready) return;
    setPending(true);
    try {
      const report = await revertChangeSet(target.setId, bodyOf(target, false, force));
      toast.success(t("activity.dialog.done", { count: report.reverted.length }));
      void queryClient.invalidateQueries({ queryKey: qk.changeSets.all });
      onClose();
    } catch (e) {
      if (conflictsOf(e)) {
        toast.error(t("activity.dialog.raced"));
        setChecked({ for: target, check: { state: "checking" } });
        run(target);
      } else {
        reportApiError(e);
        const seq =
          e instanceof ApiError
            ? (e.problem as { report?: { change?: number } }).report?.change
            : undefined;
        setChecked({
          for: target,
          check: {
            state: "failed",
            message:
              seq != null
                ? t("activity.dialog.failed", { seq, reason: problemText(e) })
                : problemText(e),
          },
        });
      }
    } finally {
      setPending(false);
    }
  };

  const count =
    check.state === "ready"
      ? check.report.reverted.length
      : (target?.seqs?.length ?? 0) || conflicts.length;

  return (
    <AlertDialog open={target !== null} onOpenChange={(open) => !open && !pending && onClose()}>
      <AlertDialogContent data-slot="revert-dialog" className="sm:max-w-lg">
        <AlertDialogHeader>
          <AlertDialogTitle>
            {target?.seqs ? t("activity.dialog.titleOne") : t("activity.dialog.titleAll")}
          </AlertDialogTitle>
          <AlertDialogDescription>{t("activity.dialog.asYou")}</AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex min-w-0 flex-col gap-3 text-sm" aria-live="polite">
          {check.state === "checking" ? (
            <p aria-busy="true" className="text-muted-foreground">
              {t("activity.dialog.checking")}
            </p>
          ) : null}
          {check.state === "ready" ? (
            <>
              <p data-slot="revert-will">{t("activity.dialog.will", { count })}</p>
              {check.report.notes.length > 0 ? (
                <ul className="list-disc ps-5 text-muted-foreground">
                  {check.report.notes.map((n) => (
                    <li key={`${n.seq}-${n.text}`}>{n.text}</li>
                  ))}
                </ul>
              ) : null}
            </>
          ) : null}
          {check.state === "conflicts" ? (
            <div data-slot="revert-conflicts" className="flex flex-col gap-2">
              <p className="flex items-start gap-2 font-medium text-foreground">
                <TriangleAlertIcon
                  aria-hidden="true"
                  className="mt-0.5 size-4 shrink-0 text-warning"
                />
                {blocked.length > 0 ? t("activity.dialog.blocked") : t("activity.dialog.conflicts")}
              </p>
              <ul className="flex flex-col gap-1.5">
                {conflicts.map((c) => (
                  <li
                    key={`${c.seq}-${c.reason}`}
                    data-slot="revert-conflict"
                    data-reason={c.reason}
                    className="flex min-w-0 flex-col rounded-md border px-3 py-2"
                  >
                    <span className="truncate font-medium text-foreground">
                      {c.object.label || c.object.id}
                    </span>
                    <span className="text-muted-foreground">{conflictText(t, c, names, ago)}</span>
                  </li>
                ))}
              </ul>
              {canForce ? (
                <div className="flex items-start gap-2.5 pt-1">
                  <Checkbox
                    id="revert-force"
                    checked={force}
                    onCheckedChange={(v) => setForcedFor(v === true ? target : null)}
                    className="mt-0.5"
                  />
                  <Label htmlFor="revert-force" className="flex flex-col items-start gap-0.5">
                    <span>{t("activity.dialog.force")}</span>
                    <span className="font-normal text-muted-foreground">
                      {t("activity.dialog.forceHint")}
                    </span>
                  </Label>
                </div>
              ) : null}
            </div>
          ) : null}
          {check.state === "failed" ? <Alert tone="error">{check.message}</Alert> : null}
        </div>
        <AlertDialogFooter>
          <Button variant="outline" onClick={onClose} disabled={pending}>
            {t("activity.dialog.cancel")}
          </Button>
          <Button
            data-slot="revert-confirm"
            className={DESTRUCTIVE}
            onClick={() => void confirm()}
            disabled={!ready}
            aria-busy={pending}
          >
            {pending ? t("activity.dialog.reverting") : t("activity.dialog.confirm")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
