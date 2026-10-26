// The restore banner (07 §3, Review Focus 3): the screen opened with a working draft the
// item does not hold (the autosave slot, or a draft kept over a sign-in), and says so
// calmly: "Unsaved changes from 4 minutes ago were restored", with Discard. A draft made
// from an older version says the item changed since; saving it goes through the
// conflict dialog. After Take theirs the banner offers the user's own version, kept in
// the slot, back (Restore) or away (Discard). While it is kept, new edits are not
// autosaved over it (the banner says so); restoring over new edits asks once.

import { HistoryIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";

export type RestoreBannerProps =
  | {
      /** The screen opened with the restored draft. */
      mode: "restored";
      at: string;
      stale: boolean;
      /** A new item: the draft is held on this page only (there is no slot yet). */
      inMemory?: boolean;
      busy: boolean;
      onDiscard: () => void;
    }
  | {
      /** The user took the saved version; theirs is kept in the slot. */
      mode: "kept";
      /** The working draft has edits of its own (Restore would replace them). */
      edited: boolean;
      /** The slot holds it (false: writing it back failed, it is on this page only). */
      stored: boolean;
      busy: boolean;
      onRestore: () => void;
      onDiscard: () => void;
    };

export function RestoreBanner(props: RestoreBannerProps) {
  const { t } = useTranslation("editor");
  const [confirming, setConfirming] = useState(false);
  const replacing = props.mode === "kept" && props.edited && confirming;
  return (
    <div
      data-slot="restore-banner"
      data-mode={props.mode}
      role="status"
      className="mt-6 flex flex-col gap-3 rounded-xl border bg-card px-4 py-3 text-sm shadow-(--shadow-1) sm:flex-row sm:items-center sm:gap-4"
    >
      <div className="flex min-w-0 flex-1 items-start gap-3">
        <span
          aria-hidden="true"
          className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full bg-primary/10 text-primary"
        >
          <HistoryIcon className="size-3.5" />
        </span>
        <div className="flex min-w-0 flex-col gap-0.5">
          {props.mode === "restored" ? (
            <>
              <p className="font-medium text-foreground">
                {t("restore.restoredBefore")} <RelativeTime at={props.at} />{" "}
                {t("restore.restoredAfter")}
              </p>
              <p className="text-muted-foreground">
                {props.stale
                  ? t("restore.stale")
                  : props.inMemory
                    ? t("restore.safeInMemory")
                    : t("restore.safe")}
              </p>
            </>
          ) : (
            <>
              <p className="font-medium text-foreground">
                {props.stored ? t("restore.kept") : t("restore.keptUnstored")}
              </p>
              <p className="text-muted-foreground">
                {replacing
                  ? t("restore.replaceEdits")
                  : props.edited
                    ? t("restore.keptEdited")
                    : t("restore.keptHint")}
              </p>
            </>
          )}
        </div>
      </div>
      <div className="flex shrink-0 gap-2 pl-10 sm:pl-0">
        {props.mode === "kept" ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={props.busy}
            onClick={() => {
              if (props.edited && !confirming) setConfirming(true);
              else props.onRestore();
            }}
          >
            {replacing ? t("restore.replace") : t("restore.restore")}
          </Button>
        ) : null}
        {replacing ? (
          <Button type="button" variant="ghost" size="sm" onClick={() => setConfirming(false)}>
            {t("restore.cancel")}
          </Button>
        ) : null}
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="text-muted-foreground"
          disabled={props.busy}
          hidden={replacing}
          onClick={props.onDiscard}
        >
          {t("restore.discard")}
        </Button>
      </div>
    </div>
  );
}
