// One webhook (design 18 §10.1): its state and actions (Send test, Rotate secret,
// Enable or Disable, Delete), the settings form and the delivery log.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { format } from "date-fns";
import { KeyRoundIcon, PowerIcon, PowerOffIcon, RotateCcwIcon, Trash2Icon } from "lucide-react";
import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type WebhookDto, patchWebhook, webhookQuery } from "@/api/webhooks";
import { Alert } from "@/auth/auth-layout";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { Button } from "@/components/ui/button";
import { problemText } from "@/structure/taxonomies/terms-model";
import { DeliveriesTable } from "./deliveries";
import { DisabledReason, WebhookStateBadge } from "./state-badge";
import { TestDelivery } from "./test-delivery";
import { WebhookFields } from "./webhook-form";
import { DeleteWebhookDialog, RedeliverSinceDialog, RotateSecretDialog } from "./webhook-dialogs";
import {
  type DraftErrors,
  type WebhookDraft,
  checkDraft,
  contentTypesOf,
  hostOf,
} from "./webhooks-model";

export function WebhookDetail({ id }: { id: string }) {
  const { t } = useTranslation("webhooks");
  const hook = useQuery(webhookQuery(id));
  if (hook.isError && !hook.data) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ErrorState error={hook.error} retry={() => void hook.refetch()} />
      </div>
    );
  }
  if (!hook.data) {
    return (
      <div className="mx-auto w-full max-w-6xl text-sm text-muted-foreground">
        {t("deliveries.loadingMore")}
      </div>
    );
  }
  return <Loaded hook={hook.data} />;
}

const draftOf = (w: WebhookDto): WebhookDraft => ({
  name: w.name,
  url: w.url,
  events: [...w.events],
  contentTypes: [...(w.content_types ?? [])],
});

