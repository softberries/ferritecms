// A user's API tokens in the user sheet: read-only but for Revoke (new tokens are made
// in Account only). Token names are the user's own text and render as text.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRoundIcon, Trash2Icon } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { qk } from "@/api/keys";
import { type TokenDto, tokensQuery } from "@/api/tokens";
import { ScopeChips } from "@/account/token-dialog";
import { Expiry, RevokeDialog } from "@/account/tokens-card";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

function isExpired(token: TokenDto): boolean {
  return token.expires_at != null && new Date(token.expires_at).getTime() <= Date.now();
}

/** The user's API tokens, read-only but for Revoke (new ones are made in Account). */
/** `own`: the viewer's own row (the hint then speaks to them). */
export function UserTokens({ userId, own = false }: { userId: string; own?: boolean }) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const list = useQuery(tokensQuery(userId));
  const [revoking, setRevoking] = useState<TokenDto | null>(null);
  const tokens = list.data ?? [];

  let content: ReactNode;
  if (list.isError && !list.data) {
    content = <ErrorState error={list.error} retry={() => void list.refetch()} />;
  } else if (list.isPending) {
    content = (
      <div aria-busy="true" className="flex flex-col gap-2">
        <Skeleton className="h-12 w-full" />
      </div>
    );
  } else if (tokens.length === 0) {
    content = (
      <p className="flex items-center gap-2 rounded-lg border border-dashed px-4 py-3 text-sm text-muted-foreground">
        <KeyRoundIcon aria-hidden="true" className="size-4 shrink-0" />
        {t("users.sheet.noTokens")}
      </p>
    );
  } else {
    content = (
      <ul aria-label={t("users.sheet.tokens")} className="flex flex-col divide-y rounded-lg border">
        {tokens.map((token) => {
          const gone = isExpired(token);
          return (
            <li key={token.id} data-slot="user-token" className="flex items-start gap-3 px-4 py-3">
              <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                <span className="flex min-w-0 items-center gap-2">
                  <span className="truncate text-sm font-medium text-foreground" title={token.name}>
                    {token.name}
                  </span>
                  {gone ? <Badge variant="destructive">{t("account.tokens.expired")}</Badge> : null}
                </span>
                <ScopeChips scopes={token.scopes} />
                <span className="flex flex-wrap gap-x-1 text-xs text-muted-foreground">
                  <span className="whitespace-nowrap">
                    {token.last_used_at ? (
                      <>
                        {t("users.sheet.tokenUsed")} <RelativeTime at={token.last_used_at} />
                      </>
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
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("account.tokens.revokeLabel", { name: token.name })}
                title={t("account.tokens.revoke")}
                onClick={() => setRevoking(token)}
                className="shrink-0 text-muted-foreground hover:text-destructive"
              >
                <Trash2Icon />
              </Button>
            </li>
          );
        })}
      </ul>
    );
  }

  return (
    <>
      {content}
      <p className="text-xs text-muted-foreground">
        {own ? t("users.sheet.tokensHintOwn") : t("users.sheet.tokensHint")}
      </p>
      <RevokeDialog
        token={revoking}
        userId={userId}
        onClose={() => setRevoking(null)}
        onRevoked={() => void queryClient.invalidateQueries({ queryKey: qk.tokens(userId) })}
      />
    </>
  );
}
