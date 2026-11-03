// One status action as a button (the panel's action row and the phone publish bar).
// Publish, Schedule and Publish now open the publish dialog; Submit runs in place; the
// save action is quiet (outlined, disabled) while there is nothing to save.

import { ClockIcon, PackagePlusIcon, SendIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import type { EditorDocument } from "../editor-state";
import { useActionRunner } from "./actions";
import { useEditorActions } from "./editor-actions";
import type { PanelAction } from "./publishing";

export interface ActionButtonProps {
  doc: EditorDocument;
  action: PanelAction;
  variant: "default" | "outline";
  className?: string;
  size?: "default" | "sm";
  onPublish: (mode: "now" | "schedule") => void;
}

export function ActionButton({
  doc,
  action,
  variant,
  className,
  size = "default",
  onPublish,
}: ActionButtonProps) {
  const { t } = useTranslation("editor");
  const { label, run, busy } = useActionRunner(doc);
  const actions = useEditorActions();
  const common = { type: "button" as const, variant, size, className, "data-action": action };
  switch (action) {
    case "publish":
    case "publishNow":
      return (
        <Button {...common} disabled={busy} onClick={() => onPublish("now")}>
          {label(action)}
        </Button>
      );
    case "schedule":
      return (
        <Button {...common} disabled={busy} onClick={() => onPublish("schedule")}>
          <ClockIcon data-icon="inline-start" />
          {label(action)}
        </Button>
      );
    case "submit":
      return (
        <Button
          {...common}
          disabled={busy}
          onClick={() => {
            void run("submit");
          }}
        >
          <SendIcon data-icon="inline-start" />
          {doc.pending === "submit" ? t("panel.actions.submitting") : label(action)}
        </Button>
      );
    case "save": {
      const clean = doc.item !== null && !doc.dirty;
      return (
        <Button
          {...common}
          // Nothing to save: a quiet outline, not a dimmed filled button.
          variant={clean ? "outline" : variant}
          disabled={busy || clean}
          aria-keyshortcuts="Meta+S Control+S"
          onClick={() => {
            void doc.save();
          }}
        >
          {doc.saveState === "saving" ? t("panel.actions.saving") : label(action)}
        </Button>
      );
    }
    case "saveChange":
      return (
        <Button
          {...common}
          disabled={busy}
          aria-keyshortcuts="Meta+S Control+S"
          onClick={() => actions.release(true)}
        >
          <PackagePlusIcon data-icon="inline-start" />
          {label(action)}
        </Button>
      );
    default:
      return null;
  }
}
