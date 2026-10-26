// The `ask` of `staleGuard`: "Changed by someone else" with Reload / Overwrite / Cancel.

import { useTranslation } from "react-i18next";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import type { StaleChoice } from "@/lib/stale-guard";

export interface StaleDialogProps {
  open: boolean;
  /** The resource's name, e.g. the menu or redirect it is about. */
  resource: string;
  onChoose: (choice: StaleChoice) => void;
}

/** Radix alert dialogs ignore outside clicks; Escape and the Cancel button cancel. */
export function StaleDialog({ open, resource, onChoose }: StaleDialogProps) {
  const { t } = useTranslation("kit");
  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onChoose("cancel");
      }}
    >
      <AlertDialogContent data-slot="stale-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("stale.title")}</AlertDialogTitle>
          <AlertDialogDescription>{t("stale.body", { resource })}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("stale.cancel")}</AlertDialogCancel>
          <Button
            variant="outline"
            onClick={() => {
              onChoose("overwrite");
            }}
          >
            {t("stale.overwrite")}
          </Button>
          <Button
            onClick={() => {
              onChoose("reload");
            }}
          >
            {t("stale.reload")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
