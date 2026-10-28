// A row of the comments queue (`queue.tsx`): the comment, the buttons its status allows
// (`moderate.ts`) and, while open, its reply box. What a row needs from the screen comes
// through `RowEnvContext`, so the table's cells stay stable while the screen re-renders.

import { use } from "react";
import { useTranslation } from "react-i18next";
import type { CommentDto } from "@/api/comments";
import { Button } from "@/components/ui/button";
import { CommentRow } from "./comment-row";
import type { CommentStatus } from "./comment-search";
import { type CommentAction, actionsFor } from "./moderate";
import { DESTRUCTIVE_ACTIONS, ICONS, RowEnvContext } from "./queue-row-env";
import { ReplyBox } from "./reply-box";

/** One row: the comment, its actions and, while open, its reply box. */
export function QueueRow({ comment }: { comment: CommentDto }) {
  const env = use(RowEnvContext);
  if (!env) return null;
  const replying = env.replying === comment.id;
  return (
    <CommentRow
      comment={comment}
      parentName={comment.parent_id ? (env.names.get(comment.parent_id) ?? null) : null}
      {...(env.filterable ? { onFilterItem: () => env.filter(comment.item_id) } : {})}
      actions={
        <RowActions
          comment={comment}
          disabled={env.busy}
          replying={replying}
          onAction={(action) => env.act(action, [comment])}
        />
      }
      footer={
        replying ? <ReplyBox comment={comment} onClose={() => env.closeReply(comment.id)} /> : null
      }
    />
  );
}

/** A row's buttons, by its status (`actionsFor`). Reply only on a public post. */
function RowActions({
  comment,
  disabled,
  replying,
  onAction,
}: {
  comment: CommentDto;
  disabled: boolean;
  replying: boolean;
  onAction: (action: CommentAction) => void;
}) {
  const { t } = useTranslation("comments");
  const status = comment.status as CommentStatus;
  const replyable = comment.item.type === "post" && comment.item.permalink !== null;
  const actions = actionsFor(status).filter((a) => a !== "reply" || replyable);
  return (
    <div
      role="group"
      aria-label={t("actions.label", { name: comment.author_name })}
      className="-ml-2 flex flex-wrap items-center gap-1"
    >
      {actions.map((action) => {
        const Icon = ICONS[action];
        const label =
          action === "reply" && status === "pending"
            ? t("actions.approveReply")
            : t(`actions.${action}`);
        return (
          <Button
            key={action}
            variant="ghost"
            size="sm"
            disabled={disabled || (action === "reply" && replying)}
            aria-label={t("actions.on", { action: label, name: comment.author_name })}
            data-action={action}
            className={
              DESTRUCTIVE_ACTIONS.has(action)
                ? "text-destructive hover:text-destructive pointer-coarse:h-control"
                : action === "approve"
                  ? "text-success hover:text-success pointer-coarse:h-control"
                  : "pointer-coarse:h-control"
            }
            onClick={() => onAction(action)}
          >
            <Icon data-icon="inline-start" />
            {label}
          </Button>
        );
      })}
    </div>
  );
}
