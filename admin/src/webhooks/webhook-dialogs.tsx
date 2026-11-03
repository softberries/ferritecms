// The detail's confirmations (design 18 §10.1): Rotate secret (the new one shown once,
// with the old one's end), Delete (names the endpoint) and Redeliver failed since….

import { format } from "date-fns";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import {
  type RotatedSecretDto,
  type WebhookDto,
  deleteWebhook,
  redeliverSince,
  rotateSecret,
} from "@/api/webhooks";
import { Alert } from "@/auth/auth-layout";
import { CopyOnce } from "@/components/kit/copy-once";
import { FormField } from "@/components/kit/form-field";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
import { DESTRUCTIVE } from "@/lib/destructive";
import { problemText } from "@/structure/taxonomies/terms-model";
import { localToRfc3339 } from "./webhooks-model";

const when = (at: string) => format(new Date(at), "PPpp");

export function RotateSecretDialog({
  hook,
  open,
  onOpenChange,
  onRotated,
}: {
  hook: WebhookDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRotated: () => void;
}) {
  const { t } = useTranslation("webhooks");
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  // The new secret: this dialog's state only, gone when it closes.
  const [rotated, setRotated] = useState<RotatedSecretDto | null>(null);
  const change = (next: boolean) => {
    if (pending) return;
    if (!next) {
      setRotated(null);
      setFailure(null);
    }
    onOpenChange(next);
  };
  const run = async () => {
    setPending(true);
    setFailure(null);
    try {
      setRotated(await rotateSecret(hook.id));
      onRotated();
    } catch (error) {
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };
  const keep = (e: Event) => {
    if (rotated) e.preventDefault();
  };
  return (
    <Dialog open={open} onOpenChange={change}>
      <DialogContent
        data-slot="rotate-secret-dialog"
        className="sm:max-w-xl"
        onEscapeKeyDown={keep}
        onInteractOutside={keep}
      >
        {rotated ? (
          <div className="flex min-w-0 flex-col gap-5">
            <DialogHeader>
              <DialogTitle>{t("rotate.doneTitle")}</DialogTitle>
              <DialogDescription>
                {t("rotate.doneDescription", { when: when(rotated.previous_valid_until) })}
              </DialogDescription>
            </DialogHeader>
            <CopyOnce value={rotated.secret} label={t("created.secret")} />
            <DialogFooter>
              <Button type="button" onClick={() => change(false)} autoFocus>
                {t("rotate.done")}
              </Button>
            </DialogFooter>
          </div>
        ) : (
          <div className="flex min-w-0 flex-col gap-5">
            <DialogHeader>
              <DialogTitle className="break-words">
                {t("rotate.title", { name: hook.name })}
              </DialogTitle>
              <DialogDescription>{t("rotate.body")}</DialogDescription>
            </DialogHeader>
            {failure ? <Alert tone="error">{failure}</Alert> : null}
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => change(false)}>
                {t("rotate.cancel")}
              </Button>
              <Button
                type="button"
                onClick={() => void run()}
                disabled={pending}
                aria-busy={pending || undefined}
              >
                {pending ? t("rotate.rotating") : t("rotate.confirm")}
              </Button>
            </DialogFooter>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

export function DeleteWebhookDialog({
  hook,
  open,
  onOpenChange,
  onDeleted,
}: {
  hook: WebhookDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDeleted: () => void;
}) {
  const { t } = useTranslation("webhooks");
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const run = async () => {
    setPending(true);
    setFailure(null);
    try {
      await deleteWebhook(hook.id);
      onDeleted();
    } catch (error) {
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <AlertDialog open={open} onOpenChange={(next) => !pending && onOpenChange(next)}>
      <AlertDialogContent data-slot="delete-webhook" className="sm:max-w-lg">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">
            {t("delete.title", { name: hook.name })}
          </AlertDialogTitle>
          <AlertDialogDescription>{t("delete.body")}</AlertDialogDescription>
        </AlertDialogHeader>
        {failure ? <Alert tone="error">{failure}</Alert> : null}
        <AlertDialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={pending}>
            {t("delete.cancel")}
          </Button>
          <Button
            variant="destructive"
            className={DESTRUCTIVE}
            onClick={() => void run()}
            disabled={pending}
            aria-busy={pending || undefined}
          >
            {pending ? t("delete.deleting") : t("delete.confirm")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export function RedeliverSinceDialog({
  hook,
  open,
  onOpenChange,
  onDone,
}: {
  hook: WebhookDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDone: (count: number) => void;
}) {
  const { t } = useTranslation("webhooks");
  const [value, setValue] = useState(() =>
    format(new Date(Date.now() - 24 * 3600_000), "yyyy-MM-dd'T'HH:mm"),
  );
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const run = async () => {
    const since = localToRfc3339(value);
    if (!since) return;
    setPending(true);
    setFailure(null);
    try {
      onDone(await redeliverSince(hook.id, since));
    } catch (error) {
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={(next) => !pending && onOpenChange(next)}>
      <DialogContent data-slot="redeliver-since-dialog" className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("since.title")}</DialogTitle>
          <DialogDescription>{t("since.body")}</DialogDescription>
        </DialogHeader>
        {failure ? <Alert tone="error">{failure}</Alert> : null}
        <FormField label={t("since.label")}>
          {(control) => (
            <Input
              {...control}
              type="datetime-local"
              value={value}
              disabled={pending}
              onChange={(e) => setValue(e.currentTarget.value)}
            />
          )}
        </FormField>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            {t("since.cancel")}
          </Button>
          <Button
            type="button"
            onClick={() => void run()}
            disabled={pending || !localToRfc3339(value)}
            aria-busy={pending || undefined}
          >
            {pending ? t("since.running") : t("since.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
