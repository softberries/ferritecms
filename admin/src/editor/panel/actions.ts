// Running the status actions, shared by the panel's status section and the phone
// publish bar: the label of each action (Save draft / Save changes / Update by status,
// Unschedule for a scheduled item) and the transitions that run in place (Submit,
// Unpublish) with their toasts. The transitions go through `doc.transition` (F40).

import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { EditorDocument, Transition } from "../editor-state";
import type { PanelAction } from "./publishing";

export function useActionRunner(doc: EditorDocument) {
  const { t } = useTranslation("editor");
  const status = doc.item?.status ?? "draft";

  const label = (action: PanelAction): string => {
    switch (action) {
      case "save":
        if (doc.item === null || status === "draft") return t("panel.actions.saveDraft");
        if (status === "pending") return t("panel.actions.saveChanges");
        return t("panel.actions.update");
      case "unpublish":
        return status === "scheduled"
          ? t("panel.actions.unschedule")
          : t("panel.actions.unpublish");
      default:
        return t(`panel.actions.${action}`);
    }
  };

  const run = async (action: Extract<Transition, "submit" | "unpublish">) => {
    const unschedule = action === "unpublish" && status === "scheduled";
    const next = await doc.transition(action);
    if (next) {
      toast.success(t(`panel.toast.${unschedule ? "unscheduled" : action}`), {
        id: "editor-transition",
      });
    }
  };

  /** A write is running: every action waits. */
  const busy = doc.saveState === "saving" || doc.pending !== null;
  return { label, run, busy };
}
