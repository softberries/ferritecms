// The user's own API tokens (`GET /users/{me}/tokens`; the list never holds a value):
// name, scopes, created, last used and expiry, with Revoke (after a confirm) and New
// token (`TokenDialog`). At most 50 per user: past that the server's `422` shows in the
// dialog. Token names are the user's own text and render as text.

import { ActivityLink } from "./connected-apps-card";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { format } from "date-fns";
import { KeyRoundIcon, PlusIcon, Trash2Icon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { MAX_TOKENS, type TokenDto, revokeToken, tokensQuery } from "@/api/tokens";
import type { MeDto } from "@/auth/session";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { DESTRUCTIVE } from "@/lib/destructive";
import { ScopeChips, TokenDialog } from "./token-dialog";
import { writeFailure } from "./account-model";

export function TokensCard({ me }: { me: MeDto }) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const list = useQuery(tokensQuery(me.id));
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<TokenDto | null>(null);
  const refresh = () => void queryClient.invalidateQueries({ queryKey: qk.tokens(me.id) });
  const tokens = list.data ?? [];
  // At the server's limit New token is off (a `422` would refuse it anyway).
  const full = tokens.length >= MAX_TOKENS;

  return (
    <Card className="gap-0 py-0">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-3">
        <span className="text-sm text-muted-foreground tabular-nums">
          {list.data ? t("account.tokens.count", { count: tokens.length, max: MAX_TOKENS }) : " "}
        </span>
        <Button
          variant="outline"
          disabled={full}
          aria-describedby={full ? "tokens-full" : undefined}
          onClick={() => setCreating(true)}
        >
          <PlusIcon data-icon="inline-start" />
          {t("account.tokens.new")}
        </Button>
        {full ? (
          <p id="tokens-full" className="w-full text-sm text-muted-foreground">
            {t("account.tokens.full", { max: MAX_TOKENS })}
          </p>
        ) : null}
      </div>
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} className="py-8" />
      ) : list.isPending ? (
        <div aria-busy="true" className="flex flex-col gap-3 p-4">
          {[0, 1].map((i) => (
            <Skeleton key={i} className="h-10 w-full" />
          ))}
        </div>
      ) : tokens.length === 0 ? (
        <EmptyState
          icon={KeyRoundIcon}
          title={t("account.tokens.emptyTitle")}
          body={t("account.tokens.emptyBody")}
          className="py-10"
        />
      ) : (
        <Table aria-label={t("account.tokens.label")}>
          <TableHeader>
            <TableRow>
              <TableHead className="pl-4">{t("account.tokens.columns.name")}</TableHead>
              <TableHead className="hidden md:table-cell">
                {t("account.tokens.columns.scopes")}
              </TableHead>
              <TableHead className="hidden lg:table-cell">
                {t("account.tokens.columns.created")}
              </TableHead>
              <TableHead className="hidden sm:table-cell">
                {t("account.tokens.columns.lastUsed")}
              </TableHead>
              <TableHead className="hidden sm:table-cell">
                {t("account.tokens.columns.expires")}
              </TableHead>
              <TableHead className="pr-4">
                <span className="sr-only">{t("account.tokens.revoke")}</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {tokens.map((token) => (
              <TokenRow key={token.id} token={token} onRevoke={() => setRevoking(token)} />
            ))}
          </TableBody>
        </Table>
      )}
      <TokenDialog
        open={creating}
        onOpenChange={setCreating}
        userId={me.id}
        held={me.scopes}
        onCreated={refresh}
      />
      <RevokeDialog
        token={revoking}
        userId={me.id}
        onClose={() => setRevoking(null)}
        onRevoked={refresh}
      />
    </Card>
  );
}

function expired(token: TokenDto): boolean {
  return token.expires_at != null && new Date(token.expires_at).getTime() <= Date.now();
}

