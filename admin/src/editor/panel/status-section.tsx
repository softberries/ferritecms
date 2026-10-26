// The panel's first section (07 §3, 11 §1): the status, when it went or goes live, who
// wrote it, and the action row. Every button comes from the item's `allowed`
// (`statusActions`, Review Focus 4); Publish and Schedule open the publish dialog, the
// rest run here. A transition with unsaved changes saves them first (F40).

import { ExternalLinkIcon, UndoDotIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { isContentStatus } from "@/api/content";
import { RelativeTime } from "@/components/kit/relative-time";
import { StatusBadge } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import type { EditorDocument } from "../editor-state";
import { ActionButton } from "./action-button";
import { useActionRunner } from "./actions";
import { formatWhen, statusActions } from "./publishing";

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
  const { label, run, busy } = useActionRunner(doc);
  const actions = statusActions(item, doc.readOnly !== null);

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
          {first ? (
            <ActionButton
              doc={doc}
              action={first}
              variant="default"
              className="w-full"
              onPublish={onPublish}
            />
          ) : null}
          {rest.length > 0 ? (
            <div className="flex gap-2">
              {rest.map((a) => (
                <ActionButton
                  key={a}
                  doc={doc}
                  action={a}
                  variant="outline"
                  className="flex-1"
                  onPublish={onPublish}
                />
              ))}
            </div>
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
                    disabled={busy}
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
