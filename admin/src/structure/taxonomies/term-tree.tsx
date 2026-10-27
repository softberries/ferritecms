// A hierarchical taxonomy's terms as a tree (07 §3 Taxonomies): a `SortableTree` in its
// unordered mode, since terms have no sibling order (P35; siblings sort by name). A drop
// only changes the parent: `PATCH /terms/{id}` `{ parent_id }`, shown at once and rolled
// back with the server's reason when it refuses (a `422`, e.g. a cycle formed by someone
// else's move), then read again. Each row's menu has Edit, Add child, Move to… (a parent
// combobox: the keyboard path to any level) and Delete. Without `manage_terms` the tree
// is shown read-only.

import { useQueryClient } from "@tanstack/react-query";
import {
  CornerDownRightIcon,
  FolderInputIcon,
  PencilIcon,
  PlusIcon,
  Trash2Icon,
} from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import type { TaxonomyDto, TermDto } from "@/api/taxonomies";
import { patchTerm } from "@/api/terms";
import { SortableTree } from "@/components/kit/sortable-tree";
import { flatten, type FlatNode } from "@/components/kit/tree-model";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { returnToRowMenu } from "./row-focus";
import { TermParentPicker } from "./term-form";
import { childCount, problemText, toTree, withParent } from "./terms-model";

/**
 * The deepest level a term may reach: core walks a parent chain up to 64 terms
 * (`MAX_PARENT_DEPTH`) and refuses a longer one.
 */
const TERM_MAX_DEPTH = 63;

/** `PATCH /terms/{id}` `{ parent_id }`, shown at once and undone on a refusal. */
function useMoveTerm(taxonomy: string) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  return useCallback(
    async (term: TermDto, parentId: string | null, all: readonly TermDto[]) => {
      const key = qk.terms(taxonomy);
      const was = term.parent_id ?? null;
      if (was === parentId) return;
      await qc.cancelQueries({ queryKey: key });
      qc.setQueryData<TermDto[]>(key, (list) => list && withParent(list, term.id, parentId));
      try {
        const saved = await patchTerm(term.id, { parent_id: parentId });
        qc.setQueryData<TermDto[]>(key, (list) =>
          list?.map((x) => (x.id === saved.id ? saved : x)),
        );
      } catch (err) {
        reportApiError(err);
        // Only this term goes back; other moves may have landed since.
        qc.setQueryData<TermDto[]>(key, (list) => list && withParent(list, term.id, was));
        const parent = all.find((x) => x.id === parentId)?.name ?? "";
        const cycle =
          err instanceof ApiError && err.fieldErrors().parent_id === "parent would create a cycle";
        toast.error(t("taxonomies.terms.move.failed", { name: term.name }), {
          description: cycle
            ? t("taxonomies.terms.move.cycle", { name: term.name, parent })
            : problemText(err),
        });
      }
      void qc.invalidateQueries({ queryKey: key });
    },
    [qc, taxonomy, t],
  );
}

export interface TermTreeProps {
  taxonomy: TaxonomyDto;
  terms: TermDto[];
  /** `manage_terms`: moves and the row menu. */
  canEdit: boolean;
  /** `origin`: the row whose menu opened it (focus goes back there), else `null`. */
  onEdit: (term: TermDto, origin: string | null) => void;
  onAddChild: (parent: TermDto, origin: string) => void;
  onDelete: (term: TermDto, origin: string) => void;
}

