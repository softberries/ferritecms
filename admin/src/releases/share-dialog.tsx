// Share preview… (design 15 §7, §11.2–11.3): a signed link to an item's saved state or a
// release's whole-site preview that opens without signing in. Pick the expiry (1 h, 24 h,
// 7 days), create it, and the link is shown once with Copy (`CopyOnce`; it lives in this
// dialog's state only). Below, the target's links with their expiry and Revoke.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LINK_HOURS,
  type LinkHours,
  type LinkTarget,
  type PreviewLinkDto,
  createPreviewLink,
  previewLinksKey,
  previewLinksQuery,
  revokePreviewLink,
} from "@/api/preview-links";
import { Alert } from "@/auth/auth-layout";
import { CopyOnce } from "@/components/kit/copy-once";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { formatWhen } from "@/editor/panel/publishing";
import { problemText } from "@/structure/taxonomies/terms-model";

export interface ShareDialogProps {
  target: LinkTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ShareDialog({ target, open, onOpenChange }: ShareDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-slot="share-dialog"
        className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-lg"
      >
        {/* Mounted only while open: the shown link goes with it. */}
        {open ? <Body target={target} close={() => onOpenChange(false)} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function Body({ target, close }: { target: LinkTarget; close: () => void }) {
  const { t } = useTranslation("releases");
  const queryClient = useQueryClient();
  const links = useQuery(previewLinksQuery(target));
  const [hours, setHours] = useState<LinkHours>(24);
  const [created, setCreated] = useState<PreviewLinkDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const listId = useId();

  const refresh = () => queryClient.invalidateQueries({ queryKey: previewLinksKey(target) });

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      setCreated(await createPreviewLink(target, hours));
      void refresh();
    } catch (e) {
      setError(t("share.failed", { detail: problemText(e) }));
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (id: string) => {
    try {
      await revokePreviewLink(id);
      void refresh();
    } catch (e) {
      setError(t("share.failed", { detail: problemText(e) }));
    }
  };

  const shown =
    created?.url ??
    (created?.token ? `${window.location.origin}/_preview/link/${created.token}` : null);
  const existing = (links.data ?? []).filter((l) => l.id !== created?.id);

  return (
    <>
      <DialogHeader>
        <DialogTitle>{t("share.title")}</DialogTitle>
        <DialogDescription>
          {target.kind === "release" ? t("share.descriptionRelease") : t("share.descriptionItem")}
        </DialogDescription>
      </DialogHeader>
      <div className="flex min-w-0 flex-col gap-5">
        {shown ? (
          <CopyOnce value={shown} label={t("share.field")} />
        ) : (
          <fieldset className="flex flex-col gap-2">
            <legend className="mb-2 text-sm font-medium">{t("share.expiry")}</legend>
            <div className="grid grid-cols-3 gap-2">
              {LINK_HOURS.map((h) => (
                <label
                  key={h}
                  className="flex min-h-11 cursor-pointer items-center justify-center rounded-lg border px-3 text-sm has-checked:border-primary has-checked:bg-primary/6 has-focus-visible:ring-2 has-focus-visible:ring-ring"
                >
                  <input
                    type="radio"
                    name="share-expiry"
                    value={h}
                    checked={hours === h}
                    onChange={() => setHours(h)}
                    className="sr-only"
                  />
                  {t(`share.hours.${h}`)}
                </label>
              ))}
            </div>
          </fieldset>
        )}
        {error ? <Alert tone="error">{error}</Alert> : null}
        <section className="flex flex-col gap-2">
          <h3 id={listId} className="text-sm font-medium">
            {t("share.links")}
          </h3>
          {existing.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("share.noLinks")}</p>
          ) : (
            <ul aria-labelledby={listId} className="flex flex-col divide-y rounded-lg border">
              {existing.map((l) => (
                <li
                  key={l.id}
                  className="flex items-center justify-between gap-3 px-3 py-2 text-sm"
                >
                  <span className="min-w-0 text-muted-foreground">
                    {t("share.expires", { when: formatWhen(l.expires_at) })}
                    {l.last_used_at
                      ? `, ${t("share.used", { when: formatWhen(l.last_used_at) })}`
                      : ""}
                  </span>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => {
                      void revoke(l.id);
                    }}
                  >
                    {t("share.revoke")}
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={close}>
          {t("share.close")}
        </Button>
        {shown ? null : (
          <Button type="button" disabled={busy} onClick={() => void create()}>
            {busy ? t("share.creating") : t("share.create")}
          </Button>
        )}
      </DialogFooter>
    </>
  );
}
