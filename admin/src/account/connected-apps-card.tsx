// Connected apps (12 §3.2, §8): the AI assistants a user connected through MCP (OAuth
// grants, `GET /oauth/grants`), each with its client, a permissions summary, when it was
// connected and last used, and Disconnect (after a confirm; the grant and its access
// tokens go). Account shows the signed-in user's; the user sheet shows another user's
// to an administrator (`?user=`). Client names are the client's own text.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { cn } from "cn";
import { Link } from "@tanstack/react-router";
import { ExternalLinkIcon, PlugZapIcon, SparklesIcon } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { metaQuery } from "@/api/meta";
import { type GrantDto, disconnectGrant, grantsQuery, mcpUrl } from "@/api/oauth";
import { qk } from "@/api/keys";
import { settingsQuery } from "@/api/settings";
import type { MeDto } from "@/auth/session";
import { CopyField } from "@/components/kit/copy-field";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { DESTRUCTIVE } from "@/lib/destructive";
import { ClientKind, ClientLogo } from "@/oauth/client-mark";
import { AI_GUIDE_URL, permissionSummary } from "@/oauth/model";
import { writeFailure } from "./account-model";

/** Account → Connected apps: the signed-in user's own. */
export function ConnectedAppsCard({ me }: { me: MeDto }) {
  const { t } = useTranslation("ai");
  const settings = useQuery({ ...settingsQuery, enabled: me.capabilities.includes("read_admin") });
  const paused = settings.data?.["mcp.enabled"] === false;
  return (
    <Card className="gap-0 py-0">
      <ConnectedApps
        userId={me.id}
        own
        empty={<OwnEmpty canSettings={me.capabilities.includes("manage_settings")} />}
        rowClassName="px-4"
      />
      {paused ? (
        <p className="border-t px-4 py-3 text-sm text-muted-foreground">{t("apps.paused")}</p>
      ) : null}
    </Card>
  );
}

/** Nothing connected yet: the URL to paste and the way to the setup steps. */
function OwnEmpty({ canSettings }: { canSettings: boolean }) {
  const { t } = useTranslation("ai");
  const site = useQuery(metaQuery);
  return (
    <div className="flex flex-col gap-5 p-4">
      <EmptyState
        icon={SparklesIcon}
        title={t("apps.emptyTitle")}
        body={t("apps.emptyBody")}
        className="border-0 px-2 py-6"
        action={
          <>
            {canSettings ? (
              <Button asChild variant="outline">
                <Link to="/settings/$group" params={{ group: "ai-assistants" }}>
                  <PlugZapIcon data-icon="inline-start" />
                  {t("apps.connectAction")}
                </Link>
              </Button>
            ) : null}
            <Button asChild variant="outline">
              <a href={AI_GUIDE_URL} target="_blank" rel="noopener noreferrer">
                <ExternalLinkIcon data-icon="inline-start" />
                {t("apps.guide")}
              </a>
            </Button>
          </>
        }
      />
      {site.data ? (
        <CopyField
          value={mcpUrl(site.data.site_url)}
          label={t("settings.connect.url")}
          className="mx-auto w-full max-w-md"
        />
      ) : null}
    </div>
  );
}

export interface ConnectedAppsProps {
  userId: string;
  /** The signed-in user's own apps (no `?user=`). */
  own: boolean;
  /** Who the apps act as, for the confirm text when they are another user's. */
  userName?: string;
  /** What shows when there are none. */
  empty: ReactNode;
  rowClassName?: string;
}

