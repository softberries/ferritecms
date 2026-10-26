// One taxonomy's screen (07 §3 Taxonomies): its terms as a tree when they nest
// (`term-tree.tsx`), as a filterable table when flat (`term-table.tsx`), the term sheet
// (`term-form.tsx`) and the delete confirm. `manage_terms` writes terms (New term is the
// primary action); `manage_settings` also gets the taxonomy's own settings dialog
// (`taxonomy-form.tsx`). The terms come from the shared `termsQuery`, so the editor's
// terms section sees every change.

import { useQuery } from "@tanstack/react-query";
import { FolderTreeIcon, PlusIcon, Settings2Icon, TagsIcon } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { type TaxonomyDto, type TermDto, termsQuery } from "@/api/taxonomies";
import { typesQuery } from "@/api/types";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { DeleteTermDialog, TermSheet, type TermSheetState } from "./term-form";
import { TermTable } from "./term-table";
import { TermTree } from "./term-tree";
import { TaxonomySettingsDialog } from "./taxonomy-form";

export function TaxonomyScreen({
  taxonomy,
  canSettings,
  canTerms,
}: {
  taxonomy: TaxonomyDto;
  canSettings: boolean;
  canTerms: boolean;
}) {
  const { t } = useTranslation("structure");
  const terms = useQuery({ ...termsQuery(taxonomy.name), refetchOnMount: "always" });
  const types = useQuery(typesQuery);
  const [sheet, setSheet] = useState<TermSheetState>(null);
  const [deleting, setDeleting] = useState<TermDto | null>(null);
  // The tree row whose menu opened the sheet or the delete confirm (focus returns there).
  const [origin, setOrigin] = useState<string | null>(null);
  const [settings, setSettings] = useState(false);
  const [query, setQuery] = useState("");
  const list = useMemo(() => terms.data ?? [], [terms.data]);

  const typeLabels = taxonomy.content_types.map(
    (name) => types.data?.find((ty) => ty.name === name)?.label ?? name,
  );
  // The header's is the primary action; the empty state repeats it in outline.
  const newTerm = (variant: "default" | "outline") =>
    canTerms ? (
      <Button
        variant={variant}
        onClick={() => {
          setOrigin(null);
          setSheet({ kind: "create", parentId: null });
        }}
      >
        <PlusIcon data-icon="inline-start" />
        {t("taxonomies.terms.new")}
      </Button>
    ) : null;
  const empty = (
    <EmptyState
      icon={taxonomy.hierarchical ? FolderTreeIcon : TagsIcon}
      title={t("taxonomies.terms.empty.title")}
      body={
        taxonomy.hierarchical
          ? t("taxonomies.terms.empty.bodyNested", { label: taxonomy.label })
          : t("taxonomies.terms.empty.bodyFlat", { label: taxonomy.label })
      }
      action={newTerm("outline")}
    />
  );

  return (
    <div data-slot="taxonomy-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={
          <span className="flex items-baseline gap-2.5">
            <span className="min-w-0 truncate" title={taxonomy.label}>
              {taxonomy.label}
            </span>
            {terms.data ? (
              <span className="text-lg font-normal text-muted-foreground tabular-nums">
                {list.length}
              </span>
            ) : null}
          </span>
        }
        description={
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <code className="font-mono text-sm text-muted-foreground">{taxonomy.name}</code>
            <Badge variant="secondary">
              {taxonomy.hierarchical ? t("taxonomies.nested") : t("taxonomies.flat")}
            </Badge>
            {taxonomy.built_in ? <Badge variant="outline">{t("types.builtIn")}</Badge> : null}
            <span className="text-sm text-muted-foreground">
              {typeLabels.length > 0
                ? t("taxonomies.appliesTo", { types: typeLabels.join(", ") })
                : t("taxonomies.appliesToNone")}
            </span>
          </span>
        }
        actions={
          <>
            {canSettings ? (
              <Button variant="outline" onClick={() => setSettings(true)}>
                <Settings2Icon data-icon="inline-start" />
                {t("taxonomies.settings.button")}
              </Button>
            ) : null}
            {newTerm("default")}
          </>
        }
      />

      {terms.isError && !terms.data ? (
        <ErrorState error={terms.error} retry={() => void terms.refetch()} />
      ) : taxonomy.hierarchical ? (
        terms.isPending ? (
          <TreeSkeleton />
        ) : list.length === 0 ? (
          empty
        ) : (
          <section className="flex flex-col gap-3" aria-label={t("taxonomies.terms.treeLabel")}>
            {canTerms ? (
              <p className="text-sm text-muted-foreground">{t("taxonomies.terms.treeHint")}</p>
            ) : null}
            <TermTree
              taxonomy={taxonomy}
              terms={list}
              canEdit={canTerms}
              onEdit={(term, from) => {
                setOrigin(from);
                setSheet({ kind: "edit", term });
              }}
              onAddChild={(parent, from) => {
                setOrigin(from);
                setSheet({ kind: "create", parentId: parent.id });
              }}
              onDelete={(term, from) => {
                setOrigin(from);
                setDeleting(term);
              }}
            />
          </section>
        )
      ) : (
        <TermTable
          label={taxonomy.label}
          terms={list}
          loading={terms.isPending}
          query={query}
          setQuery={setQuery}
          canEdit={canTerms}
          onEdit={(term) => {
            setOrigin(null);
            setSheet({ kind: "edit", term });
          }}
          onDelete={(term) => {
            setOrigin(null);
            setDeleting(term);
          }}
          empty={empty}
        />
      )}

      <TermSheet
        taxonomy={taxonomy}
        terms={list}
        state={sheet}
        origin={origin}
        onClose={() => setSheet(null)}
      />
      <DeleteTermDialog
        taxonomy={taxonomy}
        terms={list}
        term={deleting}
        origin={origin}
        onClose={() => setDeleting(null)}
      />
      {canSettings ? (
        <TaxonomySettingsDialog
          open={settings}
          onOpenChange={setSettings}
          taxonomy={taxonomy}
          termCount={terms.data?.length}
        />
      ) : null}
    </div>
  );
}

function TreeSkeleton() {
  const { t } = useTranslation("structure");
  return (
    <div className="flex flex-col gap-1" role="status" aria-label={t("taxonomies.loading")}>
      {[0, 1, 1, 0, 1].map((depth, i) => (
        <Skeleton key={i} className="h-row rounded-lg" style={{ marginInlineStart: depth * 24 }} />
      ))}
    </div>
  );
}
