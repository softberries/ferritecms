// Approve / Request changes (design 15 §6.2): a confirm step that needs nothing but takes an
// optional comment (at most 2000 characters) for the submitter.

import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { MAX_COMMENT } from "@/api/reviews";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

export interface DecisionDialogProps {
  decision: "approve" | "request_changes";
  /** The item's title. */
  title: string;
  open: boolean;
  busy: boolean;
  onOpenChange: (open: boolean) => void;
  /** `comment`: trimmed, `undefined` when none was written. */
  onConfirm: (comment: string | undefined) => void;
}

export function DecisionDialog(props: DecisionDialogProps) {
  return (
    <AlertDialog open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialogContent data-slot="decision-dialog" className="sm:max-w-lg">
        {props.open ? <Body key={props.decision} {...props} /> : null}
      </AlertDialogContent>
    </AlertDialog>
  );
}

function Body({ decision, title, busy, onConfirm }: DecisionDialogProps) {
  const { t } = useTranslation("reviews");
  const id = useId();
  const [comment, setComment] = useState("");
  const approve = decision === "approve";
  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>
          {approve ? t("decision.approveTitle", { title }) : t("decision.changesTitle", { title })}
        </AlertDialogTitle>
        <AlertDialogDescription>
          {approve ? t("decision.approveBody") : t("decision.changesBody")}
        </AlertDialogDescription>
      </AlertDialogHeader>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={id}>{t("decision.comment")}</Label>
        <Textarea
          id={id}
          rows={3}
          value={comment}
          maxLength={MAX_COMMENT}
          aria-describedby={`${id}-hint`}
          onChange={(e) => setComment(e.target.value)}
        />
        <p id={`${id}-hint`} className="text-xs text-muted-foreground">
          {t("decision.commentHint")}
        </p>
      </div>
      <AlertDialogFooter>
        <AlertDialogCancel disabled={busy}>{t("decision.cancel")}</AlertDialogCancel>
        <Button
          type="button"
          variant={approve ? "default" : "destructive"}
          disabled={busy}
          data-slot="decision-confirm"
          onClick={() => onConfirm(comment.trim() === "" ? undefined : comment.trim())}
        >
          {approve ? t("decision.approve") : t("decision.requestChanges")}
        </Button>
      </AlertDialogFooter>
    </>
  );
}
