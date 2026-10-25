// /admin/content/$type: the content list (07 §3; M2b-1 Task 13). Status tabs with counts,
// search, filter chips, a dense table with hover actions, infinite paging (50 a page),
// bulk actions from a floating selection bar, trash with Undo, and rows that enter and
// leave with motion. The loader reads the types and the type's counts, so a missing
// type is the not-found screen and a refused or failed read is the shell's forbidden or
// error state; the rows load per filter in the component (a search is not a navigation
// that waits).

import {
  keepPreviousData,
  useInfiniteQuery,
  useQueries,
  useQuery,
  type QueryKey,
  type UseQueryResult,
} from "@tanstack/react-query";
import { Link, createFileRoute, notFound, useNavigate } from "@tanstack/react-router";
import {
  CircleAlertIcon,
  FileTextIcon,
  InboxIcon,
  PlusIcon,
  SearchXIcon,
  Trash2Icon,
  XIcon,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { countsQuery, getContent, listContent } from "@/api/content";
import { qk } from "@/api/keys";
import type { SchemaContentDto } from "@/api/schema";
import { taxonomiesQuery } from "@/api/taxonomies";
import { type ContentTypeDto, typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
import { DataTable } from "@/components/kit/data-table";
import type { RowSelectionState } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
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
import { BulkBar } from "@/content/bulk-bar";
import { type BulkAction, trashUndoable } from "@/content/bulk";
import { ListFilters } from "@/content/filters";
import { type FailureReport, useListActions } from "@/content/list-actions";
import {
  COLUMN_CLASSES,
  type ContentSummary,
  type ParentRef,
  type RowAction,
  editPath,
  useListColumns,
} from "@/content/list-columns";
import {
  type ListSearch,
  hasFilters,
  listQuery,
  parseListSearch,
  tabCount,
} from "@/content/list-search";
import { RowActions } from "@/content/row-actions";
import { StatusTabs } from "@/content/status-tabs";
import { uiLocale } from "@/lib/locale";
import { useShortcut } from "@/lib/shortcuts";

/** Types listed elsewhere (the media library, M2b-2). */
const NOT_LISTED = new Set(["media"]);

interface ListLoaderData {
  label: string;
}

export const Route = createFileRoute("/_app/content/$type/")({
  validateSearch: (raw: Record<string, unknown>): ListSearch => parseListSearch(raw),
  staticData: {
    crumb: (t, data) => (data as ListLoaderData | undefined)?.label ?? t("crumbs.content"),
  },
  loader: async ({ context, params }): Promise<ListLoaderData> => {
    const [types] = await Promise.all([
      context.queryClient.ensureQueryData(typesQuery),
      context.queryClient.ensureQueryData(countsQuery(params.type)),
    ]);
    const type = types.find((ty) => ty.name === params.type);
    if (!type || NOT_LISTED.has(type.name)) throw notFound();
    return { label: type.label };
  },
  component: ContentList,
});

/** One parent per missing id, fetched once (`combine` keeps the result stable). */
function combineParents(results: UseQueryResult<SchemaContentDto>[]) {
  return results.map((r) => r.data);
}

function ContentList() {
  const { type } = Route.useParams();
  const search = Route.useSearch();
  const navigate = useNavigate();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  const { t } = useTranslation("content");

  const types = useQuery(typesQuery);
  const typeDto: ContentTypeDto | undefined = types.data?.find((ty) => ty.name === type);
  const label = typeDto?.label ?? type;
  const lower = label.toLocaleLowerCase();
  const hierarchical = typeDto?.hierarchical ?? false;
  const counts = useQuery(countsQuery(type));
  const taxonomies = useQuery(taxonomiesQuery);
  const typeTaxonomies = useMemo(
    () => (taxonomies.data ?? []).filter((tax) => tax.content_types.includes(type)),
    [taxonomies.data, type],
  );
  const canCreate = me.capabilities.includes("edit_own");

  const filters = useMemo(() => listQuery(search, me.id), [search, me.id]);
  const list = useInfiniteQuery({
    queryKey: qk.content.infinite(type, filters),
    queryFn: ({ pageParam, signal }) =>
      listContent(type, pageParam ? { ...filters, cursor: pageParam } : filters, { signal }),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    // While a new filter loads, the previous rows stay (dimmed), but never another type's.
    placeholderData: (previous, query) =>
      sameType(query?.queryKey, type) ? keepPreviousData(previous) : undefined,
  });
  const items = useMemo(() => {
    const seen = new Map<string, ContentSummary>();
    for (const page of list.data?.pages ?? []) {
      for (const item of page.items) if (!seen.has(item.id)) seen.set(item.id, item);
    }
    return [...seen.values()];
  }, [list.data]);

  // Parents: from the loaded rows, else one `GET` each (hierarchical types only).
  const missingParents = useMemo(() => {
    if (!hierarchical) return [];
    const loaded = new Set(items.map((i) => i.id));
    return [
      ...new Set(
        items.flatMap((i) => (i.parent_id && !loaded.has(i.parent_id) ? [i.parent_id] : [])),
      ),
    ];
  }, [items, hierarchical]);
  const fetchedParents = useQueries({
    queries: missingParents.map((id) => ({
      queryKey: qk.content.item(type, id),
      queryFn: () => getContent(type, id),
      staleTime: 60_000,
    })),
    combine: combineParents,
  });
  const parents = useMemo(() => {
    const map = new Map<string, ParentRef>();
    for (const item of items) map.set(item.id, { title: item.title, slug: item.slug });
    for (const p of fetchedParents) if (p) map.set(p.id, { title: p.title, slug: p.slug });
    return map;
  }, [items, fetchedParents]);

  // Selection belongs to one list: another type or filter starts empty.
  const scope = JSON.stringify([type, filters]);
  const [selection, setSelection] = useState<{ scope: string; ids: RowSelectionState }>({
    scope,
    ids: {},
  });
  const selectedIds = selection.scope === scope ? selection.ids : {};
  const selected = items.filter((i) => selectedIds[i.id]);
  const setSelected = (ids: RowSelectionState) => setSelection({ scope, ids });
  const clearSelection = () => setSelection({ scope, ids: {} });

  const actions = useListActions(type, {
    onDone: (ok) =>
      setSelection((s) => ({
        scope: s.scope,
        ids: Object.fromEntries(Object.entries(s.ids).filter(([id]) => !ok.includes(id))),
      })),
  });
  const [confirm, setConfirm] = useState<Confirm | null>(null);

  // Undo rather than confirm (07 §3): only a permanent delete, or a trash nothing could
  // undo (e.g. a contributor's own drafts: never restorable by them), asks first.
  const run = (action: BulkAction, targets: ContentSummary[]) => {
    if (action === "delete") {
      setConfirm({ kind: "delete", items: targets.filter((i) => i.allowed.delete) });
      return;
    }
    if (action === "trash" && !trashUndoable(targets)) {
      setConfirm({ kind: "trash", items: targets.filter((i) => i.allowed.trash) });
      return;
    }
    actions.run(action, targets);
  };
  const onRowAction = (action: RowAction, item: ContentSummary) => run(action, [item]);
  const open = (item: ContentSummary) => {
    void navigate({ to: editPath(type, item.id) as "/" });
  };

  useShortcut({
    id: "list.trashSelection",
    keys: "#",
    label: t("shortcuts.trashSelection"),
    group: t("shortcuts.group"),
    when: () => selected.some((i) => i.allowed.trash),
    run: () => run("trash", selected),
  });

  const columns = useListColumns({ type, hierarchical, parents });
  const setSearch = (patch: Partial<Record<keyof ListSearch, unknown>>) => {
    void navigate({
      to: "/content/$type",
      params: { type },
      search: (prev: ListSearch) => parseListSearch({ ...prev, ...patch }),
      replace: true,
    });
  };

  const number = new Intl.NumberFormat(uiLocale());
  const total = counts.data ? tabCount(counts.data, "all") : undefined;

  return (
    <div className="mx-auto flex w-full max-w-7xl flex-col gap-5">
      <PageHeader
        title={
          <span className="flex items-baseline gap-2.5">
            {label}
            {total !== undefined ? (
              <span className="text-lg font-normal text-muted-foreground tabular-nums">
                {number.format(total)}
              </span>
            ) : null}
          </span>
        }
        actions={
          canCreate ? (
            <Button asChild>
              <Link to={`/content/${type}/new` as "/"}>
                <PlusIcon data-icon="inline-start" />
                {newLabel(type, t)}
              </Link>
            </Button>
          ) : null
        }
        tabs={<StatusTabs type={type} search={search} counts={counts.data} />}
      />
      <ListFilters label={lower} search={search} taxonomies={typeTaxonomies} onChange={setSearch} />
      {actions.report ? (
        <FailurePanel report={actions.report} onDismiss={actions.dismissReport} />
      ) : null}
      {list.isError && !list.data ? (
        <ErrorState
          error={list.error}
          retry={() => {
            void list.refetch();
          }}
        />
      ) : (
        <DataTable
          label={t("list.tableLabel", { label })}
          columns={columns}
          data={items}
          getRowId={(i) => i.id}
          selection={{ state: selectedIds, onChange: setSelected }}
          onRowOpen={open}
          loading={list.isPending}
          animateRows
          columnClassName={COLUMN_CLASSES}
          className={
            list.isPlaceholderData ? "opacity-60 transition-opacity" : "transition-opacity"
          }
          empty={
            <ListEmpty
              type={type}
              lower={lower}
              search={search}
              canCreate={canCreate}
              onClear={() => setSearch({ q: undefined, mine: undefined, term: undefined })}
            />
          }
          rowActions={(item) => (
            <RowActions type={type} item={item} onOpen={open} onAction={onRowAction} />
          )}
          footer={
            items.length > 0 ? (
              <ListFooter
                hasNext={list.hasNextPage}
                loadingNext={list.isFetchingNextPage}
                shown={items.length}
                onMore={() => {
                  if (!list.isFetchingNextPage) void list.fetchNextPage();
                }}
              />
            ) : null
          }
        />
      )}
      {/* Room for the floating selection bar, so it never hides the last row. */}
      {selected.length > 0 ? <div aria-hidden="true" className="h-14" /> : null}
      <BulkBar
        selected={selected}
        pending={actions.pending}
        onAction={(action) => run(action, selected)}
        onClear={clearSelection}
      />
      <ConfirmDialog
        confirm={confirm}
        onCancel={() => setConfirm(null)}
        onConfirm={({ kind, items: targets }) => {
          setConfirm(null);
          actions.run(kind, targets);
        }}
      />
    </div>
  );
}

/** Whether a list query key (`["content", type, "list", …]`) is of `type`. */
function sameType(key: QueryKey | undefined, type: string): boolean {
  return key?.[1] === type;
}

function newLabel(
  type: string,
  t: (key: "list.newPost" | "list.newPage" | "list.newEntry") => string,
) {
  if (type === "post") return t("list.newPost");
  if (type === "page") return t("list.newPage");
  return t("list.newEntry");
}

/** What an empty list says: per tab, for a search, or for other filters. */
function ListEmpty({
  type,
  lower,
  search,
  canCreate,
  onClear,
}: {
  type: string;
  lower: string;
  search: ListSearch;
  canCreate: boolean;
  onClear: () => void;
}) {
  const { t } = useTranslation("content");
  if (search.q !== undefined) {
    return (
      <EmptyState
        icon={SearchXIcon}
        title={t("empty.search.title", { q: search.q })}
        body={t("empty.search.body", { label: lower })}
        action={
          <Button variant="outline" onClick={onClear}>
            {t("empty.search.action")}
          </Button>
        }
        className="border-0"
      />
    );
  }
  if (hasFilters(search)) {
    return (
      <EmptyState
        icon={SearchXIcon}
        title={t("empty.filtered.title")}
        body={t("empty.filtered.body")}
        action={
          <Button variant="outline" onClick={onClear}>
            {t("filters.clear")}
          </Button>
        }
        className="border-0"
      />
    );
  }
  const tab = search.status ?? "all";
  const icon = tab === "trash" ? Trash2Icon : tab === "all" ? FileTextIcon : InboxIcon;
  return (
    <EmptyState
      icon={icon}
      title={t(`empty.${tab}.title`, { label: lower })}
      body={t(`empty.${tab}.body`, { label: lower })}
      action={
        tab === "all" && canCreate ? (
          <Button asChild>
            <Link to={`/content/${type}/new` as "/"}>
              <PlusIcon data-icon="inline-start" />
              {t("empty.all.action")}
            </Link>
          </Button>
        ) : undefined
      }
      className="border-0"
    />
  );
}

/** Below the rows: "Load more" plus a sentinel that loads the next page when seen. */
function ListFooter({
  hasNext,
  loadingNext,
  shown,
  onMore,
}: {
  hasNext: boolean;
  loadingNext: boolean;
  shown: number;
  onMore: () => void;
}) {
  const { t } = useTranslation("content");
  const sentinel = useRef<HTMLDivElement>(null);
  const latest = useRef(onMore);
  useEffect(() => {
    latest.current = onMore;
  });
  useEffect(() => {
    const el = sentinel.current;
    if (!el || !hasNext || typeof IntersectionObserver === "undefined") return;
    const root = el.closest("[data-slot=data-table]");
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) latest.current();
      },
      { root, rootMargin: "0px 0px 240px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
    // A new page moves the sentinel; observe again so a still-visible one fires again.
  }, [hasNext, shown]);
  const number = new Intl.NumberFormat(uiLocale());
  return (
    <div className="flex flex-col items-center gap-2 border-t px-4 py-3">
      {hasNext ? (
        <>
          <div
            ref={sentinel}
            data-slot="list-sentinel"
            aria-hidden="true"
            className="h-px w-full"
          />
          <Button
            variant="ghost"
            onClick={onMore}
            disabled={loadingNext}
            aria-busy={loadingNext || undefined}
          >
            {loadingNext ? t("list.loadingMore") : t("list.loadMore")}
          </Button>
        </>
      ) : (
        <p className="text-sm text-muted-foreground tabular-nums">
          {t("list.end", { count: shown, formatted: number.format(shown) })}
        </p>
      )}
    </div>
  );
}

/** The failures of the last run, listed until dismissed (a toast would vanish). */
function FailurePanel({ report, onDismiss }: { report: FailureReport; onDismiss: () => void }) {
  const { t } = useTranslation("content");
  return (
    <section
      role="alert"
      aria-labelledby="list-failures"
      data-slot="failure-report"
      className="flex gap-3 rounded-xl border border-destructive/40 bg-destructive/5 p-4"
    >
      <CircleAlertIcon className="mt-0.5 size-5 shrink-0 text-destructive" aria-hidden="true" />
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <h2 id="list-failures" className="font-medium text-foreground">
          {t(`report.${report.action}`, { count: report.failed.length, total: report.total })}
        </h2>
        <ul className="flex flex-col gap-1.5 text-sm">
          {report.failed.map((f) => (
            <li key={f.id} className="flex min-w-0 flex-wrap gap-x-2">
              <span className="max-w-full truncate font-medium" title={f.title}>
                {f.title}
              </span>
              <span className="text-muted-foreground">{failureText(f.error, t)}</span>
            </li>
          ))}
        </ul>
      </div>
      <Button variant="ghost" size="icon-sm" aria-label={t("report.dismiss")} onClick={onDismiss}>
        <XIcon />
      </Button>
    </section>
  );
}

function failureText(
  error: FailureReport["failed"][number]["error"],
  t: (key: "report.network" | "report.stale") => string,
): ReactNode {
  if (error.status === 0) return t("report.network");
  if (error.slug === "precondition-failed") return t("report.stale");
  return error.problem.detail || error.problem.title;
}

/** What asks before it runs: a permanent delete, or a trash nothing could undo. */
interface Confirm {
  kind: "delete" | "trash";
  items: ContentSummary[];
}

/** The two confirms of this screen (07 §3: undo rather than confirm everywhere else). */
function ConfirmDialog({
  confirm,
  onCancel,
  onConfirm,
}: {
  confirm: Confirm | null;
  onCancel: () => void;
  onConfirm: (confirm: Confirm) => void;
}) {
  const { t } = useTranslation("content");
  const count = confirm?.items.length ?? 0;
  const first = confirm?.items[0];
  const kind = confirm?.kind ?? "delete";
  const title = first ? first.title || t("row.untitled") : "";
  return (
    <AlertDialog
      open={confirm !== null && count > 0}
      onOpenChange={(o) => {
        if (!o) onCancel();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t(`${kind}.title`, { count })}</AlertDialogTitle>
          <AlertDialogDescription>
            {count === 1 ? t(`${kind}.bodyOne`, { title }) : t(`${kind}.bodyMany`, { count })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("delete.cancel")}</AlertDialogCancel>
          {/* Filled (a plain `Button`: `AlertDialogAction` puts its class on the inner
              element, so it cannot override the variant): the tinted destructive style
              fails contrast on the dialog's muted footer. The dialog is controlled, so
              confirming closes it through `onConfirm`. */}
          <Button
            variant="destructive"
            className="bg-destructive text-background hover:bg-destructive/90 dark:bg-destructive dark:hover:bg-destructive/90"
            onClick={() => {
              if (confirm) onConfirm(confirm);
            }}
          >
            {t(`${kind}.confirm`, { count })}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
