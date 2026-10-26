// The leave guard's question (`leave-guard.ts`): unsaved changes and a navigation
// waiting. Save (and then leave), Discard (leave without them, the autosave slot
// cleared) or Stay. Stay is the safe default (it has the focus). Images dropped on the
// body that are still uploading are said too: leaving cancels them.

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

export interface LeaveDialogProps {
  open: boolean;
  /** The item is not created yet: discarding drops it entirely. */
  isNew: boolean;
  /** The autosave slot holds the changes (said, so leaving is less of a worry). */
  autosaved: boolean;
  /** A kept copy (after Take theirs) holds the slot: Discard leaves it there. */
  kept: boolean;
  /** A kept copy could not be stored in the slot: it is only on this page. */
  keptOnlyHere: boolean;
  /** Images still uploading (leaving cancels them). */
  uploading?: number;
  /** There are changes to save (else only uploads hold the screen). */
  changes?: boolean;
  /** Why the last Save or Discard did not leave (the dialog stays open). */
  error: string | null;
  /** Saves; the navigation goes on only when nothing is left unsaved. */
  onSave: () => Promise<void>;
  onDiscard: () => Promise<void>;
  onStay: () => void;
}

export function LeaveDialog({
  open,
  isNew,
  autosaved,
  kept,
  keptOnlyHere,
  uploading = 0,
  changes = true,
  error,
  onSave,
  onDiscard,
  onStay,
}: LeaveDialogProps) {
  const { t } = useTranslation("editor");
  const [busy, setBusy] = useState<"save" | "discard" | null>(null);
  const run = async (which: "save" | "discard") => {
    setBusy(which);
    try {
      await (which === "save" ? onSave() : onDiscard());
    } finally {
      setBusy(null);
    }
  };
  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next && busy === null) onStay();
      }}
    >
      <AlertDialogContent data-slot="leave-dialog" className="sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle>
            {changes ? t("leave.title") : t("leave.uploadingTitle")}
          </AlertDialogTitle>
          {uploading > 0 && changes ? (
            <p data-slot="leave-uploading" className="text-sm font-medium text-foreground">
              {t("leave.uploading", { count: uploading })}
            </p>
          ) : null}
          <AlertDialogDescription>
            {!changes
              ? t("leave.uploading", { count: uploading })
              : isNew
                ? t("leave.descriptionNew")
                : keptOnlyHere
                  ? t("leave.descriptionKeptOnlyHere")
                  : kept
                    ? t("leave.descriptionKept")
                    : autosaved
                      ? t("leave.descriptionAutosaved")
                      : t("leave.description")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {error ? (
          <p role="alert" data-slot="leave-error" className="text-sm text-destructive">
            {error}
          </p>
        ) : null}
        <AlertDialogFooter>
          <Button
            type="button"
            variant="outline"
            autoFocus
            disabled={busy !== null}
            onClick={onStay}
          >
            {t("leave.stay")}
          </Button>
          <Button
            type="button"
            variant="outline"
            className="text-destructive hover:text-destructive"
            disabled={busy !== null}
            onClick={() => {
              void run("discard");
            }}
          >
            {changes ? t("leave.discard") : t("leave.cancelUploads")}
          </Button>
          {changes ? (
            <Button
              type="button"
              disabled={busy !== null}
              onClick={() => {
                void run("save");
              }}
            >
              {busy === "save" ? t("leave.saving") : t("leave.save")}
            </Button>
          ) : null}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
