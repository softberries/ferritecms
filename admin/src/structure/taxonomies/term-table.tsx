// A flat taxonomy's terms as a table (07 §3 Taxonomies): name, slug and description,
// sorted by name. The API returns every term at once, so the filter runs here (name, slug
// and description, case and accents aside). A row opens the term's sheet; Delete is on the
// row (`manage_terms`; read-only without it).

import { SearchIcon, TagIcon, Trash2Icon, XIcon } from "lucide-react";
import { useMemo, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { TermDto } from "@/api/taxonomies";
import { DataTable } from "@/components/kit/data-table";
import { dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { Button } from "@/components/ui/button";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { filterTerms } from "./terms-model";

export interface TermTableProps {
  label: string;
  terms: TermDto[];
  loading: boolean;
  query: string;
  setQuery: (q: string) => void;
  canEdit: boolean;
  onEdit: (term: TermDto) => void;
  onDelete: (term: TermDto) => void;
  /** Shown when the taxonomy has no terms at all. */
  empty: ReactNode;
}

export function TermTable({
  label,
  terms,
  loading,
  query,
  setQuery,
  canEdit,
  onEdit,
  onDelete,
  empty,
}: TermTableProps) {
  const { t } = useTranslation("structure");
  const input = useRef<HTMLInputElement>(null);
  const shown = useMemo(() => filterTerms(terms, query), [terms, query]);
  const columns = useMemo(() => {
    const col = dataTableColumns<TermDto>();
    return [
      col.accessor("name", {
        header: () => t("taxonomies.terms.columns.name"),
        cell: (c) => (
          <span
            className="block max-w-[calc(100vw-9rem)] truncate py-2 font-medium text-foreground sm:max-w-xs"
            title={c.getValue()}
          >
            {c.getValue()}
          </span>
        ),
      }),
      col.accessor("slug", {
        header: () => t("taxonomies.terms.columns.slug"),
        cell: (c) => (
          <code
            className="block max-w-xs truncate font-mono text-xs text-muted-foreground"
            title={c.getValue()}
          >
            {c.getValue()}
          </code>
        ),
      }),
      col.accessor("description", {
        header: () => t("taxonomies.terms.columns.description"),
        cell: (c) =>
          c.getValue() ? (
            <span className="block max-w-md truncate text-muted-foreground" title={c.getValue()}>
              {c.getValue()}
            </span>
          ) : (
            <span className="text-muted-foreground">—</span>
          ),
      }),
    ];
  }, [t]);

  return (
    <div className="flex flex-col gap-3" data-slot="term-table">
      <div className="flex flex-wrap items-center gap-3">
        <InputGroup className="w-full sm:w-80">
          <InputGroupAddon>
            <SearchIcon />
          </InputGroupAddon>
          <InputGroupInput
            ref={input}
            type="search"
            value={query}
            placeholder={t("taxonomies.terms.filterPlaceholder")}
            aria-label={t("taxonomies.terms.filterLabel", { label })}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape" && query) {
                e.preventDefault();
                setQuery("");
              }
            }}
          />
          {query ? (
            <InputGroupAddon align="inline-end">
              <InputGroupButton
                size="icon-xs"
                aria-label={t("taxonomies.terms.clearFilter")}
                onClick={() => {
                  setQuery("");
                  input.current?.focus();
                }}
              >
                <XIcon />
              </InputGroupButton>
            </InputGroupAddon>
          ) : null}
        </InputGroup>
        {query && !loading ? (
          <p role="status" className="text-sm text-muted-foreground tabular-nums">
            {t("taxonomies.terms.filtered", { shown: shown.length, total: terms.length })}
          </p>
        ) : null}
      </div>
      <DataTable
        columns={columns}
        data={shown}
        getRowId={(term) => term.id}
        loading={loading}
        label={label}
        animateRows
        {...(canEdit ? { onRowOpen: onEdit } : {})}
        columnClassName={{
          slug: "hidden sm:table-cell",
          description: "hidden md:table-cell",
        }}
        empty={
          terms.length === 0 ? (
            empty
          ) : (
            <EmptyState
              icon={TagIcon}
              title={t("taxonomies.terms.noMatch.title", { q: query })}
              body={t("taxonomies.terms.noMatch.body")}
              action={
                <Button variant="outline" onClick={() => setQuery("")}>
                  {t("taxonomies.terms.clearFilter")}
                </Button>
              }
            />
          )
        }
        {...(canEdit
          ? {
              rowActions: (term: TermDto) => (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="pointer-coarse:size-control"
                      aria-label={t("taxonomies.terms.deleteNamed", { name: term.name })}
                      data-term-delete={term.id}
                      onClick={() => onDelete(term)}
                    >
                      <Trash2Icon />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>{t("taxonomies.terms.menu.delete")}</TooltipContent>
                </Tooltip>
              ),
            }
          : {})}
      />
    </div>
  );
}
