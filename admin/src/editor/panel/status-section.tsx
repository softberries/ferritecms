// The panel's first section (07 §3, 11 §1): the status, when it went or goes live, who
// wrote it, and the action row. Every button comes from the item's `allowed`
// (`statusActions`, Review Focus 4); Publish and Schedule open the publish dialog, the
// rest run here. A transition with unsaved changes saves them first (F40).

import { ClockIcon, ExternalLinkIcon, SendIcon, UndoDotIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { isContentStatus } from "@/api/content";
import { RelativeTime } from "@/components/kit/relative-time";
import { StatusBadge } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import type { EditorDocument, Transition } from "../editor-state";
import { type PanelAction, formatWhen, statusActions } from "./publishing";

export interface StatusSectionProps {
  doc: EditorDocument;
  /** Opens the publish dialog. */
  onPublish: (mode: "now" | "schedule") => void;
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  const first = parts[0]?.[0] ?? "";
  const last = parts.length > 1 ? (parts[parts.length - 1]?.[0] ?? "") : "";
  return (first + last).toUpperCase();
}

export function StatusSection({ doc, onPublish }: StatusSectionProps) {
  const { t } = useTranslation("editor");
  const { item } = doc;
  const [busy, setBusy] = useState<Transition | null>(null);
  const actions = statusActions(item, doc.readOnly !== null);
  const saving = doc.saveState === "saving" || busy !== null;
  const status = item?.status ?? "draft";

  const run = async (action: Transition) => {
    setBusy(action);
    try {
      const next = await doc.transition(action);
      if (next) {
        toast.success(
          t(
            `panel.toast.${action === "unpublish" && status === "scheduled" ? "unscheduled" : action}`,
          ),
          {
            id: "editor-transition",
          },
        );
      }
    } finally {
      setBusy(null);
    }
  };

  const label = (action: PanelAction): string => {
    switch (action) {
      case "save":
        if (item === null || status === "draft") return t("panel.actions.saveDraft");
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

  const button = (action: PanelAction, variant: "default" | "outline", wide: boolean) => {
    const common = {
      type: "button" as const,
      variant,
      className: wide ? "w-full" : "flex-1",
      "data-action": action,
    };
    switch (action) {
      case "publish":
      case "publishNow":
        return (
          <Button key={action} {...common} disabled={saving} onClick={() => onPublish("now")}>
            {label(action)}
          </Button>
        );
      case "schedule":
        return (
          <Button key={action} {...common} disabled={saving} onClick={() => onPublish("schedule")}>
            <ClockIcon data-icon="inline-start" />
            {label(action)}
          </Button>
        );
      case "submit":
        return (
          <Button
            key={action}
            {...common}
            disabled={saving}
            onClick={() => {
              void run("submit");
            }}
          >
            <SendIcon data-icon="inline-start" />
            {busy === "submit" ? t("panel.actions.submitting") : label(action)}
          </Button>
        );
      case "save":
        return (
          <Button
            key={action}
            {...common}
            // Nothing to save: a quiet outline, not a dimmed filled button.
            variant={item !== null && !doc.dirty ? "outline" : variant}
            disabled={saving || (item !== null && !doc.dirty)}
            aria-keyshortcuts="Meta+S Control+S"
            onClick={() => {
              void doc.save();
            }}
          >
            {doc.saveState === "saving" ? t("panel.actions.saving") : label(action)}
          </Button>
        );
      default:
        return null;
    }
  };

  const [first, ...rest] = actions.primary;
  const author = item?.author;
  const authorName = author ? author.display_name || author.login : null;

  return (
    <section
      aria-labelledby="panel-status"
      data-slot="panel-status"
      className="flex flex-col gap-4"
    >
      <h3 id="panel-status" className="sr-only">
        {t("panel.status.heading")}
      </h3>
      <dl className="grid grid-cols-[5.5rem_minmax(0,1fr)] items-center gap-x-3 gap-y-2.5 text-sm">
        <dt className="text-muted-foreground">{t("panel.status.status")}</dt>
        <dd>
          {item && isContentStatus(item.status) ? (
            <StatusBadge status={item.status} />
          ) : (
            <span className="text-muted-foreground">{t("panel.status.notSaved")}</span>
          )}
        </dd>
        {item?.status === "scheduled" && item.scheduled_at ? (
          <>
            <dt className="text-muted-foreground">{t("panel.status.goesLive")}</dt>
            <dd data-slot="panel-scheduled-at" className="tabular-nums">
              {formatWhen(item.scheduled_at)}
            </dd>
          </>
        ) : null}
        {item?.published_at && (item.status === "published" || item.status === "private") ? (
          <>
            <dt className="text-muted-foreground">{t("panel.status.published")}</dt>
            <dd className="tabular-nums" title={formatWhen(item.published_at)}>
              <RelativeTime at={item.published_at} />
            </dd>
          </>
        ) : null}
        {authorName && author ? (
          <>
            <dt className="text-muted-foreground">{t("panel.status.author")}</dt>
            <dd className="flex min-w-0 items-center gap-2">
              <span
                aria-hidden="true"
                className="flex size-5 shrink-0 items-center justify-center rounded-full bg-muted text-[0.625rem] font-semibold text-muted-foreground"
              >
                {initials(authorName)}
              </span>
              <span className="truncate" title={author.login}>
                {authorName}
              </span>
            </dd>
          </>
        ) : null}
      </dl>
      {item?.status === "pending" && !item.allowed.publish && doc.readOnly === null ? (
        <p className="rounded-md bg-muted/60 px-3 py-2 text-xs text-muted-foreground">
          {t("panel.status.waiting")}
        </p>
      ) : null}
      {first || actions.secondary.length > 0 ? (
        <div data-slot="panel-actions" className="flex flex-col gap-2">
          {first ? button(first, "default", true) : null}
          {rest.length > 0 ? (
            <div className="flex gap-2">{rest.map((a) => button(a, "outline", false))}</div>
          ) : null}
          {actions.secondary.length > 0 ? (
            <div className="flex flex-wrap gap-1">
              {actions.secondary.map((a) =>
                a === "preview" && item ? (
                  <Button
                    key={a}
                    asChild
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground"
                  >
                    <a href={`/_preview/${item.id}`} target="_blank" rel="noopener noreferrer">
                      <ExternalLinkIcon data-icon="inline-start" />
                      {label(a)}
                      <span className="sr-only">{t("panel.newTab")}</span>
                    </a>
                  </Button>
                ) : a === "unpublish" ? (
                  <Button
                    key={a}
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground"
                    disabled={saving}
                    data-action={a}
                    onClick={() => {
                      void run("unpublish");
                    }}
                  >
                    <UndoDotIcon data-icon="inline-start" />
                    {label(a)}
                  </Button>
                ) : null,
              )}
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
