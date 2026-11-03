// "Suppress…" (design 16 §7.2): a reason is required; the finding stays listed as
// suppressed and counts nowhere. An error on a gated type needs an editor (the server
// answers `403`, shown here).

import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { isProblem } from "@/api/problem";
import type { FindingDto } from "@/api/quality";
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
import { Textarea } from "@/components/ui/textarea";
import { messageFor } from "./findings";

export interface SuppressDialogProps {
  finding: FindingDto | null;
  onClose: () => void;
  /** Sends the suppression; rejects on failure. */
  onSubmit: (input: { key: string; reason: string }) => Promise<void>;
}

export function SuppressDialog({ finding, onClose, onSubmit }: SuppressDialogProps) {
  const { t } = useTranslation("quality");
  const [reason, setReason] = useState("");
  const [whole, setWhole] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const reasonId = useId();
  const hintId = useId();
  const wholeId = useId();
  const trimmed = reason.trim();
  return (
    <Dialog
      open={finding !== null}
      onOpenChange={(open) => {
        if (!open) {
          setReason("");
          setWhole(false);
          setError(null);
          onClose();
        }
      }}
    >
      <DialogContent data-slot="suppress-dialog">
        <DialogHeader>
          <DialogTitle>{t("suppress.title")}</DialogTitle>
          <DialogDescription>{t("suppress.description")}</DialogDescription>
        </DialogHeader>
        {finding ? <p className="text-sm">{messageFor(t, finding)}</p> : null}
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!finding || trimmed === "") return;
            setBusy(true);
            setError(null);
            onSubmit({ key: whole ? "" : finding.key, reason: trimmed })
              .then(() => {
                setReason("");
                setWhole(false);
                onClose();
              })
              .catch((err: unknown) => {
                setError(
                  isProblem(err, "forbidden") ? t("suppress.forbidden") : t("suppress.failed"),
                );
              })
              .finally(() => setBusy(false));
          }}
        >
          <Label htmlFor={reasonId}>{t("suppress.reason")}</Label>
          <Textarea
            id={reasonId}
            value={reason}
            required
            maxLength={500}
            rows={3}
            aria-describedby={hintId}
            aria-invalid={error !== null || undefined}
            onChange={(e) => setReason(e.target.value)}
          />
          <p id={hintId} className="text-xs text-muted-foreground">
            {t("suppress.reasonHint")}
          </p>
          <div className="flex min-h-11 items-center gap-2">
            <Checkbox id={wholeId} checked={whole} onCheckedChange={(v) => setWhole(v === true)} />
            <Label htmlFor={wholeId} className="font-normal">
              {t("suppress.whole")}
            </Label>
          </div>
          {error ? (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          ) : null}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>
              {t("suppress.cancel")}
            </Button>
            <Button type="submit" disabled={busy || trimmed === ""}>
              {t("suppress.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
