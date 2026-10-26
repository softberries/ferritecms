// The redirects screen (07 §3 Redirects; `manage_redirects`): every redirect by source,
// 50 a page with Load more; search (`q`, in the URL, 200 ms after the last key, `/`
// focuses it) over source and target; New redirect and Edit in a dialog
// (`redirect-form.tsx`); Delete at once with Undo, which creates it again with `POST`
// (same source, target and code; its hits start again at 0, as the toast says); Import
// (`import-dialog.tsx`) and Export, a plain link to the CSV download (same-origin GET
// with the session cookie). Sources and targets are text, never markup (Review Focus 3).

import {
  keepPreviousData,
  useInfiniteQuery,
  useQueryClient,
  type InfiniteData,
} from "@tanstack/react-query";
import {
  DownloadIcon,
  PencilIcon,
  PlusIcon,
  SearchXIcon,
  SignpostIcon,
  Trash2Icon,
  UploadIcon,
} from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import {
  type RedirectDto,
  deleteRedirect,
  exportRedirectsUrl,
  listRedirects,
  upsertRedirect,
} from "@/api/redirects";
import type { SchemaRedirectList } from "@/api/schema";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { SearchField } from "@/content/filters";
import { UNDO_TOAST_MS } from "@/content/list-actions";
import { uiLocale } from "@/lib/locale";
import { problemText } from "../taxonomies/terms-model";
import { isRedirectCode } from "./codes";
import { ImportDialog } from "./import-dialog";
import { RedirectFormDialog, type RedirectFormState } from "./redirect-form";

/** Redirects a page holds. */
const PAGE_SIZE = 50;

export interface RedirectsSearch {
  q?: string;
  /** The New redirect dialog is open (the palette's "New redirect"). */
  new?: true;
}

export interface RedirectsScreenProps {
  search: RedirectsSearch;
  setSearch: (patch: Partial<RedirectsSearch>) => void;
}

export function RedirectsScreen({ search, setSearch }: RedirectsScreenProps) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const q = search.q ?? "";
  const key = qk.redirects.list(q);
  const list = useInfiniteQuery({
    queryKey: key,
    queryFn: ({ pageParam, signal }) =>
      listRedirects(
        { q, limit: PAGE_SIZE, ...(pageParam ? { cursor: pageParam } : {}) },
        { signal },
      ),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    placeholderData: keepPreviousData,
  });
  const redirects = useMemo(() => {
    const seen = new Map<string, RedirectDto>();
    for (const page of list.data?.pages ?? []) {
      for (const r of page.items) if (!seen.has(r.id)) seen.set(r.id, r);
    }
    return [...seen.values()];
  }, [list.data]);

  const [editing, setEditing] = useState<RedirectFormState | null>(null);
  const formState: RedirectFormState | null = search.new ? NEW : editing;
  const closeForm = () => {
    setEditing(null);
    if (search.new) setSearch({ new: undefined });
  };
  const [importing, setImporting] = useState(false);

  const refresh = () => void qc.invalidateQueries({ queryKey: qk.redirects.all });

  /** Deletes `r` at once; Undo creates it again (hits from 0). */
  const remove = async (r: RedirectDto) => {
    qc.setQueryData<InfiniteData<SchemaRedirectList, string>>(key, (data) =>
      data
        ? {
            ...data,
            pages: data.pages.map((p) => ({ ...p, items: p.items.filter((x) => x.id !== r.id) })),
          }
        : data,
    );
    try {
      await deleteRedirect(r.id);
    } catch (err) {
      reportApiError(err);
      refresh();
      toast.error(t("redirects.deleteFailed", { source: r.source }), {
        description: problemText(err),
      });
      return;
    }
    refresh();
    toast.success(t("redirects.deleted"), {
      description: t("redirects.deletedHint"),
      duration: UNDO_TOAST_MS,
      action: {
        label: t("redirects.undo"),
        onClick: () => {
          void (async () => {
            try {
              await upsertRedirect({ source: r.source, target: r.target, code: r.code });
              toast.success(t("redirects.restored", { source: r.source }));
            } catch (err) {
              reportApiError(err);
              toast.error(t("redirects.restoreFailed", { source: r.source }), {
                description: problemText(err),
              });
            } finally {
              refresh();
            }
          })();
        },
      },
    });
  };

  const rowEnv: RowEnv = {
    edit: (r) => setEditing({ kind: "edit", redirect: r }),
    remove,
  };
  const columns = useColumns();

  const actions = (
    <>
      <Button variant="outline" onClick={() => setImporting(true)}>
        <UploadIcon data-icon="inline-start" />
        {t("redirects.importAction")}
      </Button>
      <Button asChild variant="outline">
        <a href={exportRedirectsUrl} download="redirects.csv">
          <DownloadIcon data-icon="inline-start" />
          {t("redirects.export")}
        </a>
      </Button>
      <Button onClick={() => setSearch({ new: true })}>
        <PlusIcon data-icon="inline-start" />
        {t("redirects.new")}
      </Button>
    </>
  );

  let empty: ReactNode;
  if (q) {
    empty = (
      <EmptyState
        icon={SearchXIcon}
        title={t("redirects.noMatch.title")}
        body={t("redirects.noMatch.body", { q })}
        action={
          <Button variant="outline" onClick={() => setSearch({ q: undefined })}>
            {t("redirects.noMatch.clear")}
          </Button>
        }
        className="border-0"
      />
    );
  } else {
    empty = (
      <EmptyState
        icon={SignpostIcon}
        title={t("redirects.empty.title")}
        body={t("redirects.empty.body")}
        action={
          <Button variant="outline" onClick={() => setSearch({ new: true })}>
            <PlusIcon data-icon="inline-start" />
            {t("redirects.new")}
          </Button>
        }
        className="border-0"
      />
    );
  }

  return (
    <div data-slot="redirects-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={t("redirects.title")}
        description={t("redirects.description")}
        actions={actions}
      />
      <SearchField
        label={t("redirects.searchLabel")}
        value={q}
        onSearch={(text) => setSearch({ q: text || undefined })}
      />
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} />
      ) : (
        <DataTable
          label={t("redirects.title")}
          columns={columns}
          data={redirects}
          getRowId={(r) => r.id}
          onRowOpen={rowEnv.edit}
          loading={list.isPending}
          animateRows
          rowActions={(r) => <RowActions redirect={r} env={rowEnv} />}
          columnClassName={{
            source: "w-full max-w-0 md:w-[34%]",
            target: "hidden max-w-0 md:table-cell md:w-[34%]",
            hits: "hidden text-right sm:table-cell",
            last_hit: "hidden whitespace-nowrap lg:table-cell",
            origin: "hidden md:table-cell",
          }}
          className={
            list.isPlaceholderData ? "opacity-60 transition-opacity" : "transition-opacity"
          }
          empty={empty}
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
                  {list.isFetchingNextPage ? t("redirects.loadingMore") : t("redirects.loadMore")}
                </Button>
              </div>
            ) : null
          }
        />
      )}
      <RedirectFormDialog
        state={formState}
        onClose={closeForm}
        onSaved={(saved, how) => {
          closeForm();
          refresh();
          toast.success(t(`redirects.${how}`, { source: saved.source }));
        }}
      />
      <ImportDialog
        open={importing}
        onOpenChange={setImporting}
        onImported={(report) => {
          setImporting(false);
          refresh();
          const count = report.created + report.updated;
          toast.success(t("redirects.imported", { count }), {
            description: t("redirects.importedHint", {
              created: report.created,
              updated: report.updated,
            }),
          });
        }}
      />
    </div>
  );
}

