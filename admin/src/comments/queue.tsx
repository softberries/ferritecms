// The comments queue (07 §3 Comments; M2b-2 Task 12): status tabs with their counts
// (`GET /comments/counts`, of one item while the queue is narrowed to it), the comments
// newest first, 50 a page, each with the actions its status allows (`moderate.ts`).
//
// - Status changes are optimistic: the rows leave the tab at once, the counts move, and
//   a toast offers Undo, which sets the previous status again (P21). Rows whose request
//   failed come back and are listed in a report that stays until dismissed.
// - Delete permanently (spam and trash only) asks first: it also removes the replies.
// - Keys: the table's `j`/`k`/`x`/`enter` (open the item in the editor), and `a`
//   approve, `u` unapprove, `s` spam, `#` trash on the selection or else the focused
//   row (registered with the shortcuts module, so `?` lists them).
// - Reply opens the inline reply box (`reply-box.tsx`); on a pending comment it is
//   "Approve and reply". Only a public post takes comments, so Reply shows only there.

import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
  type UseQueryResult,
} from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  CheckIcon,
  CircleAlertIcon,
  InboxIcon,
  MessageSquareReplyIcon,
  RotateCcwIcon,
  ShieldAlertIcon,
  ShieldCheckIcon,
  Trash2Icon,
  UndoIcon,
  XCircleIcon,
  XIcon,
  type LucideIcon,
} from "lucide-react";
import { createContext, use, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { type CommentDto, commentCountsQuery, listComments } from "@/api/comments";
import { qk } from "@/api/keys";
import type { ApiError } from "@/api/problem";
import type { SchemaCommentCountsDto, SchemaCommentList } from "@/api/schema";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import type { RowSelectionState } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { TAB_LIST, tabCountClass, tabLinkClass } from "@/components/kit/tab-styles";
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
import { REDIRECTING } from "@/content/bulk";
import { SelectionBar, type SelectionAction } from "@/content/bulk-bar";
import { contentTargetQuery } from "@/content/find-item";
import { UNDO_TOAST_MS } from "@/content/list-actions";
import { editPath } from "@/content/list-columns";
import { DESTRUCTIVE } from "@/lib/destructive";
import { uiLocale } from "@/lib/locale";
import { useShortcut } from "@/lib/shortcuts";
import { CommentRow } from "./comment-row";
import {
  COMMENT_STATUSES,
  type CommentSearch,
  type CommentStatus,
  commentQuery,
  parseCommentSearch,
  statusOfSearch,
  withStatus,
} from "./comment-search";
import {
  type CommentAction,
  actionsFor,
  canDelete,
  nextStatuses,
  runDeletion,
  runModeration,
  statusOf,
} from "./moderate";
import { ReplyBox } from "./reply-box";

/** Approved comments read per item to name the comments that replies answer (the API's
 * page maximum); an older parent reads "Reply to another comment". */
const PARENT_LOOKUP_LIMIT = 200;

/** The pages of several list queries (a stable `combine`, so the result is kept). */
function listData(results: UseQueryResult<SchemaCommentList>[]) {
  return results.map((r) => r.data);
}

const ICONS: Record<CommentAction, LucideIcon> = {
  approve: CheckIcon,
  unapprove: UndoIcon,
  notSpam: ShieldCheckIcon,
  restore: RotateCcwIcon,
  spam: ShieldAlertIcon,
  trash: Trash2Icon,
  reply: MessageSquareReplyIcon,
  delete: XCircleIcon,
};
const DESTRUCTIVE_ACTIONS = new Set<CommentAction>(["trash", "delete"]);

export interface CommentsQueueProps {
  search: CommentSearch;
  setSearch: (patch: Partial<Record<keyof CommentSearch, unknown>>) => void;
}

/** What a run of the last action failed on, listed until dismissed. */
interface Report {
  total: number;
  failed: { id: string; name: string; error: ApiError }[];
}

export function CommentsQueue({ search, setSearch }: CommentsQueueProps) {
  const { t } = useTranslation("comments");
  const navigate = useNavigate();
  const qc = useQueryClient();
  const status = statusOfSearch(search);
  const filters = useMemo(() => commentQuery(search), [search]);

  const counts = useQuery(commentCountsQuery(search.item));
  const list = useInfiniteQuery({
    queryKey: qk.comments.infinite(filters),
    queryFn: ({ pageParam, signal }) =>
      listComments(pageParam ? { ...filters, cursor: pageParam } : filters, { signal }),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    // While another tab loads, the previous rows stay (dimmed).
    placeholderData: keepPreviousData,
  });

  // Rows moved out of this tab, before the server's list says so (optimistic).
  const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
  const loaded = useMemo(() => {
    const seen = new Map<string, CommentDto>();
    for (const page of list.data?.pages ?? []) {
      for (const c of page.items) if (!seen.has(c.id)) seen.set(c.id, c);
    }
    return [...seen.values()];
  }, [list.data]);
  // Placeholder rows of another tab keep their own actions (each row acts by its status).
  const comments = useMemo(() => loaded.filter((c) => !hidden.has(c.id)), [loaded, hidden]);
  // Who a reply answers: from the loaded rows, else from the approved comments of its
  // item (replies go only to approved comments), one request per such item.
  const missingParentItems = useMemo(() => {
    const ids = new Set(loaded.map((c) => c.id));
    return [
      ...new Set(loaded.flatMap((c) => (c.parent_id && !ids.has(c.parent_id) ? [c.item_id] : []))),
    ];
  }, [loaded]);
  const parentLists = useQueries({
    queries: missingParentItems.map((item) => ({
      queryKey: qk.comments.list({ item, status: "approved" }),
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        listComments({ item, status: "approved", limit: PARENT_LOOKUP_LIMIT }, { signal }),
      staleTime: 60_000,
    })),
    combine: listData,
  });
  const names = useMemo(() => {
    const map = new Map<string, string>();
    for (const page of parentLists) for (const c of page?.items ?? []) map.set(c.id, c.author_name);
    for (const c of loaded) map.set(c.id, c.author_name);
    return map;
  }, [loaded, parentLists]);

  // The item's title for the filter chip: from the rows, else the item named by id.
  const itemRef = useQuery({
    ...contentTargetQuery(search.item ?? ""),
    enabled: search.item !== undefined && !loaded.some((c) => c.item_id === search.item),
  });
  const itemTitle =
    loaded.find((c) => c.item_id === search.item)?.item.title ?? itemRef.data?.title;

  // Selection belongs to one tab and filter.
  const scope = JSON.stringify(filters);
  const [selection, setSelection] = useState<{ scope: string; ids: RowSelectionState }>({
    scope,
    ids: {},
  });
  const selectedIds = selection.scope === scope ? selection.ids : {};
  const selected = comments.filter((c) => selectedIds[c.id]);
  const setSelected = (ids: RowSelectionState) => setSelection({ scope, ids });
  const unselect = (ids: readonly string[]) =>
    setSelection((s) => ({
      scope: s.scope,
      ids: Object.fromEntries(Object.entries(s.ids).filter(([id]) => !ids.includes(id))),
    }));

  const [replying, setReplying] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<CommentDto[] | null>(null);
  const [report, setReport] = useState<Report | null>(null);
  // Runs in flight (two groups of one action can run at once).
  const [running, setRunning] = useState(0);
  const busy = running > 0;
  // Every run goes through a mutation, so a `401` among its failures (re-thrown by
  // `failures`) reaches the global handler like any write's.
  const runner = useMutation({
    mutationFn: async (job: () => Promise<void>) => {
      setRunning((n) => n + 1);
      try {
        await job();
      } finally {
        setRunning((n) => n - 1);
      }
    },
  });
  // The row to focus once the acted-on rows have left (keyboard and screen readers keep
  // their place in the list).
  const focusNext = useRef<string | null>(null);
  useEffect(() => {
    const id = focusNext.current;
    if (id === null) return;
    focusNext.current = null;
    document
      .querySelector<HTMLElement>(`[data-slot=comments-queue] tr[data-row-id="${CSS.escape(id)}"]`)
      ?.focus();
  });

  /** The row after (else before) `targets` in the list, if focus is in one of them. */
  const keepFocus = (targets: readonly CommentDto[]) => {
    const ids = new Set(targets.map((c) => c.id));
    const active = document.activeElement;
    const row = active instanceof Element ? active.closest<HTMLElement>("tr[data-row-id]") : null;
    const inTargets = row?.dataset.rowId !== undefined && ids.has(row.dataset.rowId);
    if (!inTargets && active !== document.body) return;
    const last = Math.max(...targets.map((c) => comments.findIndex((x) => x.id === c.id)));
    if (last < 0) return;
    const after = comments.slice(last + 1).find((c) => !ids.has(c.id));
    const before = [...comments.slice(0, last)].reverse().find((c) => !ids.has(c.id));
    const next = after ?? before;
    if (next) focusNext.current = next.id;
  };

  const shiftCounts = (from: CommentStatus, to: CommentStatus, n: number, item?: string) => {
    const shift = (key: readonly unknown[]) =>
      qc.setQueryData<SchemaCommentCountsDto>(key, (old) =>
        old ? { ...old, [from]: Math.max(0, old[from] - n), [to]: old[to] + n } : old,
      );
    shift(qk.comments.counts());
    if (item !== undefined) shift(qk.comments.counts(item));
  };
  const dropCounts = (from: CommentStatus, n: number, item?: string) => {
    const drop = (key: readonly unknown[]) =>
      qc.setQueryData<SchemaCommentCountsDto>(key, (old) =>
        old ? { ...old, [from]: Math.max(0, old[from] - n) } : old,
      );
    drop(qk.comments.counts());
    if (item !== undefined) drop(qk.comments.counts(item));
  };

  const hide = (ids: readonly string[]) => setHidden((h) => new Set([...h, ...ids]));
  const unhide = (ids: readonly string[]) =>
    setHidden((h) => new Set([...h].filter((id) => !ids.includes(id))));

  /** Refetches every list and count, then forgets the rows the server no longer lists. */
  const settle = async (done: readonly string[]) => {
    await qc.invalidateQueries({ queryKey: qk.comments.all });
    unhide(done);
  };

  const failures = (targets: readonly CommentDto[], failed: { id: string; error: ApiError }[]) => {
    const byId = new Map(targets.map((c) => [c.id, c]));
    setReport(
      failed.length > 0
        ? {
            total: targets.length,
            failed: failed.map((f) => ({
              ...f,
              name: byId.get(f.id)?.author_name ?? "",
            })),
          }
        : null,
    );
    const redirect = failed.find((f) => REDIRECTING.has(f.error.slug));
    if (redirect) throw redirect.error;
  };

  /** Moves `targets` (all of status `from`) to `to`; Undo moves them back. */
  const moderate = (
    targets: readonly CommentDto[],
    from: CommentStatus,
    to: CommentStatus,
    undo = false,
  ) => {
    if (targets.length === 0) return;
    runner.mutate(() => moderateRun(targets, from, to, undo));
  };
  const moderateRun = async (
    targets: readonly CommentDto[],
    from: CommentStatus,
    to: CommentStatus,
    undo: boolean,
  ) => {
    const ids = targets.map((c) => c.id);
    keepFocus(targets);
    hide(ids);
    unselect(ids);
    if (replying !== null && ids.includes(replying)) setReplying(null);
    shiftCounts(from, to, ids.length, search.item);
    const result = await runModeration(ids, to);
    unhide(result.failed.map((f) => f.id));
    void settle(result.ok);
    failures(targets, result.failed);
    const done = targets.filter((c) => result.ok.includes(c.id));
    if (done.length === 0) return;
    if (undo) {
      toast.success(t("toast.undone", { count: done.length }));
      return;
    }
    toast.success(t(`toast.${to}`, { count: done.length }), {
      duration: UNDO_TOAST_MS,
      action: {
        label: t("toast.undo"),
        onClick: () => {
          moderate(
            done.map((c) => ({ ...c, status: to })),
            to,
            from,
            true,
          );
        },
      },
    });
  };

  /** Deletes `targets` for good (after the confirm). */
  const remove = (targets: readonly CommentDto[]) => {
    if (targets.length === 0) return;
    runner.mutate(async () => {
      const ids = targets.map((c) => c.id);
      keepFocus(targets);
      hide(ids);
      unselect(ids);
      dropCounts(status, ids.length, search.item);
      const result = await runDeletion(ids);
      unhide(result.failed.map((f) => f.id));
      void settle(result.ok);
      failures(targets, result.failed);
      if (result.ok.length > 0) toast.success(t("toast.deleted", { count: result.ok.length }));
    });
  };

  /** One action on `targets` (all of the current tab's status). */
  const act = (action: CommentAction, targets: readonly CommentDto[]) => {
    if (action === "reply") {
      const first = targets[0];
      if (first) setReplying(first.id);
      return;
    }
    if (action === "delete") {
      const deletable = targets.filter((c) => canDelete(c.status as CommentStatus));
      if (deletable.length > 0) setConfirm(deletable);
      return;
    }
    const to = statusOf(action);
    if (to === null) return;
    // Each comment moves from its own status (a placeholder row may be of another tab).
    const groups = new Map<CommentStatus, CommentDto[]>();
    for (const c of targets) {
      const from = c.status as CommentStatus;
      if (!nextStatuses(from).includes(to)) continue;
      groups.set(from, [...(groups.get(from) ?? []), c]);
    }
    for (const [from, group] of groups) moderate(group, from, to);
  };

  /** The selection, else the focused row (for the queue keys). */
  const keyTargets = (): CommentDto[] => {
    if (selected.length > 0) return selected;
    const active = document.activeElement;
    const row =
      active instanceof Element
        ? active.closest<HTMLElement>("[data-slot=comments-queue] tr[data-row-id]")
        : null;
    const found = comments.find((c) => c.id === row?.dataset.rowId);
    return found ? [found] : [];
  };
  const keyAction = (action: CommentAction) => {
    const targets = keyTargets().filter((c) =>
      actionsFor(c.status as CommentStatus).includes(action),
    );
    if (targets.length > 0) act(action, targets);
  };
  const noOverlay = () =>
    document.querySelector("[role=dialog], [role=alertdialog], [role=menu]") === null;
  const group = t("shortcuts.group");
  useShortcut({
    id: "comments.approve",
    keys: "a",
    label: t("shortcuts.approve"),
    group,
    when: () => !busy && noOverlay(),
    run: () => keyAction("approve"),
  });
  useShortcut({
    id: "comments.unapprove",
    keys: "u",
    label: t("shortcuts.unapprove"),
    group,
    when: () => !busy && noOverlay(),
    run: () => keyAction("unapprove"),
  });
  useShortcut({
    id: "comments.spam",
    keys: "s",
    label: t("shortcuts.spam"),
    group,
    when: () => !busy && noOverlay(),
    run: () => keyAction("spam"),
  });
  useShortcut({
    id: "comments.trash",
    keys: "#",
    label: t("shortcuts.trash"),
    group,
    when: () => !busy && noOverlay(),
    run: () => keyAction("trash"),
  });

  // What the rows need, through a context: the columns stay the same for the screen's
  // life, so a row (an open reply box, an expanded body) keeps its state while the queue
  // around it changes.
  const rowEnv: RowEnv = {
    names,
    busy,
    replying,
    filterable: search.item === undefined,
    act,
    filter: (item) => setSearch({ item }),
    closeReply: (id) => {
      setReplying(null);
      focusNext.current = id;
    },
  };
  const columns = useMemo((): DataTableColumn<CommentDto>[] => {
    const col = dataTableColumns<CommentDto>();
    return [
      col.display({
        id: "comment",
        header: () => t("table.column"),
        cell: (c) => <QueueRow comment={c.row.original} />,
      }),
    ];
  }, [t]);

  const selectionActions: SelectionAction[] = (() => {
    const actions = actionsFor(status).filter((a) => a !== "reply");
    return actions.map((action) => {
      const Icon = ICONS[action];
      return {
        id: action,
        label: t(`actions.${action}`),
        icon: <Icon />,
        destructive: DESTRUCTIVE_ACTIONS.has(action),
        onClick: () => act(action, selected),
      };
    });
  })();

  const number = new Intl.NumberFormat(uiLocale());
  const pending = counts.data?.pending;

  return (
    <div data-slot="comments-queue" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={
          <span className="flex items-baseline gap-2.5">
            {t("title")}
            {pending !== undefined && search.item === undefined ? (
              <span className="text-lg font-normal text-muted-foreground tabular-nums">
                {number.format(pending)}
              </span>
            ) : null}
          </span>
        }
        description={t("description")}
        tabs={<StatusTabs search={search} counts={counts.data} />}
      />
      {search.item !== undefined ? (
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <span className="text-muted-foreground">{t("filter.label")}</span>
          <span
            data-slot="item-chip"
            className="inline-flex h-control max-w-full min-w-0 items-center gap-1 rounded-full border bg-card pr-1 pl-3 shadow-1"
          >
            <span className="truncate font-medium" title={itemTitle ?? t("filter.anItem")}>
              {itemTitle ?? t("filter.anItem")}
            </span>
            <Button
              variant="ghost"
              size="icon-xs"
              className="rounded-full pointer-coarse:size-control"
              aria-label={t("filter.clear")}
              title={t("filter.clear")}
              onClick={() => setSearch({ item: undefined })}
            >
              <XIcon />
            </Button>
          </span>
        </div>
      ) : null}
      {report ? <FailurePanel report={report} onDismiss={() => setReport(null)} /> : null}
      {list.isError && !list.data ? (
        <ErrorState
          error={list.error}
          retry={() => {
            void list.refetch();
          }}
        />
      ) : (
        <RowEnvContext value={rowEnv}>
          <DataTable
            label={t("table.label", { status: t(`tabs.${status}`) })}
            columns={columns}
            data={comments}
            getRowId={(c) => c.id}
            selection={{ state: selectedIds, onChange: setSelected }}
            onRowOpen={(c) => {
              void navigate({ to: editPath(c.item.type, c.item.id) as "/" });
            }}
            openOnClick={false}
            align="top"
            loading={list.isPending}
            animateRows
            columnClassName={{ comment: "w-full max-w-0 whitespace-normal" }}
            className={
              list.isPlaceholderData ? "opacity-60 transition-opacity" : "transition-opacity"
            }
            empty={
              <QueueEmpty
                status={status}
                filtered={search.item !== undefined}
                onClear={() => setSearch({ item: undefined })}
              />
            }
            footer={
              list.hasNextPage ? (
                <div className="flex justify-center border-t px-4 py-3">
                  <Button
                    variant="ghost"
                    disabled={list.isFetchingNextPage}
                    aria-busy={list.isFetchingNextPage || undefined}
                    onClick={() => {
                      if (!list.isFetchingNextPage) void list.fetchNextPage();
                    }}
                  >
                    {list.isFetchingNextPage ? t("list.loadingMore") : t("list.loadMore")}
                  </Button>
                </div>
              ) : null
            }
          />
        </RowEnvContext>
      )}
      {/* Room for the floating selection bar, so it never hides the last row. */}
      {selected.length > 0 ? <div aria-hidden="true" className="h-14" /> : null}
      <SelectionBar
        count={selected.length}
        actions={selectionActions}
        disabled={busy}
        onClear={() => setSelected({})}
      />
      <DeleteDialog
        targets={confirm}
        onCancel={() => setConfirm(null)}
        onConfirm={(targets) => {
          setConfirm(null);
          remove(targets);
        }}
      />
    </div>
  );
}

/** What a row of the queue reads from the screen. */
interface RowEnv {
  /** Comment id → author name (for "Reply to …"). */
  names: ReadonlyMap<string, string>;
  busy: boolean;
  /** The comment whose reply box is open. */
  replying: string | null;
  /** Rows offer "Only comments on this item" (not while the queue is narrowed). */
  filterable: boolean;
  act: (action: CommentAction, targets: readonly CommentDto[]) => void;
  filter: (item: string) => void;
  closeReply: (id: string) => void;
}

const RowEnvContext = createContext<RowEnv | null>(null);

/** One row: the comment, its actions and, while open, its reply box. */
function QueueRow({ comment }: { comment: CommentDto }) {
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

/** Pending, Approved, Spam, Trash: links (each a URL), with their counts. */
function StatusTabs({
  search,
  counts,
}: {
  search: CommentSearch;
  counts: SchemaCommentCountsDto | undefined;
}) {
  const { t } = useTranslation("comments");
  const current = statusOfSearch(search);
  const number = new Intl.NumberFormat(uiLocale());
  return (
    <nav aria-label={t("tabs.label")}>
      <ScrollFade activeSelector={`[data-tab="${current}"]`} activeKey={current}>
        <ul className={TAB_LIST}>
          {COMMENT_STATUSES.map((tab) => {
            const active = tab === current;
            const count = counts?.[tab];
            return (
              <li key={tab}>
                <Link
                  to="/comments"
                  search={(prev: Record<string, unknown>) =>
                    withStatus(parseCommentSearch(prev), tab)
                  }
                  activeOptions={{ exact: true, includeSearch: true }}
                  aria-current={active ? "page" : undefined}
                  data-tab={tab}
                  className={tabLinkClass(active)}
                >
                  {t(`tabs.${tab}`)}
                  {count !== undefined ? (
                    <span data-slot="tab-count" className={tabCountClass(active)}>
                      {number.format(count)}
                    </span>
                  ) : null}
                </Link>
              </li>
            );
          })}
        </ul>
      </ScrollFade>
    </nav>
  );
}

function QueueEmpty({
  status,
  filtered,
  onClear,
}: {
  status: CommentStatus;
  filtered: boolean;
  onClear: () => void;
}) {
  const { t } = useTranslation("comments");
  const icon = status === "trash" ? Trash2Icon : status === "spam" ? ShieldAlertIcon : InboxIcon;
  return (
    <EmptyState
      icon={icon}
      title={t(`empty.${status}.title`)}
      body={filtered ? t("empty.filtered.body") : t(`empty.${status}.body`)}
      action={
        filtered ? (
          <Button variant="outline" onClick={onClear}>
            {t("empty.filtered.action")}
          </Button>
        ) : undefined
      }
      className="border-0"
    />
  );
}

/** The failures of the last run (a toast would vanish). */
function FailurePanel({ report, onDismiss }: { report: Report; onDismiss: () => void }) {
  const { t } = useTranslation("comments");
  const reason = (error: ApiError): ReactNode => {
    if (error.status === 0) return t("report.network");
    if (error.slug === "not-found") return t("report.gone");
    return error.problem.detail || error.problem.title;
  };
  return (
    <section
      role="alert"
      aria-labelledby="comment-failures"
      data-slot="failure-report"
      className="flex gap-3 rounded-xl border border-destructive/40 bg-destructive/5 p-4"
    >
      <CircleAlertIcon className="mt-0.5 size-5 shrink-0 text-destructive" aria-hidden="true" />
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <h2 id="comment-failures" className="font-medium text-foreground">
          {t("report.title", { count: report.failed.length, total: report.total })}
        </h2>
        <ul className="flex flex-col gap-1.5 text-sm">
          {report.failed.map((f) => (
            <li key={f.id} className="flex min-w-0 flex-wrap gap-x-2">
              <span className="max-w-full truncate font-medium" title={f.name}>
                {f.name}
              </span>
              <span className="text-muted-foreground">{reason(f.error)}</span>
            </li>
          ))}
        </ul>
      </div>
      <Button
        variant="ghost"
        size="icon-sm"
        className="pointer-coarse:size-control"
        aria-label={t("report.dismiss")}
        onClick={onDismiss}
      >
        <XIcon />
      </Button>
    </section>
  );
}

/** Delete permanently: asks first, and says the replies go too (the server's rule). */
function DeleteDialog({
  targets,
  onCancel,
  onConfirm,
}: {
  targets: CommentDto[] | null;
  onCancel: () => void;
  onConfirm: (targets: CommentDto[]) => void;
}) {
  const { t } = useTranslation("comments");
  // The last targets, so the dialog keeps its text while it closes.
  const [shown, setShown] = useState<CommentDto[]>([]);
  if (targets !== null && targets !== shown) setShown(targets);
  const list = targets ?? shown;
  const count = list.length;
  return (
    <AlertDialog
      open={targets !== null && targets.length > 0}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <AlertDialogContent data-slot="delete-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("delete.title", { count })}</AlertDialogTitle>
          <AlertDialogDescription className="break-words">
            {t("delete.body", { count, name: list[0]?.author_name ?? "" })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("delete.cancel")}</AlertDialogCancel>
          <Button
            variant="destructive"
            className={DESTRUCTIVE}
            onClick={() => {
              if (targets) onConfirm(targets);
            }}
          >
            {t("delete.confirm", { count })}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