function TokenRow({ token, onRevoke }: { token: TokenDto; onRevoke: () => void }) {
  const { t } = useTranslation("admin");
  const gone = expired(token);
  return (
    <TableRow data-slot="token-row" className="align-top">
      <TableCell className="max-w-0 py-3 pl-4 whitespace-normal md:w-[30%]">
        <div className="flex min-w-0 flex-col gap-1.5">
          <span className="flex min-w-0 items-center gap-2">
            <span className="truncate font-medium text-foreground" title={token.name}>
              {token.name}
            </span>
            {gone ? <Badge variant="destructive">{t("account.tokens.expired")}</Badge> : null}
          </span>
          {/* Below `md` the scopes column is hidden: the chips go under the name. */}
          <ScopeChips scopes={token.scopes} className="md:hidden" />
          {/* Below `sm` the date columns are hidden too. */}
          <span className="flex flex-wrap gap-x-1 text-xs text-muted-foreground sm:hidden">
            <span className="whitespace-nowrap">
              {token.last_used_at ? (
                <RelativeTime at={token.last_used_at} />
              ) : (
                t("account.tokens.neverUsed")
              )}
            </span>
            <span aria-hidden="true">·</span>
            <span className="whitespace-nowrap">
              <Expiry token={token} labelled />
            </span>
          </span>
        </div>
      </TableCell>
      <TableCell className="hidden py-3 whitespace-normal md:table-cell">
        <ScopeChips scopes={token.scopes} />
      </TableCell>
      <TableCell className="hidden py-3 text-muted-foreground lg:table-cell">
        <RelativeTime at={token.created_at} />
      </TableCell>
      <TableCell className="hidden py-3 text-muted-foreground sm:table-cell">
        {token.last_used_at ? (
          <RelativeTime at={token.last_used_at} />
        ) : (
          t("account.tokens.neverUsed")
        )}
      </TableCell>
      <TableCell className="hidden py-3 text-muted-foreground sm:table-cell">
        <Expiry token={token} />
      </TableCell>
      <TableCell className="py-2 pr-4 text-right whitespace-nowrap">
        <ActivityLink agent={token.id} own name={token.name} />
        <Button
          variant="ghost"
          size="icon"
          aria-label={t("account.tokens.revokeLabel", { name: token.name })}
          title={t("account.tokens.revoke")}
          onClick={onRevoke}
          className="text-muted-foreground hover:text-destructive"
        >
          <Trash2Icon />
        </Button>
      </TableCell>
    </TableRow>
  );
}

/** The expiry date; `labelled` says what the date is (the phone layout has no column). */
export function Expiry({ token, labelled = false }: { token: TokenDto; labelled?: boolean }) {
  const { t } = useTranslation("admin");
  if (!token.expires_at) {
    return <>{labelled ? t("account.tokens.neverExpires") : t("account.tokens.never")}</>;
  }
  return (
    <>
      {labelled ? `${t("account.tokens.expiresPrefix")} ` : null}
      <time
        dateTime={token.expires_at}
        title={format(new Date(token.expires_at), "PPpp")}
        className="tabular-nums"
      >
        {format(new Date(token.expires_at), "PP")}
      </time>
    </>
  );
}

export function RevokeDialog({
  token,
  userId,
  onClose,
  onRevoked,
}: {
  token: TokenDto | null;
  userId: string;
  onClose: () => void;
  onRevoked: () => void;
}) {
  const { t } = useTranslation("admin");
  const [pending, setPending] = useState(false);
  // The last token asked about, so the text stays while the dialog closes.
  const [shown, setShown] = useState(token);
  if (token && token !== shown) setShown(token);

  const revoke = async () => {
    if (!token || pending) return;
    setPending(true);
    try {
      await revokeToken(userId, token.id);
      onRevoked();
      toast.success(t("account.tokens.revoked"));
      onClose();
    } catch (error) {
      writeFailure(error);
      toast.error(t("account.tokens.revokeFailed"), {
        description: error instanceof Error ? error.message : undefined,
      });
    } finally {
      setPending(false);
    }
  };

  return (
    <AlertDialog open={token !== null} onOpenChange={(open) => !open && !pending && onClose()}>
      <AlertDialogContent data-slot="revoke-token">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">
            {t("account.tokens.revokeTitle", { name: shown?.name ?? "" })}
          </AlertDialogTitle>
          <AlertDialogDescription>{t("account.tokens.revokeBody")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <Button variant="outline" onClick={onClose} disabled={pending}>
            {t("account.tokens.cancel")}
          </Button>
          <Button
            className={DESTRUCTIVE}
            onClick={() => void revoke()}
            disabled={pending}
            aria-busy={pending}
          >
            {pending ? t("account.tokens.revoking") : t("account.tokens.revokeConfirm")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