export function TermTree({
  taxonomy,
  terms,
  canEdit,
  onEdit,
  onAddChild,
  onDelete,
}: TermTreeProps) {
  const { t } = useTranslation("structure");
  const nodes = useMemo(() => toTree(terms), [terms]);
  const move = useMoveTerm(taxonomy.name);
  const [moving, setMoving] = useState<TermDto | null>(null);
  // The row the Move to… dialog came from (kept past the close, for the focus return).
  const [moveOrigin, setMoveOrigin] = useState<string | null>(null);

  const row = (node: FlatNode<TermDto>) => {
    const term = node.data;
    const children = childCount(terms, term.id);
    return (
      <div className="flex min-w-0 items-center gap-2.5 pr-1">
        {canEdit ? (
          <button
            type="button"
            onClick={() => onEdit(term, null)}
            className="min-w-0 truncate rounded-sm text-left font-medium text-foreground hover:underline"
            title={term.name}
            data-slot="term-name"
          >
            {term.name}
          </button>
        ) : (
          <span className="min-w-0 truncate font-medium" title={term.name} data-slot="term-name">
            {term.name}
          </span>
        )}
        <code
          className="hidden min-w-0 truncate font-mono text-xs text-muted-foreground sm:block"
          title={term.slug}
        >
          {term.slug}
        </code>
        {children > 0 ? (
          <Badge
            variant="secondary"
            className="ml-auto shrink-0 tabular-nums"
            title={t("taxonomies.terms.childCount", { count: children })}
          >
            <span aria-hidden="true">{children}</span>
            <span className="sr-only">{t("taxonomies.terms.childCount", { count: children })}</span>
          </Badge>
        ) : null}
      </div>
    );
  };

  if (!canEdit) {
    return (
      <ul aria-label={taxonomy.label} data-slot="term-tree" className="flex flex-col gap-1">
        {flatten(nodes).map((node) => (
          <li
            key={node.id}
            aria-level={node.depth + 1}
            data-term={node.data.slug}
            style={{ marginInlineStart: node.depth * 24 }}
            className="flex min-h-row items-center gap-2 rounded-lg border bg-card px-3 py-1 text-sm shadow-1"
          >
            {node.depth > 0 ? (
              <CornerDownRightIcon aria-hidden="true" className="size-4 text-muted-foreground" />
            ) : null}
            <div className="min-w-0 flex-1">{row(node)}</div>
          </li>
        ))}
      </ul>
    );
  }

  return (
    <>
      <SortableTree
        nodes={nodes}
        name={taxonomy.label}
        maxDepth={TERM_MAX_DEPTH}
        unordered
        label={(term) => term.name}
        render={row}
        onChange={(_next, moved) => {
          const term = terms.find((x) => x.id === moved.id);
          if (term) void move(term, moved.parentId, terms);
        }}
        actions={(node) => [
          {
            key: "edit",
            overlay: true,
            label: t("taxonomies.terms.menu.edit"),
            icon: PencilIcon,
            disabled: false,
            run: () => onEdit(node.data, node.id),
          },
          {
            key: "child",
            overlay: true,
            label: t("taxonomies.terms.menu.addChild"),
            icon: PlusIcon,
            disabled: node.depth >= TERM_MAX_DEPTH,
            run: () => onAddChild(node.data, node.id),
          },
          {
            key: "move",
            overlay: true,
            label: t("taxonomies.terms.menu.moveTo"),
            icon: FolderInputIcon,
            disabled: false,
            run: () => {
              setMoveOrigin(node.id);
              setMoving(node.data);
            },
          },
          {
            key: "delete",
            overlay: true,
            label: t("taxonomies.terms.menu.delete"),
            icon: Trash2Icon,
            disabled: false,
            separated: true,
            run: () => onDelete(node.data, node.id),
          },
        ]}
      />
      <MoveToDialog
        term={moving}
        origin={moveOrigin}
        terms={terms}
        onClose={() => setMoving(null)}
        onMove={(term, parentId) => {
          setMoving(null);
          void move(term, parentId, terms);
        }}
      />
    </>
  );
}

/** "Move to…": the term's new parent from a combobox (the keyboard path to any level). */
function MoveToDialog({
  term,
  origin,
  terms,
  onClose,
  onMove,
}: {
  term: TermDto | null;
  origin: string | null;
  terms: TermDto[];
  onClose: () => void;
  onMove: (term: TermDto, parentId: string | null) => void;
}) {
  const { t } = useTranslation("structure");
  // The last term, so the dialog keeps its title while it closes.
  const [shown, setShown] = useState<TermDto | null>(term);
  if (term !== null && term !== shown) setShown(term);
  const named = term ?? shown;
  return (
    <Dialog open={term !== null} onOpenChange={(o) => (o ? undefined : onClose())}>
      <DialogContent
        data-slot="move-term-dialog"
        className="sm:max-w-md"
        onCloseAutoFocus={returnToRowMenu(origin)}
      >
        <DialogHeader>
          <DialogTitle className="break-words">
            {t("taxonomies.terms.move.title", { name: named?.name ?? "" })}
          </DialogTitle>
          <DialogDescription>{t("taxonomies.terms.move.description")}</DialogDescription>
        </DialogHeader>
        {term ? <MoveToForm term={term} terms={terms} onClose={onClose} onMove={onMove} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function MoveToForm({
  term,
  terms,
  onClose,
  onMove,
}: {
  term: TermDto;
  terms: TermDto[];
  onClose: () => void;
  onMove: (term: TermDto, parentId: string | null) => void;
}) {
  const { t } = useTranslation("structure");
  const [parentId, setParentId] = useState<string | null>(term.parent_id ?? null);
  const unchanged = parentId === (term.parent_id ?? null);
  return (
    <form
      noValidate
      className="flex flex-col gap-5"
      onSubmit={(e) => {
        e.preventDefault();
        if (!unchanged) onMove(term, parentId);
      }}
    >
      <TermParentPicker terms={terms} termId={term.id} value={parentId} onChange={setParentId} />
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onClose}>
          {t("create.cancel")}
        </Button>
        <Button type="submit" disabled={unchanged}>
          {t("taxonomies.terms.move.submit")}
        </Button>
      </DialogFooter>
    </form>
  );
}
