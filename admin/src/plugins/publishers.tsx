// Publisher keys (M6-2, design 14 §10; `/admin/plugins/publishers`): the keys that sign
// plugins here (approved at an install, added by the operator, listed by the registry)
// with their plugins and revocation state, and Revoke with a reason and "the key was
// stolen" (which disables its plugins on every node). The signature policy is shown
// read-only with the config key that sets it: an admin cannot change it (design 14 D8).
// There is no "add a key" here: that is the operator's (`ferrite plugin keys trust`).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRoundIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { type PublisherKeyDto, publishersQuery, revokePublisher } from "@/api/plugins";
import { Alert } from "@/auth/auth-layout";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import { DESTRUCTIVE } from "@/lib/destructive";
import { problemText } from "@/structure/taxonomies/terms-model";

export function PublishersScreen() {
  const { t } = useTranslation("admin");
  const keys = useQuery(publishersQuery);
  const [revoking, setRevoking] = useState<PublisherKeyDto | null>(null);
  let body;
  if (keys.isError && !keys.data) {
    body = <ErrorState error={keys.error} retry={() => void keys.refetch()} />;
  } else if (!keys.data) {
    body = (
      <div aria-busy="true" className="flex flex-col gap-2">
        <Skeleton className="h-14 w-full rounded-lg" />
        <Skeleton className="h-14 w-full rounded-lg" />
      </div>
    );
  } else if (keys.data.items.length === 0) {
    body = (
      <EmptyState
        icon={KeyRoundIcon}
        title={t("plugins.publishers.emptyTitle")}
        body={t("plugins.publishers.empty")}
      />
    );
  } else {
    body = (
      <ul
        aria-label={t("plugins.publishers.list")}
        data-slot="publisher-keys"
        className="flex flex-col gap-2"
      >
        {keys.data.items.map((k) => (
          <KeyRow key={k.key_id} k={k} onRevoke={() => setRevoking(k)} />
        ))}
      </ul>
    );
  }
  return (
    <div data-slot="publishers-screen" className="mx-auto flex w-full max-w-4xl flex-col gap-5">
      <PageHeader
        title={t("plugins.publishers.title")}
        description={t("plugins.publishers.description")}
      />
      {keys.data ? (
        <p data-slot="signature-policy" className="text-sm text-muted-foreground">
          {t("plugins.publishers.policy", { policy: keys.data.policy })}
        </p>
      ) : null}
      {body}
      <RevokeDialog k={revoking} onClose={() => setRevoking(null)} />
    </div>
  );
}

function KeyRow({ k, onRevoke }: { k: PublisherKeyDto; onRevoke: () => void }) {
  const { t } = useTranslation("admin");
  const source =
    k.source === "approved" || k.source === "operator" || k.source === "registry"
      ? t(`plugins.publishers.source.${k.source}`)
      : k.source;
  const state = k.revoked_at
    ? k.compromised
      ? t("plugins.publishers.compromised", { reason: k.revoked_reason ?? "" })
      : t("plugins.publishers.revoked", { reason: k.revoked_reason ?? "" })
    : t("plugins.publishers.active");
  return (
    <li
      data-slot="publisher-key"
      data-key={k.key_id}
      data-revoked={k.revoked_at ? "true" : "false"}
      className="flex min-w-0 flex-wrap items-start gap-3 rounded-lg border bg-card px-3 py-3"
    >
      <KeyRoundIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5 text-sm">
        <span className="font-mono text-xs break-all text-foreground">{k.key_id}</span>
        <span className="break-words text-foreground">
          {k.registry_publisher ? k.registry_publisher.name : k.label}
        </span>
        <span className="text-xs text-muted-foreground">
          {source} · {k.plugins.length > 0 ? k.plugins.join(", ") : t("plugins.publishers.none")}
        </span>
        <span
          data-slot="publisher-key-state"
          className={k.revoked_at ? "text-xs text-destructive" : "text-xs text-success"}
        >
          {state}
        </span>
      </div>
      {k.revoked_at ? null : (
        <Button
          variant="outline"
          size="sm"
          className="shrink-0 pointer-coarse:min-h-11"
          aria-label={t("plugins.publishers.revokeLabel", { key: k.key_id })}
          onClick={onRevoke}
        >
          {t("plugins.publishers.revoke")}
        </Button>
      )}
    </li>
  );
}

function RevokeDialog({ k, onClose }: { k: PublisherKeyDto | null; onClose: () => void }) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const [reason, setReason] = useState("");
  const [compromised, setCompromised] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [opened, setOpened] = useState<string | null>(null);
  const id = k?.key_id ?? null;
  if (id !== opened) {
    setOpened(id);
    setReason("");
    setCompromised(false);
    setFailure(null);
  }
  const revoke = useMutation({
    mutationFn: () => revokePublisher(id ?? "", reason.trim(), compromised),
    onSuccess: (key) => {
      toast.success(t("plugins.publishers.revokedToast", { key: key.key_id }));
      void queryClient.invalidateQueries({ queryKey: qk.pluginPublishers });
      void queryClient.invalidateQueries({ queryKey: qk.plugins });
      onClose();
    },
    onError: (error) => {
      reportApiError(error);
      setFailure(problemText(error));
    },
  });
  const submit = () => {
    if (!reason.trim()) {
      setFailure(t("plugins.publishers.reasonRequired"));
      return;
    }
    revoke.mutate();
  };
  return (
    <Dialog open={k !== null} onOpenChange={(o) => !o && !revoke.isPending && onClose()}>
      <DialogContent data-slot="revoke-key" className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle className="break-all">
            {t("plugins.publishers.revokeTitle", { key: id ?? "" })}
          </DialogTitle>
          <DialogDescription>{t("plugins.publishers.revokeBody")}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="revoke-reason">{t("plugins.publishers.reason")}</Label>
            <Textarea
              id="revoke-reason"
              value={reason}
              maxLength={500}
              onChange={(e) => setReason(e.currentTarget.value)}
            />
          </div>
          <div className="flex items-start gap-2">
            <Checkbox
              id="revoke-compromised"
              checked={compromised}
              onCheckedChange={(v) => setCompromised(v === true)}
              className="mt-0.5"
            />
            <Label htmlFor="revoke-compromised" className="font-normal">
              {t("plugins.publishers.compromisedLabel")}
            </Label>
          </div>
          {failure ? <Alert tone="error">{failure}</Alert> : null}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={revoke.isPending}>
            {t("plugins.install.cancel")}
          </Button>
          <Button
            variant="destructive"
            className={DESTRUCTIVE}
            onClick={submit}
            disabled={revoke.isPending}
            aria-busy={revoke.isPending || undefined}
          >
            {revoke.isPending ? t("plugins.publishers.revoking") : t("plugins.publishers.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
