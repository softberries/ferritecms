// The taxonomies list (07 §3 Taxonomies): label (built-ins marked) with the name in mono,
// whether terms nest, the content types it applies to and its term count (each row reads
// its own `GET /taxonomies/{t}/terms`, the same query the taxonomy's screen and the
// editor use). A row opens the taxonomy's terms. New taxonomy needs `manage_settings`;
// with only `manage_terms` the list is the way to the terms.

import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { PlusIcon, TagsIcon } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { type TaxonomyDto, taxonomiesQuery, termsQuery } from "@/api/taxonomies";
import { typesQuery } from "@/api/types";
import { dataTableColumns } from "@/components/kit/data-table-features";
import { DataTable } from "@/components/kit/data-table";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { NewTaxonomyDialog } from "./taxonomy-form";

/** Built-ins first (category, tag), then by label. */
function listed(taxonomies: readonly TaxonomyDto[]): TaxonomyDto[] {
  return [...taxonomies].sort(
    (a, b) => Number(b.built_in) - Number(a.built_in) || a.label.localeCompare(b.label),
  );
}

export interface TaxonomyListProps {
  /** `manage_settings`: New taxonomy. */
  canManage: boolean;
  /** The New taxonomy dialog is open (`?new=true`). */
  creating: boolean;
  setCreating: (open: boolean) => void;
}

export function TaxonomyList({ canManage, creating, setCreating }: TaxonomyListProps) {
  const { t } = useTranslation("structure");
  const navigate = useNavigate();
  const list = useQuery({ ...taxonomiesQuery, refetchOnMount: "always" });
  const taxonomies = useMemo(() => listed(list.data ?? []), [list.data]);
  const columns = useTaxonomyColumns();
  const open = (tax: TaxonomyDto) =>
    void navigate({ to: "/structure/taxonomies/$name", params: { name: tax.name } });
  const newButton = canManage ? (
    <Button onClick={() => setCreating(true)}>
      <PlusIcon data-icon="inline-start" />
      {t("taxonomies.new")}
    </Button>
  ) : null;

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={
          <span className="flex items-baseline gap-2.5">
            {t("taxonomies.title")}
            {list.data ? (
              <span className="text-lg font-normal text-muted-foreground tabular-nums">
                {taxonomies.length}
              </span>
            ) : null}
          </span>
        }
        description={canManage ? t("taxonomies.description") : t("taxonomies.descriptionTerms")}
        actions={newButton}
      />
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} />
      ) : (
        <DataTable
          columns={columns}
          data={taxonomies}
          getRowId={(tax) => tax.name}
          onRowOpen={open}
          loading={list.isPending}
          label={t("taxonomies.label")}
          columnClassName={{
            hierarchical: "hidden md:table-cell",
            types: "hidden sm:table-cell",
          }}
          empty={
            <EmptyState
              icon={TagsIcon}
              title={t("taxonomies.empty.title")}
              body={t("taxonomies.empty.body")}
              action={newButton}
            />
          }
        />
      )}
      {canManage ? <NewTaxonomyDialog open={creating} onOpenChange={setCreating} /> : null}
    </div>
  );
}

function useTaxonomyColumns() {
  const { t } = useTranslation("structure");
  return useMemo(() => {
    const col = dataTableColumns<TaxonomyDto>();
    return [
      col.accessor("label", {
        header: () => t("taxonomies.columns.label"),
        cell: (c) => {
          const tax = c.row.original;
          return (
            <div className="flex min-w-0 flex-col gap-0.5 py-2">
              <span className="flex min-w-0 items-center gap-2">
                <span
                  className="max-w-[calc(100vw-14rem)] truncate font-medium text-foreground sm:max-w-sm"
                  title={tax.label}
                >
                  {tax.label}
                </span>
                {tax.built_in ? <Badge variant="secondary">{t("types.builtIn")}</Badge> : null}
              </span>
              <code
                className="block max-w-[calc(100vw-12rem)] truncate font-mono text-xs text-muted-foreground sm:max-w-sm"
                title={tax.name}
              >
                {tax.name}
              </code>
            </div>
          );
        },
      }),
      col.accessor("hierarchical", {
        id: "hierarchical",
        header: () => t("taxonomies.columns.hierarchical"),
        cell: (c) => (
          <span className="text-muted-foreground">
            {c.getValue() ? t("taxonomies.nested") : t("taxonomies.flat")}
          </span>
        ),
      }),
      col.accessor("content_types", {
        id: "types",
        header: () => t("taxonomies.columns.types"),
        cell: (c) => <TypeLabels names={c.getValue()} />,
      }),
      col.display({
        id: "terms",
        header: () => t("taxonomies.columns.terms"),
        cell: (c) => <TermCount taxonomy={c.row.original.name} />,
      }),
    ];
  }, [t]);
}

function TypeLabels({ names }: { names: string[] }) {
  const { t } = useTranslation("structure");
  const types = useQuery(typesQuery);
  if (names.length === 0)
    return <span className="text-muted-foreground">{t("taxonomies.noTypes")}</span>;
  const text = names
    .map((name) => types.data?.find((ty) => ty.name === name)?.label ?? name)
    .join(", ");
  return (
    <span className="block max-w-56 truncate text-muted-foreground" title={text}>
      {text}
    </span>
  );
}

/** The row's term count, read when the row is shown. */
function TermCount({ taxonomy }: { taxonomy: string }) {
  const { t } = useTranslation("structure");
  const terms = useQuery(termsQuery(taxonomy));
  if (terms.isPending) {
    return (
      <Skeleton className="h-4 w-8">
        <span className="sr-only">{t("taxonomies.loading")}</span>
      </Skeleton>
    );
  }
  if (terms.isError) {
    return (
      <span className="text-muted-foreground" title={t("taxonomies.countFailed")}>
        —<span className="sr-only">{t("taxonomies.countFailed")}</span>
      </span>
    );
  }
  return (
    <span className="tabular-nums" data-slot="term-count">
      {t("taxonomies.termCount", { count: terms.data.length })}
    </span>
  );
}
