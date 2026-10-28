// Mail, read-only (P49): whether `[mail]` in `ferrite.toml` is set and its From address
// (`GET /system/status` `mail`; without `manage_system`, `MeDto.mail_configured` alone).
// "Send test e-mail" (`manage_system`) queues one mail to the signed-in user's own
// address (`POST /system/mail/test`, 3 per 10 minutes). Without mail, a note on how to
// set it up. The SMTP URL is never shown.

import { useQuery } from "@tanstack/react-query";
import { CircleCheckIcon, CircleDashedIcon, SendIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { INLINE_ERRORS, reportApiError } from "@/api/client";
import { ApiError } from "@/api/problem";
import { sendTestMail, systemStatusQuery } from "@/api/system";
import type { MeDto } from "@/auth/session";
import { ErrorState } from "@/components/kit/error-state";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { GroupFrame } from "./group-frame";

export function MailGroup({ me }: { me: MeDto }) {
  const { t } = useTranslation("admin");
  const canSystem = me.capabilities.includes("manage_system");
  const status = useQuery({ ...systemStatusQuery, enabled: canSystem, meta: INLINE_ERRORS });
  return (
    <GroupFrame
      title={t("settings.groups.mail.title")}
      description={t("settings.groups.mail.description")}
    >
      {!canSystem ? (
        <MailCard
          configured={me.mail_configured === true}
          from={null}
          canTest={false}
          email={me.email}
        />
      ) : status.isError ? (
        <ErrorState error={status.error} retry={() => void status.refetch()} />
      ) : status.data ? (
        <MailCard
          configured={status.data.mail.configured}
          from={status.data.mail.from ?? null}
          canTest
          email={me.email}
        />
      ) : (
        <Card aria-busy="true" className="gap-3 p-4">
          <Skeleton className="h-5 w-40" />
          <Skeleton className="h-5 w-64" />
        </Card>
      )}
    </GroupFrame>
  );
}

function MailCard({
  configured,
  from,
  canTest,
  email,
}: {
  configured: boolean;
  from: string | null;
  canTest: boolean;
  email: string;
}) {
  const { t } = useTranslation("admin");
  const [sending, setSending] = useState(false);
  const send = async () => {
    setSending(true);
    try {
      const sent = await sendTestMail();
      toast.success(t("settings.mail.sent", { to: sent.to }));
    } catch (e) {
      reportApiError(e);
      const detail = e instanceof ApiError ? e.problem.detail || e.problem.title : undefined;
      toast.error(t("settings.mail.notSent"), detail ? { description: detail } : {});
    } finally {
      setSending(false);
    }
  };
  const Icon = configured ? CircleCheckIcon : CircleDashedIcon;
  return (
    <Card data-slot="mail-panel" data-configured={configured}>
      <CardContent className="flex flex-col gap-4">
        <div className="flex items-start gap-3">
          <Icon
            aria-hidden="true"
            className={
              configured
                ? "mt-0.5 size-5 shrink-0 text-success"
                : "mt-0.5 size-5 shrink-0 text-muted-foreground"
            }
          />
          <div className="flex min-w-0 flex-col gap-0.5">
            <p className="text-sm font-medium text-foreground">
              {configured ? t("settings.mail.configured") : t("settings.mail.notConfigured")}
            </p>
            <p className="text-sm text-muted-foreground">
              {configured
                ? t("settings.mail.configuredHint")
                : t("settings.mail.notConfiguredHint")}
            </p>
          </div>
        </div>
        {configured && from ? (
          <dl className="grid gap-0.5 rounded-lg bg-muted/60 px-4 py-3">
            <dt className="text-xs text-muted-foreground">{t("settings.mail.from")}</dt>
            <dd className="min-w-0 font-mono text-sm text-foreground wrap-anywhere">{from}</dd>
          </dl>
        ) : null}
        {!configured ? (
          <div data-slot="mail-setup" className="flex flex-col gap-2">
            <p className="text-sm text-foreground">{t("settings.mail.setup")}</p>
            <pre className="rounded-lg bg-muted/60 px-4 py-3 font-mono text-xs whitespace-pre-wrap text-foreground wrap-anywhere">
              {
                '[mail]\nsmtp_url = "smtps://user:pass@mail.example.com"\nfrom = "Site <noreply@example.com>"'
              }
            </pre>
            <p className="text-sm text-muted-foreground">{t("settings.mail.setupRestart")}</p>
          </div>
        ) : null}
        {configured && !canTest ? (
          <p className="text-sm text-muted-foreground">{t("settings.mail.testNeedsSystem")}</p>
        ) : null}
      </CardContent>
      {configured && canTest ? (
        <CardFooter className="flex-wrap justify-end gap-3 border-t py-3">
          <span className="mr-auto min-w-0 text-sm text-muted-foreground wrap-anywhere">
            {t("settings.mail.testHint", { email })}
          </span>
          <Button onClick={() => void send()} disabled={sending} aria-busy={sending}>
            <SendIcon data-icon="inline-start" />
            {sending ? t("settings.mail.sending") : t("settings.mail.test")}
          </Button>
        </CardFooter>
      ) : null}
    </Card>
  );
}