/** The New form's state (one object, so the dialog keeps its form while open). */
const NEW: RedirectFormState = { kind: "new" };

interface RowEnv {
  edit: (r: RedirectDto) => void;
  remove: (r: RedirectDto) => Promise<void>;
}

function useColumns(): DataTableColumn<RedirectDto>[] {
  const { t } = useTranslation("structure");
  return useMemo(() => {
    const col = dataTableColumns<RedirectDto>();
    const number = new Intl.NumberFormat(uiLocale());
    return [
      col.accessor("source", {
        id: "source",
        header: () => t("redirects.columns.source"),
        cell: (c) => (
          <div className="flex min-w-0 flex-col py-2">
            <code
              data-slot="redirect-source"
              className="truncate font-mono text-sm text-foreground"
              title={c.row.original.source}
            >
              {c.row.original.source}
            </code>
            {/* Below `md` the target column is hidden: the target goes under the source. */}
            <span
              className="truncate font-mono text-xs text-muted-foreground md:hidden"
              title={c.row.original.target}
            >
              <span aria-hidden="true">→ </span>
              <span className="sr-only">{t("redirects.columns.target")}: </span>
              {c.row.original.target}
            </span>
          </div>
        ),
      }),
      col.accessor("target", {
        id: "target",
        header: () => t("redirects.columns.target"),
        cell: (c) => (
          <code
            className="block truncate font-mono text-sm text-muted-foreground"
            title={c.row.original.target}
          >
            {c.row.original.target}
          </code>
        ),
      }),
      col.accessor("code", {
        id: "code",
        header: () => t("redirects.columns.code"),
        cell: (c) => (
          <span
            className="font-mono text-sm tabular-nums"
            title={
              isRedirectCode(c.row.original.code)
                ? t(`redirects.codes.${c.row.original.code}.label`)
                : undefined
            }
          >
            {c.row.original.code}
          </span>
        ),
      }),
      col.accessor("hits", {
        id: "hits",
        header: () => t("redirects.columns.hits"),
        cell: (c) => <span className="tabular-nums">{number.format(c.row.original.hits)}</span>,
      }),
      col.accessor("last_hit_at", {
        id: "last_hit",
        header: () => t("redirects.columns.lastHit"),
        cell: (c) =>
          c.row.original.last_hit_at ? (
            <RelativeTime at={c.row.original.last_hit_at} className="text-muted-foreground" />
          ) : (
            <span className="text-muted-foreground">{t("redirects.never")}</span>
          ),
      }),
      col.accessor("origin", {
        id: "origin",
        header: () => t("redirects.columns.origin"),
        cell: (c) => (
          <Badge variant={c.row.original.origin === "manual" ? "outline" : "secondary"}>
            {t(`redirects.origin.${c.row.original.origin}`)}
          </Badge>
        ),
      }),
    ];
  }, [t]);
}

function RowActions({ redirect, env }: { redirect: RedirectDto; env: RowEnv }) {
  const { t } = useTranslation("structure");
  const quick = (
    label: string,
    name: string,
    icon: ReactNode,
    onClick: () => void,
    destructive = false,
  ) => (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className={
            destructive
              ? "text-destructive hover:text-destructive pointer-coarse:size-control"
              : "pointer-coarse:size-control"
          }
          aria-label={name}
          onClick={onClick}
        >
          {icon}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
  return (
    <>
      {quick(
        t("redirects.edit"),
        t("redirects.editOn", { source: redirect.source }),
        <PencilIcon />,
        () => env.edit(redirect),
      )}
      {quick(
        t("redirects.delete"),
        t("redirects.deleteOn", { source: redirect.source }),
        <Trash2Icon />,
        () => void env.remove(redirect),
        true,
      )}
    </>
  );
}
