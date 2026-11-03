// New release (design 15 §11.3): a title (1–200 characters) and an optional description;
// on create the screen opens the release.

import { useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MAX_RELEASE_DESCRIPTION,
  MAX_RELEASE_TITLE,
  type ReleaseDetail,
  createRelease,
  releaseKeys,
} from "@/api/releases";
import { Alert } from "@/auth/auth-layout";
import { FormField } from "@/components/kit/form-field";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { problemText } from "@/structure/taxonomies/terms-model";

export interface NewReleaseDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: (release: ReleaseDetail) => void;
}

export function NewReleaseDialog({ open, onOpenChange, onCreated }: NewReleaseDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="new-release-dialog" className="sm:max-w-lg">
        {open ? <Body close={() => onOpenChange(false)} onCreated={onCreated} /> : null}
      </DialogContent>
    </Dialog>
  );
}

/** A release title's problem, else `undefined`. */
function titleError(
  t: (k: "newDialog.titleRequired" | "newDialog.titleTooLong", o?: { max: number }) => string,
  title: string,
): string | undefined {
  const s = title.trim();
  if (s === "") return t("newDialog.titleRequired");
  if ([...s].length > MAX_RELEASE_TITLE)
    return t("newDialog.titleTooLong", { max: MAX_RELEASE_TITLE });
  return undefined;
}

function Body({ close, onCreated }: { close: () => void; onCreated: (r: ReleaseDetail) => void }) {
  const { t } = useTranslation("releases");
  const queryClient = useQueryClient();
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const error = touched ? titleError(t, title) : undefined;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (titleError(t, title) || busy) return;
    setBusy(true);
    setFailure(null);
    try {
      const created = await createRelease({
        title: title.trim(),
        ...(description.trim() ? { description: description.trim() } : {}),
      });
      void queryClient.invalidateQueries({ queryKey: releaseKeys.lists });
      queryClient.setQueryData(releaseKeys.item(created.id), created);
      close();
      onCreated(created);
    } catch (err) {
      setFailure(problemText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-4" noValidate>
      <DialogHeader>
        <DialogTitle>{t("newDialog.title")}</DialogTitle>
        <DialogDescription>{t("newDialog.description")}</DialogDescription>
      </DialogHeader>
      <FormField label={t("newDialog.name")} error={error}>
        {(control) => (
          <Input
            {...control}
            value={title}
            autoFocus
            maxLength={MAX_RELEASE_TITLE}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={() => setTouched(true)}
          />
        )}
      </FormField>
      <FormField label={t("newDialog.descriptionField")} hint={t("newDialog.descriptionHint")}>
        {(control) => (
          <Textarea
            {...control}
            value={description}
            rows={3}
            maxLength={MAX_RELEASE_DESCRIPTION}
            onChange={(e) => setDescription(e.target.value)}
          />
        )}
      </FormField>
      {failure ? <Alert tone="error">{failure}</Alert> : null}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={close} disabled={busy}>
          {t("newDialog.cancel")}
        </Button>
        <Button type="submit" disabled={busy}>
          {busy ? t("newDialog.creating") : t("newDialog.create")}
        </Button>
      </DialogFooter>
    </form>
  );
}
