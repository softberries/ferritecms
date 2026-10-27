// The user's live sessions (`GET /auth/sessions`, newest first): when each signed in and
// was last active (updated at most every 5 minutes), "This session" for the current one,
// Sign out per other row (`DELETE /auth/sessions/{id}`) and "Sign out all other
// sessions" (`POST /auth/sessions/revoke-others`). This session ends with Sign out in
// the user menu instead.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { format } from "date-fns";
import { cn } from "cn";
import { LogOutIcon, MonitorIcon, MonitorSmartphoneIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { type SessionDto, revokeOtherSessions, revokeSession, sessionsQuery } from "@/api/sessions";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { writeFailure } from "./account-model";

export function SessionsCard() {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const list = useQuery(sessionsQuery);
  const [busy, setBusy] = useState<string | null>(null);
  const sessions = list.data ?? [];
  const others = sessions.filter((s) => !s.current).length;
  const refresh = () => queryClient.invalidateQueries({ queryKey: qk.sessions });

  const run = async (key: string, action: () => Promise<string>) => {
    if (busy) return;
    setBusy(key);
    try {
      toast.success(await action());
    } catch (error) {
      writeFailure(error);
      toast.error(t("account.sessions.failed"), {
        description: error instanceof Error ? error.message : undefined,
      });
    } finally {
      await refresh();
      setBusy(null);
    }
  };

  return (
    <Card className="gap-0 py-0">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-3">
        <span className="text-sm text-muted-foreground tabular-nums">
          {list.data
            ? others === 0
              ? t("account.sessions.onlyThis")
              : t("account.sessions.count", { count: sessions.length })
            : "\u00a0"}
        </span>
        <Button
          variant="outline"
          disabled={others === 0 || busy !== null}
          aria-busy={busy === "others"}
          onClick={() =>
            void run("others", async () => {
              const { revoked } = await revokeOtherSessions();
              return t("account.sessions.signedOutOthers", { count: revoked });
            })
          }
        >
          <LogOutIcon data-icon="inline-start" />
          {t("account.sessions.signOutOthers")}
        </Button>
      </div>
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} className="py-8" />
      ) : list.isPending ? (
        <div aria-busy="true" className="flex flex-col gap-3 p-4">
          {[0, 1].map((i) => (
            <Skeleton key={i} className="h-10 w-full" />
          ))}
        </div>
      ) : (
        <ul aria-label={t("account.sessions.label")} className="divide-y">
          {sessions.map((s) => (
            <SessionRow
              key={s.id}
              session={s}
              busy={busy === s.id}
              disabled={busy !== null}
              onSignOut={() =>
                void run(s.id, async () => {
                  await revokeSession(s.id);
                  return t("account.sessions.signedOut");
                })
              }
            />
          ))}
        </ul>
      )}
    </Card>
  );
}

function SessionRow({
  session,
  busy,
  disabled,
  onSignOut,
}: {
  session: SessionDto;
  busy: boolean;
  disabled: boolean;
  onSignOut: () => void;
}) {
  const { t } = useTranslation("admin");
  const Icon = session.current ? MonitorSmartphoneIcon : MonitorIcon;
  const signedIn = format(new Date(session.created_at), "PPp");
  return (
    <li data-slot="session-row" className="flex items-center gap-3 px-4 py-3">
      <span
        aria-hidden="true"
        className={cn(
          "flex size-9 shrink-0 items-center justify-center rounded-lg border",
          session.current ? "border-primary/30 bg-primary/8 text-primary" : "text-muted-foreground",
        )}
      >
        <Icon className="size-4" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex items-center gap-2 text-sm font-medium text-foreground">
          {session.current ? t("account.sessions.current") : t("account.sessions.other")}
        </span>
        <span className="text-xs text-muted-foreground">
          <span title={signedIn}>{t("account.sessions.signedIn", { when: signedIn })}</span>
          {" · "}
          <span>
            {t("account.sessions.lastSeenPrefix")} <RelativeTime at={session.last_seen_at} />
          </span>
        </span>
      </div>
      {session.current ? null : (
        <Button
          variant="ghost"
          disabled={disabled}
          aria-busy={busy}
          aria-label={t("account.sessions.signOutLabel", { when: signedIn })}
          onClick={onSignOut}
        >
          {t("account.sessions.signOut")}
        </Button>
      )}
    </li>
  );
}
