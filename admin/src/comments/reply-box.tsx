// The moderator's reply under a comment (07 §1 Comments): an inline textarea that posts
// through the public submit `POST /content/{type}/{id}/comments` with the session (the
// client adds the CSRF header) and `parent_id`; a moderator's comment is approved at
// once. Replies go only to approved comments, so under a pending comment the box
// approves it first, then posts ("Approve and reply"). The server's refusals (not a
// public post, comments off, nested too deep, rate limit) show in the box.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { type CommentDto, moderateComment, replyComment } from "@/api/comments";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";

/** The server's limit on a comment body (`MAX_COMMENT_BODY` in ferrite-core). */
export const MAX_REPLY_LENGTH = 10_000;

export interface ReplyBoxProps {
  comment: CommentDto;
  /** The box is done: `posted` after a reply went out. */
  onClose: (posted: boolean) => void;
}

/** Thrown when the approval went through but the reply did not. */
class ReplyAfterApprove extends Error {
  constructor(readonly cause: unknown) {
    super("approved, reply failed");
  }
}

export function ReplyBox({ comment, onClose }: ReplyBoxProps) {
  const { t } = useTranslation("comments");
  const qc = useQueryClient();
  const id = useId();
  const [body, setBody] = useState("");
  const [error, setError] = useState<string | null>(null);
  const approveFirst = comment.status === "pending";

  const problemText = (e: unknown): string => {
    if (!(e instanceof ApiError)) return t("report.network");
    if (e.status === 0) return t("report.network");
    if (e.slug === "not-found") return t("reply.closed");
    const fields = e.fieldErrors();
    return fields.body ?? fields.parent_id ?? (e.problem.detail || e.problem.title);
  };

  const send = useMutation({
    mutationFn: async (text: string) => {
      if (approveFirst) await moderateComment(comment.id, "approved");
      try {
        await replyComment(comment.item.type, comment.item.id, {
          parent_id: comment.id,
          body: text,
        });
      } catch (e) {
        if (approveFirst) throw new ReplyAfterApprove(e);
        throw e;
      }
    },
    onSuccess: () => {
      toast.success(approveFirst ? t("toast.approvedReplied") : t("toast.replied"));
      void qc.invalidateQueries({ queryKey: qk.comments.all });
      onClose(true);
    },
    onError: (e) => {
      if (e instanceof ReplyAfterApprove) {
        // The comment has left Pending; the box goes with its row.
        toast.error(t("reply.approvedOnly", { detail: problemText(e.cause) }));
        void qc.invalidateQueries({ queryKey: qk.comments.all });
        onClose(false);
        return;
      }
      setError(problemText(e));
    },
  });

  const submit = () => {
    const text = body.trim();
    if (text === "") {
      setError(t("reply.empty"));
      return;
    }
    setError(null);
    send.mutate(text);
  };

  return (
    <form
      data-slot="reply-box"
      className="mt-1 flex max-w-prose flex-col gap-2 rounded-lg border bg-muted/40 p-3"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <label htmlFor={`${id}-body`} className="text-sm font-medium">
        {t("reply.label", { name: comment.author_name })}
      </label>
      <Textarea
        id={`${id}-body`}
        // The box opens because someone asked to reply: the caret goes there.
        autoFocus
        rows={3}
        maxLength={MAX_REPLY_LENGTH}
        value={body}
        placeholder={t("reply.placeholder")}
        aria-invalid={error !== null || undefined}
        aria-describedby={`${id}-hint${error ? ` ${id}-error` : ""}`}
        disabled={send.isPending}
        className="min-h-20 bg-background"
        onChange={(e) => {
          setBody(e.target.value);
          if (error) setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            onClose(false);
          } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            submit();
          }
        }}
      />
      {error ? (
        <p id={`${id}-error`} role="alert" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p id={`${id}-hint`} className="text-xs text-muted-foreground">
          {approveFirst ? t("reply.hintApprove") : t("reply.hint")}
        </p>
        <div className="ml-auto flex items-center gap-2">
          <Button
            type="button"
            variant="ghost"
            className="pointer-coarse:h-control"
            onClick={() => onClose(false)}
          >
            {t("reply.cancel")}
          </Button>
          <Button type="submit" disabled={send.isPending} aria-busy={send.isPending || undefined}>
            {send.isPending
              ? t("reply.sending")
              : approveFirst
                ? t("actions.approveReply")
                : t("reply.send")}
          </Button>
        </div>
      </div>
    </form>
  );
}
