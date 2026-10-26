// The phone and tablet publish bar (below 1024 px, where the side panel is a sheet): the
// status and the screen's primary action stay in view at the foot of the editor, with a
// labelled Settings button for the sheet. The action is the panel's first primary one
// (`statusActions`, from `allowed` only) and runs the same way (dialog or F40 transition).

import { PanelRightIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { isContentStatus } from "@/api/content";
import { StatusDot } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import type { EditorDocument } from "../editor-state";
import { ActionButton } from "./action-button";
import { statusActions } from "./publishing";
import { PANEL_ID } from "./side-panel";

export interface PublishBarProps {
  doc: EditorDocument;
  onSettings: () => void;
  settingsOpen: boolean;
  onPublish: (mode: "now" | "schedule") => void;
}

export function PublishBar({ doc, onSettings, settingsOpen, onPublish }: PublishBarProps) {
  const { t } = useTranslation("editor");
  const { t: tc } = useTranslation();
  const status = doc.item?.status;
  const [first] = statusActions(doc.item, doc.readOnly !== null).primary;
  return (
    <div
      data-slot="publish-bar"
      role="region"
      aria-label={t("publishBar.label")}
      className="sticky bottom-0 z-10 flex h-14 items-center gap-2 border-t bg-background/95 px-page pb-[env(safe-area-inset-bottom)] backdrop-blur-md supports-backdrop-filter:bg-background/80"
    >
      <span
        data-slot="publish-bar-status"
        className="flex min-w-0 flex-1 items-center gap-1.5 text-sm"
      >
        {status && isContentStatus(status) ? (
          <>
            <StatusDot status={status} />
            <span className="truncate">{tc(`status.${status}`)}</span>
          </>
        ) : (
          <span className="truncate text-muted-foreground">{t("panel.status.notSaved")}</span>
        )}
      </span>
      <Button
        type="button"
        variant="outline"
        size="sm"
        aria-expanded={settingsOpen}
        aria-controls={settingsOpen ? PANEL_ID : undefined}
        data-slot="panel-toggle"
        onClick={onSettings}
      >
        <PanelRightIcon data-icon="inline-start" />
        {t("publishBar.settings")}
      </Button>
      {first ? (
        <ActionButton doc={doc} action={first} variant="default" size="sm" onPublish={onPublish} />
      ) : null}
    </div>
  );
}