function Loaded({ hook }: { hook: WebhookDto }) {
  const { t } = useTranslation("webhooks");
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [rotating, setRotating] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [since, setSince] = useState(false);
  const [toggling, setToggling] = useState(false);
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: qk.webhooks.all });
  };
  const canSend = hook.state !== "disabled" && hook.state !== "paused";
  const enabled = hook.state === "active" || hook.state === "failing";

  const toggle = async () => {
    setToggling(true);
    try {
      await patchWebhook(hook.id, { enabled: !enabled });
      toast.success(enabled ? t("detail.disabledToast") : t("detail.enabled"));
      refresh();
    } catch (error) {
      reportApiError(error);
      toast.error(problemText(error));
    } finally {
      setToggling(false);
    }
  };

  return (
    <div data-slot="webhook-detail" className="mx-auto flex w-full max-w-6xl flex-col gap-8">
      <div className="flex flex-col gap-4">
        <PageHeader
          title={hook.name}
          description={hostOf(hook.url)}
          actions={
            <>
              <Button
                variant={enabled ? "outline" : "default"}
                disabled={toggling}
                aria-busy={toggling || undefined}
                onClick={() => void toggle()}
              >
                {enabled ? (
                  <PowerOffIcon data-icon="inline-start" />
                ) : (
                  <PowerIcon data-icon="inline-start" />
                )}
                {enabled ? t("detail.disable") : t("detail.enable")}
              </Button>
              <Button variant="outline" onClick={() => setRotating(true)}>
                <KeyRoundIcon data-icon="inline-start" />
                {t("detail.rotate")}
              </Button>
              <Button variant="outline" onClick={() => setDeleting(true)}>
                <Trash2Icon data-icon="inline-start" />
                {t("detail.delete")}
              </Button>
            </>
          }
        />
        <div className="flex flex-wrap items-center gap-3">
          <WebhookStateBadge hook={hook} />
          <DisabledReason hook={hook} />
          {hook.secret_rotating_until ? (
            <span className="text-xs text-muted-foreground">
              {t("detail.rotating", { when: format(new Date(hook.secret_rotating_until), "PPpp") })}
            </span>
          ) : null}
        </div>
        {hook.state === "paused" ? (
          <Alert tone="warning">{t("banner.paused", { count: 1 })}</Alert>
        ) : null}
        {hook.state !== "paused" ? <TestDelivery id={hook.id} /> : null}
      </div>

      <section className="flex flex-col gap-4" aria-labelledby="webhook-settings">
        <h2 id="webhook-settings" className="text-lg text-foreground">
          {t("detail.settings")}
        </h2>
        <SettingsForm key={hook.updated_at} hook={hook} onSaved={refresh} />
      </section>

      <section className="flex flex-col gap-4" aria-labelledby="webhook-deliveries">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
          <div className="flex flex-col gap-1">
            <h2 id="webhook-deliveries" className="text-lg text-foreground">
              {t("detail.deliveries")}
            </h2>
            <p className="text-sm text-muted-foreground">{t("detail.deliveriesHint")}</p>
          </div>
          {canSend &&
          (hook.counts_24h.failed > 0 || hook.counts_24h.skipped > 0 || hook.last_failure_at) ? (
            <Button variant="outline" onClick={() => setSince(true)}>
              <RotateCcwIcon data-icon="inline-start" />
              {t("deliveries.redeliverSince")}
            </Button>
          ) : null}
        </div>
        <DeliveriesTable id={hook.id} canRedeliver={canSend} />
      </section>

      <RotateSecretDialog
        hook={hook}
        open={rotating}
        onOpenChange={setRotating}
        onRotated={refresh}
      />
      <DeleteWebhookDialog
        hook={hook}
        open={deleting}
        onOpenChange={setDeleting}
        onDeleted={() => {
          setDeleting(false);
          toast.success(t("delete.done", { name: hook.name }));
          queryClient.removeQueries({ queryKey: qk.webhooks.item(hook.id) });
          void queryClient.invalidateQueries({ queryKey: qk.webhooks.list });
          void navigate({ to: "/webhooks" });
        }}
      />
      <RedeliverSinceDialog
        hook={hook}
        open={since}
        onOpenChange={setSince}
        onDone={(count) => {
          setSince(false);
          toast.success(t("since.done", { count }));
          refresh();
        }}
      />
    </div>
  );
}

function SettingsForm({ hook, onSaved }: { hook: WebhookDto; onSaved: () => void }) {
  const { t } = useTranslation("webhooks");
  const [draft, setDraft] = useState<WebhookDraft>(() => draftOf(hook));
  const [errors, setErrors] = useState<DraftErrors>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    const own = checkDraft(t, draft);
    setErrors(own);
    if (Object.keys(own).length > 0) return;
    setPending(true);
    setFailure(null);
    try {
      await patchWebhook(hook.id, {
        name: draft.name.trim(),
        url: draft.url.trim(),
        events: draft.events,
        content_types: contentTypesOf(draft),
      });
      toast.success(t("form.saved"));
      onSaved();
    } catch (error) {
      if (error instanceof ApiError && error.status === 422) {
        const fields = error.fieldErrors();
        const known: DraftErrors = {};
        for (const k of ["name", "url", "events", "content_types"] as const) {
          if (fields[k]) known[k] = fields[k];
        }
        setErrors(known);
        if (Object.keys(known).length === 0) setFailure(error.problem.detail ?? t("form.failed"));
      } else {
        reportApiError(error);
        setFailure(problemText(error));
      }
    } finally {
      setPending(false);
    }
  };

  return (
    <form
      noValidate
      data-slot="webhook-settings"
      onSubmit={(e) => void submit(e)}
      className="flex max-w-2xl min-w-0 flex-col gap-5"
    >
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
      <Button
        type="submit"
        className="self-start"
        disabled={pending}
        aria-busy={pending || undefined}
      >
        {pending ? t("form.saving") : t("form.save")}
      </Button>
    </form>
  );
}