/** The list, its loading and error states, and the Disconnect confirm. */
export function ConnectedApps({ userId, own, userName, empty, rowClassName }: ConnectedAppsProps) {
  const { t } = useTranslation("ai");
  const queryClient = useQueryClient();
  const list = useQuery(grantsQuery(userId, own));
  const [disconnecting, setDisconnecting] = useState<GrantDto | null>(null);
  const grants = list.data ?? [];

  let content: ReactNode;
  if (list.isError && !list.data) {
    content = <ErrorState error={list.error} retry={() => void list.refetch()} className="m-4" />;
  } else if (list.isPending) {
    content = (
      <div aria-busy="true" className="flex flex-col gap-3 p-4">
        <Skeleton className="h-14 w-full" />
        <Skeleton className="h-14 w-full" />
      </div>
    );
  } else if (grants.length === 0) {
    content = empty;
  } else {
    content = (
      <ul
        aria-label={t("apps.label")}
        data-slot="connected-apps"
        className="flex flex-col divide-y"
      >
        {grants.map((grant) => (
          <GrantRow
            key={grant.id}
            grant={grant}
            className={rowClassName}
            onDisconnect={() => setDisconnecting(grant)}
          />
        ))}
      </ul>
    );
  }

  return (
    <>
      {content}
      <DisconnectDialog
        grant={disconnecting}
        otherUser={own ? undefined : userName}
        onClose={() => setDisconnecting(null)}
        onDone={() => void queryClient.invalidateQueries({ queryKey: qk.grants(userId) })}
      />
    </>
  );
}

function GrantRow({
  grant,
  className,
  onDisconnect,
}: {
  grant: GrantDto;
  className?: string;
  onDisconnect: () => void;
}) {
  const { t } = useTranslation("ai");
  const name = grant.client.name;
  const summary = permissionSummary(grant.scopes, t);
  return (
    <li
      data-slot="connected-app"
      className={cn("flex items-start gap-3 py-3 sm:items-center", className)}
    >
      <ClientLogo client={grant.client} />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          <span className="truncate text-sm font-medium text-foreground" title={name}>
            {name}
          </span>
          <ClientKind verified={grant.client.kind === "metadata"} />
        </span>
        <span className="text-sm text-muted-foreground" title={grant.scopes.join(" ")}>
          {summary}
        </span>
        <span className="flex flex-wrap gap-x-1 text-xs text-muted-foreground">
          <span className="whitespace-nowrap">
            {t("apps.connected")} <RelativeTime at={grant.created_at} />
          </span>
          <span aria-hidden="true">·</span>
          <span className="whitespace-nowrap">
            {t("apps.lastUsed")} <RelativeTime at={grant.last_used_at} />
          </span>
        </span>
      </div>
      <Button
        variant="outline"
        aria-label={t("apps.disconnectLabel", { name })}
        onClick={onDisconnect}
        className="shrink-0 hover:text-destructive"
      >
        {t("apps.disconnect")}
      </Button>
    </li>
  );
}

function DisconnectDialog({
  grant,
  otherUser,
  onClose,
  onDone,
}: {
  grant: GrantDto | null;
  otherUser: string | undefined;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation("ai");
  const [pending, setPending] = useState(false);
  // The last grant asked about, so the text stays while the dialog closes.
  const [shown, setShown] = useState(grant);
  if (grant && grant !== shown) setShown(grant);

  const disconnect = async () => {
    if (!grant || pending) return;
    setPending(true);
    try {
      await disconnectGrant(grant.id);
      onDone();
      toast.success(t("apps.disconnected"));
      onClose();
    } catch (error) {
      writeFailure(error);
      toast.error(t("apps.disconnectFailed"), {
        description: error instanceof Error ? error.message : undefined,
      });
    } finally {
      setPending(false);
    }
  };

  return (
    <AlertDialog open={grant !== null} onOpenChange={(open) => !open && !pending && onClose()}>
      <AlertDialogContent data-slot="disconnect-app">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">
            {t("apps.disconnectTitle", { name: shown?.client.name ?? "" })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {otherUser ? t("apps.disconnectOther", { user: otherUser }) : t("apps.disconnectBody")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <Button variant="outline" onClick={onClose} disabled={pending}>
            {t("apps.cancel")}
          </Button>
          <Button
            className={DESTRUCTIVE}
            onClick={() => void disconnect()}
            disabled={pending}
            aria-busy={pending}
          >
            {pending ? t("apps.disconnecting") : t("apps.disconnect")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
