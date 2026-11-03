// New webhook (design 18 §10.1): the fields, an optional own secret behind Advanced,
// then the secret shown once with `CopyOnce` and a Send test button. The created secret
// is the dialog body's state only (never the query cache): it goes when the dialog
// closes, and while it is shown Escape and a click outside do not close the dialog.

import { useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type CreatedWebhookDto, createWebhook } from "@/api/webhooks";
import { Alert } from "@/auth/auth-layout";
import { CopyOnce } from "@/components/kit/copy-once";
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
import { problemText } from "@/structure/taxonomies/terms-model";
import { TestDelivery } from "./test-delivery";
import { WebhookFields } from "./webhook-form";
import { type DraftErrors, type WebhookDraft, checkDraft, contentTypesOf } from "./webhooks-model";

export interface NewWebhookDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function NewWebhookDialog({ open, onOpenChange }: NewWebhookDialogProps) {
  const [holding, setHolding] = useState(false);
  const change = (next: boolean) => {
    if (!next) setHolding(false);
    onOpenChange(next);
  };
  const keep = (e: Event) => {
    if (holding) e.preventDefault();
  };
  return (
    <Dialog open={open} onOpenChange={change}>
      <DialogContent
        data-slot="new-webhook-dialog"
        className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-2xl"
        onEscapeKeyDown={keep}
        onInteractOutside={keep}
      >
        {/* Mounted only while open: the created secret goes with it. */}
        {open ? <Body onCreated={() => setHolding(true)} close={() => change(false)} /> : null}
      </DialogContent>
    </Dialog>
  );
}

const EMPTY: WebhookDraft = { name: "", url: "", events: [], contentTypes: [] };

function Body({ onCreated, close }: { onCreated: () => void; close: () => void }) {
  const { t } = useTranslation("webhooks");
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState<WebhookDraft>(EMPTY);
  const [secret, setSecret] = useState("");
  const [advanced, setAdvanced] = useState(false);
  const [errors, setErrors] = useState<DraftErrors>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [created, setCreated] = useState<CreatedWebhookDto | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    const own = checkDraft(t, draft);
    setErrors(own);
    if (Object.keys(own).length > 0) return;
    setPending(true);
    setFailure(null);
    try {
      const done = await createWebhook({
        name: draft.name.trim(),
        url: draft.url.trim(),
        events: draft.events,
        content_types: contentTypesOf(draft),
        ...(secret.trim() ? { secret: secret.trim() } : {}),
      });
      setCreated(done);
      setSecret("");
      onCreated();
      void queryClient.invalidateQueries({ queryKey: qk.webhooks.list });
    } catch (error) {
      if (error instanceof ApiError && error.status === 422) {
        const fields = error.fieldErrors();
        const known: DraftErrors = {};
        for (const k of ["name", "url", "events", "content_types", "secret"] as const) {
          if (fields[k]) known[k] = fields[k];
        }
        if (known.secret) setAdvanced(true);
        setErrors(known);
        if (Object.keys(known).length === 0) {
          setFailure(error.problem.detail ?? t("form.failed"));
        }
      } else {
        reportApiError(error);
        setFailure(problemText(error));
      }
    } finally {
      setPending(false);
    }
  };

  if (created) {
    return (
      <div className="flex min-w-0 flex-col gap-5">
        <DialogHeader>
          <DialogTitle>{t("created.title")}</DialogTitle>
          <DialogDescription>{t("created.description")}</DialogDescription>
        </DialogHeader>
        <CopyOnce value={created.secret} label={t("created.secret")} />
        <TestDelivery id={created.webhook.id} />
        <DialogFooter>
          <Button type="button" onClick={close} autoFocus>
            {t("created.done")}
          </Button>
        </DialogFooter>
      </div>
    );
  }

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex min-w-0 flex-col gap-5">
      <DialogHeader>
        <DialogTitle>{t("new")}</DialogTitle>
        <DialogDescription>{t("description")}</DialogDescription>
      </DialogHeader>
      {failure ? <Alert tone="error">{failure}</Alert> : null}
      <WebhookFields
        draft={draft}
        errors={errors}
        disabled={pending}
        onChange={(next, field) => {
          setDraft(next);
          setErrors((prev) => {
            const copy = { ...prev };
            delete copy[field];
            return copy;
          });
        }}
      />
      <div className="flex flex-col gap-3">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="self-start"
          aria-expanded={advanced}
          onClick={() => setAdvanced((v) => !v)}
        >
          {t("form.advanced")}
        </Button>
        {advanced ? (
          <FormField label={t("form.secret")} hint={t("form.secretHint")} error={errors.secret}>
            {(control) => (
              <Input
                {...control}
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={secret}
                disabled={pending}
                className="font-mono text-[0.8125rem]"
                onChange={(e) => {
                  setSecret(e.currentTarget.value);
                  setErrors((prev) => {
                    const copy = { ...prev };
                    delete copy.secret;
                    return copy;
                  });
                }}
              />
            )}
          </FormField>
        ) : null}
      </div>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={close}>
          {t("form.cancel")}
        </Button>
        <Button type="submit" disabled={pending} aria-busy={pending || undefined}>
          {pending ? t("form.creating") : t("form.create")}
        </Button>
      </DialogFooter>
    </form>
  );
}
