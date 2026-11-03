// The side panel's Review section (design 15 §6.2, §11.2), in a type with an approval
// workflow: the open review's steps with their state, the decisions with their comments,
// the submitter's note, the assignees; Approve / Request changes for editors and admins
// (the server refuses anyone not eligible, and says why), Withdraw, Assign… (for accounts
// that can list users), and Submit / Resubmit with a note (and reviewers) when there is no
// open review. After every change the item is read again, so `allowed` stays the server's.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckIcon, CircleDotIcon, CircleIcon, SendIcon, UserPlusIcon } from "lucide-react";
import { cn } from "cn";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { getContent } from "@/api/content";
import {
  MAX_ASSIGNEES,
  MAX_COMMENT,
  type ReviewDto,
  assign,
  decide,
  reviewKeys,
  reviewQuery,
  withdraw,
} from "@/api/reviews";
import { listUsers } from "@/api/users";
import { useMe } from "@/auth/session";
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
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import type { EditorDocument } from "@/editor/editor-state";
import { problemText } from "@/structure/taxonomies/terms-model";
import { DecisionDialog } from "./decision-dialog";
import { ReviewStatus } from "./review-status";

/** Whether the item is in a type with a workflow, as far as the server's answer shows. */
function inWorkflow(doc: EditorDocument): boolean {
  const item = doc.item;
  if (!item) return false;
  const a = item.allowed;
  const open = item.status === "draft" || item.status === "pending";
  return Boolean(item.review) || (open && (a.needs_approval || a.override));
}

export function ReviewSection({ doc, type }: { doc: EditorDocument; type: string }) {
  const { t } = useTranslation("reviews");
  const [submitting, setSubmitting] = useState(false);
  const item = doc.item;
  if (!item || doc.changeMode || !inWorkflow(doc)) return null;
  const refresh = async () => {
    try {
      doc.adopt(await getContent(type, item.id));
    } catch {
      // The panel keeps what it shows; the next load is the server's.
    }
  };
  const summary = item.review;
  return (
    <section
      aria-labelledby="panel-review"
      data-slot="panel-review"
      className="flex flex-col gap-3"
    >
      <h3 id="panel-review" className="text-sm font-semibold">
        {t("section.heading")}
      </h3>
      {summary ? (
        <>
          {!summary.current ? (
            <p className="rounded-md bg-warning/8 px-3 py-2 text-xs text-foreground">
              {t("section.stale")}
            </p>
          ) : null}
          <ReviewDetail
            id={summary.id}
            title={doc.title || item.title}
            onChanged={() => void refresh()}
          />
        </>
      ) : (
        <p className="text-sm text-muted-foreground">{t("section.none")}</p>
      )}
      {item.allowed.submit && doc.readOnly === null ? (
        <Button
          type="button"
          variant="outline"
          data-slot="review-submit"
          disabled={doc.pending !== null}
          onClick={() => setSubmitting(true)}
        >
          <SendIcon data-icon="inline-start" />
          {item.status === "pending" ? t("section.resubmit") : t("section.submit")}
        </Button>
      ) : null}
      <SubmitDialog
        open={submitting}
        onOpenChange={setSubmitting}
        onSubmit={async (submit) => {
          const next = await doc.transition("submit", { submit });
          if (next) toast.success(t("submit.done"), { id: "editor-transition" });
          return next !== null;
        }}
      />
    </section>
  );
}

