// Delete a plugin (07 §1 H-P3, Review Focus 4): two separate choices. "Delete, keep data
// for 30 days" keeps its stored data, settings and sealed secrets, which a plugin of the
// same name installed within that time picks up again (whoever publishes it; publishers
// are not verified); "Delete and erase data" erases them at once. Kept data is listed
// under Retained data, where it can still be erased early (P47).

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import { deletePlugin } from "@/api/plugins";
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
import { DESTRUCTIVE } from "@/lib/destructive";
import { problemText } from "@/structure/taxonomies/terms-model";

export interface DeletePluginDialogProps {
  /** The plugin asked about; `null` closes the dialog. */
  name: string | null;
  onClose: () => void;
  onDeleted: (name: string, erased: boolean) => void;
}

export function DeletePluginDialog({ name, onClose, onDeleted }: DeletePluginDialogProps) {
  const { t } = useTranslation("admin");
  // The last name asked about, so the closing animation keeps its text.
  const [shown, setShown] = useState(name);
  if (name && name !== shown) setShown(name);
  const [opened, setOpened] = useState(name);
  const [pending, setPending] = useState<"keep" | "erase" | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  if (name !== opened) {
    setOpened(name);
    setFailure(null);
  }

  const run = async (purge: boolean) => {
    if (!name || pending) return;
    setPending(purge ? "erase" : "keep");
    setFailure(null);
    try {
      await deletePlugin(name, { purge });
      onDeleted(name, purge);
    } catch (error) {
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(null);
    }
  };

  const shownName = shown ?? "";
  return (
    <AlertDialog open={name !== null} onOpenChange={(open) => !open && !pending && onClose()}>
      <AlertDialogContent data-slot="delete-plugin" className="sm:max-w-lg">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">
            {t("plugins.delete.title", { name: shownName })}
          </AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex flex-col gap-2">
              <p>{t("plugins.delete.body")}</p>
              <p>{t("plugins.delete.keepBody", { name: shownName })}</p>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        {failure ? <Alert tone="error">{failure}</Alert> : null}
        <AlertDialogFooter className="flex-col gap-2 sm:flex-row sm:flex-wrap sm:justify-end">
          <Button variant="outline" onClick={onClose} disabled={pending !== null}>
            {t("plugins.delete.cancel")}
          </Button>
          <Button
            variant="outline"
            onClick={() => void run(false)}
            disabled={pending !== null}
            aria-busy={pending === "keep" || undefined}
          >
            {pending === "keep" ? t("plugins.delete.deleting") : t("plugins.delete.keep")}
          </Button>
          <Button
            variant="destructive"
            className={DESTRUCTIVE}
            onClick={() => void run(true)}
            disabled={pending !== null}
            aria-busy={pending === "erase" || undefined}
          >
            {pending === "erase" ? t("plugins.delete.deleting") : t("plugins.delete.erase")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