/** One review: its steps, decisions and note, with the actions the viewer is offered. */
export function ReviewDetail({
  id,
  title,
  onChanged,
}: {
  id: string;
  title: string;
  onChanged: () => void;
}) {
  const { t } = useTranslation("reviews");
  const queryClient = useQueryClient();
  const me = useMe();
  const query = useQuery(reviewQuery(id));
  const [deciding, setDeciding] = useState<"approve" | "request_changes" | null>(null);
  const [assigning, setAssigning] = useState(false);
  const [busy, setBusy] = useState(false);
  const r = query.data;
  if (!r) return query.isError ? null : <Skeleton className="h-24 w-full" />;

  const open = r.status === "in_review" || r.status === "approved";
  const editor = me?.capabilities.includes("edit_others") === true;
  const submitter = me?.id !== undefined && r.submitted_by === me.id;
  const canAssign = me?.capabilities.includes("manage_users") === true;

  const run = async (fn: () => Promise<ReviewDto>, done?: string) => {
    setBusy(true);
    try {
      queryClient.setQueryData(reviewKeys.item(id), await fn());
      void queryClient.invalidateQueries({ queryKey: reviewKeys.lists });
      if (done) toast.success(done, { id: "review-action" });
      onChanged();
    } catch (e) {
      toast.error(t("section.failed", { detail: problemText(e) }), { id: "review-action" });
      void queryClient.invalidateQueries({ queryKey: reviewKeys.item(id) });
    } finally {
      setBusy(false);
    }
  };

  const decisions = r.decisions ?? [];
  const assignees = r.assignees ?? [];
  return (
    <div data-slot="review-detail" className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <ReviewStatus status={r.status} />
      </div>
      <ol aria-label={t("workflow.steps")} className="flex flex-col gap-1.5">
        {r.steps.map((s, i) => {
          const approvals = decisions.filter(
            (d) => d.step === i && d.decision === "approve",
          ).length;
          const state =
            r.status === "approved" || i < r.step
              ? "done"
              : i === r.step && r.status === "in_review"
                ? "current"
                : "waiting";
          const Icon =
            state === "done" ? CheckIcon : state === "current" ? CircleDotIcon : CircleIcon;
          const assigned = assignees.filter((a) => a.step === i).length;
          return (
            <li
              key={s.key}
              data-state={state}
              className={cn(
                "flex items-start gap-2 rounded-md px-2 py-1.5 text-sm",
                state === "current" && "bg-muted/60",
              )}
            >
              <Icon
                aria-hidden="true"
                className={cn(
                  "mt-0.5 size-4 shrink-0",
                  state === "done" ? "text-status-published" : "text-muted-foreground",
                )}
              />
              <span className="flex min-w-0 flex-col">
                <span className="font-medium text-foreground">{s.label}</span>
                <span className="text-xs text-muted-foreground">
                  {t(`section.step.${state}`)} ·{" "}
                  {t("section.approvals", { count: approvals, required: s.required })}
                  {assigned > 0 ? ` · ${t("section.assigned", { count: assigned })}` : ""}
                </span>
              </span>
            </li>
          );
        })}
      </ol>
      {r.note ? (
        <div className="flex flex-col gap-0.5 text-sm">
          <span className="text-xs text-muted-foreground">{t("section.note")}</span>
          <p className="whitespace-pre-wrap text-foreground">{r.note}</p>
        </div>
      ) : null}
      {decisions.length > 0 ? (
        <div className="flex flex-col gap-1.5">
          <span className="text-xs text-muted-foreground">{t("section.decisions")}</span>
          <ul className="flex flex-col gap-1.5">
            {decisions.map((d, i) => (
              <li key={`${d.at}-${i}`} className="rounded-md border px-2.5 py-1.5 text-sm">
                <span className="font-medium">
                  {d.user_id && d.user_id === me?.id
                    ? t("section.decidedBy.you")
                    : t("section.decidedBy.someone")}
                </span>{" "}
                <span className="text-muted-foreground">
                  {d.decision === "approve" ? t("section.approved") : t("section.requested")}
                  {r.steps[d.step] ? ` · ${r.steps[d.step]?.label}` : ""}
                </span>
                {d.comment ? (
                  <p className="mt-0.5 whitespace-pre-wrap text-foreground">{d.comment}</p>
                ) : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {open ? (
        <div data-slot="review-actions" className="flex flex-wrap gap-2">
          {editor && r.status === "in_review" ? (
            <>
              <Button
                type="button"
                size="sm"
                disabled={busy}
                onClick={() => setDeciding("approve")}
              >
                {t("section.approve")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => setDeciding("request_changes")}
              >
                {t("section.requestChanges")}
              </Button>
            </>
          ) : null}
          {canAssign && r.status === "in_review" ? (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => setAssigning(true)}
            >
              <UserPlusIcon data-icon="inline-start" />
              {t("section.assign")}
            </Button>
          ) : null}
          {submitter || editor ? (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              disabled={busy}
              className="text-muted-foreground"
              onClick={() => void run(() => withdraw(id, r.version), t("section.withdrawn"))}
            >
              {t("section.withdraw")}
            </Button>
          ) : null}
        </div>
      ) : null}
      <DecisionDialog
        decision={deciding ?? "approve"}
        title={title}
        open={deciding !== null}
        busy={busy}
        onOpenChange={(o) => {
          if (!o) setDeciding(null);
        }}
        onConfirm={(comment) => {
          const d = deciding;
          if (!d) return;
          setDeciding(null);
          void run(() => decide(id, r.version, d, comment), t("section.decided"));
        }}
      />
      {canAssign ? (
        <AssignDialog
          review={r}
          open={assigning}
          onOpenChange={setAssigning}
          onSaved={() => {
            void queryClient.invalidateQueries({ queryKey: reviewKeys.item(id) });
          }}
        />
      ) : null}
    </div>
  );
}

/** Active editors and admins (who may approve), for accounts that may list users. */
function useApprovers(enabled: boolean) {
  return useQuery({
    queryKey: ["users", "list", "approvers"],
    enabled,
    staleTime: 60_000,
    queryFn: async () => {
      const [editors, admins] = await Promise.all([
        listUsers({ role: "editor", status: "active", limit: 200 }),
        listUsers({ role: "admin", status: "active", limit: 200 }),
      ]);
      return [...editors.items, ...admins.items];
    },
  });
}

function UserChecks({
  users,
  selected,
  onChange,
  max,
}: {
  users: { id: string; login: string; display_name: string }[];
  selected: string[];
  onChange: (next: string[]) => void;
  max: number;
}) {
  return (
    <ul className="flex max-h-60 flex-col gap-1 overflow-y-auto">
      {users.map((u) => {
        const checked = selected.includes(u.id);
        const id = `approver-${u.id}`;
        return (
          <li key={u.id} className="flex min-h-11 items-center gap-2.5 rounded-md px-1">
            <Checkbox
              id={id}
              checked={checked}
              disabled={!checked && selected.length >= max}
              onCheckedChange={(c) =>
                onChange(c === true ? [...selected, u.id] : selected.filter((x) => x !== u.id))
              }
            />
            <Label htmlFor={id} className="font-normal">
              {u.display_name || u.login}
              <span className="text-muted-foreground"> ({u.login})</span>
            </Label>
          </li>
        );
      })}
    </ul>
  );
}

function AssignDialog({
  review,
  open,
  onOpenChange,
  onSaved,
}: {
  review: ReviewDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation("reviews");
  const users = useApprovers(open);
  const [selected, setSelected] = useState<string[]>(() =>
    (review.assignees ?? []).filter((a) => a.step === review.step).map((a) => a.user_id),
  );
  const [busy, setBusy] = useState(false);
  const save = async () => {
    setBusy(true);
    try {
      await assign(review.id, review.step, selected);
      onSaved();
      onOpenChange(false);
    } catch (e) {
      toast.error(t("section.failed", { detail: problemText(e) }), { id: "review-action" });
    } finally {
      setBusy(false);
    }
  };
  const eligible = (users.data ?? []).filter((u) => u.id !== review.submitted_by);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="assign-dialog" className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("assignDialog.title")}</DialogTitle>
          <DialogDescription>
            {t("assignDialog.description", { max: MAX_ASSIGNEES })}
          </DialogDescription>
        </DialogHeader>
        {users.isSuccess && eligible.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t("assignDialog.none")}</p>
        ) : (
          <UserChecks
            users={eligible}
            selected={selected}
            onChange={setSelected}
            max={MAX_ASSIGNEES}
          />
        )}
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={busy}
          >
            {t("assignDialog.cancel")}
          </Button>
          <Button type="button" onClick={() => void save()} disabled={busy}>
            {t("assignDialog.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Submit for review with a note (and reviewers, for accounts that can list users). */
function SubmitDialog({
  open,
  onOpenChange,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSubmit: (submit: { reviewers?: string[]; note?: string }) => Promise<boolean>;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="submit-dialog" className="sm:max-w-md">
        {open ? <SubmitBody onOpenChange={onOpenChange} onSubmit={onSubmit} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function SubmitBody({
  onOpenChange,
  onSubmit,
}: {
  onOpenChange: (open: boolean) => void;
  onSubmit: (submit: { reviewers?: string[]; note?: string }) => Promise<boolean>;
}) {
  const { t } = useTranslation("reviews");
  const me = useMe();
  const canPick = me?.capabilities.includes("manage_users") === true;
  const users = useApprovers(canPick);
  const [reviewers, setReviewers] = useState<string[]>([]);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const noteId = useId();
  const submit = async () => {
    setBusy(true);
    const ok = await onSubmit({
      ...(reviewers.length > 0 ? { reviewers } : {}),
      ...(note.trim() ? { note: note.trim() } : {}),
    });
    setBusy(false);
    if (ok) onOpenChange(false);
  };
  return (
    <>
      <DialogHeader>
        <DialogTitle>{t("submit.title")}</DialogTitle>
        <DialogDescription>{t("submit.description")}</DialogDescription>
      </DialogHeader>
      <div className="flex flex-col gap-4">
        <div className="flex flex-col gap-1.5">
          <span className="text-sm font-medium">{t("submit.reviewers")}</span>
          {canPick ? (
            <>
              <p className="text-xs text-muted-foreground">{t("submit.reviewersHint")}</p>
              <UserChecks
                users={(users.data ?? []).filter((u) => u.id !== me?.id)}
                selected={reviewers}
                onChange={setReviewers}
                max={MAX_ASSIGNEES}
              />
            </>
          ) : (
            <p className="text-sm text-muted-foreground">{t("submit.reviewersUnavailable")}</p>
          )}
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={noteId}>{t("submit.note")}</Label>
          <Textarea
            id={noteId}
            rows={3}
            value={note}
            maxLength={MAX_COMMENT}
            placeholder={t("submit.notePlaceholder")}
            onChange={(e) => setNote(e.target.value)}
          />
        </div>
      </div>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
          {t("submit.cancel")}
        </Button>
        <Button
          type="button"
          data-slot="submit-confirm"
          onClick={() => void submit()}
          disabled={busy}
        >
          {busy ? t("submit.submitting") : t("submit.confirm")}
        </Button>
      </DialogFooter>
    </>
  );
}
