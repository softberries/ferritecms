// The content types list (07 §3 Content types): every type but `media` (its own
// screen), built-ins first and marked, then custom types by label; each row shows the
// label, the name in mono, whether items nest, the archive and how many fields the schema
// has ("Any" without one). "New type" opens a dialog: label, a name derived from it until
// typed (it cannot change once created), hierarchy and archive.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { BlocksIcon, PlusIcon } from "lucide-react";
import { useMemo, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type ContentTypeDto, createType, typesQuery } from "@/api/types";
import { dataTableColumns } from "@/components/kit/data-table-features";
import { DataTable } from "@/components/kit/data-table";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
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
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { nameFromLabel } from "./builder-model";
import { FormField, SwitchRow } from "./form-field";
import { MAX_TYPE_LABEL, MAX_TYPE_NAME, labelProblem, validTypeName } from "./type-rules";

const without = (errs: Record<string, string>, key: string) =>
  Object.fromEntries(Object.entries(errs).filter(([k]) => k !== key));

/** Listed elsewhere (the media library). */
const NOT_LISTED = new Set(["media"]);
const BUILT_IN_ORDER = ["post", "page"];

/** The listed types: built-ins first (post, page, then any other), then by label. */
function listedTypes(types: readonly ContentTypeDto[]): ContentTypeDto[] {
  const shown = types.filter((ty) => !NOT_LISTED.has(ty.name));
  const rank = (ty: ContentTypeDto) => {
    const i = BUILT_IN_ORDER.indexOf(ty.name);
    return i >= 0 ? i : ty.built_in ? BUILT_IN_ORDER.length : BUILT_IN_ORDER.length + 1;
  };
  return [...shown].sort((a, b) => rank(a) - rank(b) || a.label.localeCompare(b.label));
}

export interface TypesListProps {
  /** The New type dialog is open (`?new=true`). */
  creating: boolean;
  setCreating: (open: boolean) => void;
}

export function TypesList({ creating, setCreating }: TypesListProps) {
  const { t } = useTranslation("structure");
  const navigate = useNavigate();
  const list = useQuery({ ...typesQuery, refetchOnMount: "always" });
  const types = useMemo(() => listedTypes(list.data ?? []), [list.data]);
  const columns = useTypeColumns();
  const open = (ty: ContentTypeDto) =>
    void navigate({ to: "/structure/types/$name", params: { name: ty.name } });

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={
          <span className="flex items-baseline gap-2.5">
            {t("types.title")}
            {list.data ? (
              <span className="text-lg font-normal text-muted-foreground tabular-nums">
                {types.length}
              </span>
            ) : null}
          </span>
        }
        description={t("types.description")}
        actions={
          <Button onClick={() => setCreating(true)}>
            <PlusIcon data-icon="inline-start" />
            {t("types.new")}
          </Button>
        }
      />
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} />
      ) : (
        <DataTable
          columns={columns}
          data={types}
          getRowId={(ty) => ty.name}
          onRowOpen={open}
          loading={list.isPending}
          label={t("types.label")}
          columnClassName={{
            hierarchical: "hidden md:table-cell",
            archive: "hidden md:table-cell",
          }}
          empty={
            <EmptyState
              icon={BlocksIcon}
              title={t("types.empty.title")}
              body={t("types.empty.body")}
              action={
                <Button onClick={() => setCreating(true)}>
                  <PlusIcon data-icon="inline-start" />
                  {t("types.new")}
                </Button>
              }
            />
          }
        />
      )}
      <NewTypeDialog open={creating} onOpenChange={setCreating} />
    </div>
  );
}

function useTypeColumns() {
  const { t } = useTranslation("structure");
  return useMemo(() => {
    const col = dataTableColumns<ContentTypeDto>();
    return [
      col.accessor("label", {
        header: () => t("types.columns.label"),
        cell: (c) => {
          const ty = c.row.original;
          return (
            <div className="flex min-w-0 flex-col gap-0.5 py-2">
              <span className="flex min-w-0 items-center gap-2">
                <span
                  className="max-w-[calc(100vw-12rem)] truncate font-medium text-foreground sm:max-w-sm"
                  title={ty.label}
                >
                  {ty.label}
                </span>
                {ty.built_in ? <Badge variant="secondary">{t("types.builtIn")}</Badge> : null}
              </span>
              <code
                className="block max-w-[calc(100vw-10rem)] truncate font-mono text-xs text-muted-foreground sm:max-w-sm"
                title={ty.name}
              >
                {ty.name}
              </code>
            </div>
          );
        },
      }),
      col.accessor("hierarchical", {
        id: "hierarchical",
        header: () => t("types.columns.hierarchical"),
        cell: (c) => (
          <span className="text-muted-foreground">
            {c.getValue() ? t("types.hierarchical") : t("types.flat")}
          </span>
        ),
      }),
      col.accessor("has_archive", {
        id: "archive",
        header: () => t("types.columns.archive"),
        cell: (c) =>
          c.getValue() ? (
            <code className="font-mono text-xs">
              {t("types.archiveOn", { name: c.row.original.name })}
            </code>
          ) : (
            <span className="text-muted-foreground">{t("types.archiveOff")}</span>
          ),
      }),
      col.accessor("fields", {
        id: "fields",
        header: () => t("types.columns.fields"),
        cell: (c) => {
          const fields = c.getValue();
          return fields == null ? (
            <span className="text-muted-foreground" title={t("types.noSchemaHint")}>
              {t("types.noSchema")}
            </span>
          ) : (
            <span className="tabular-nums">{t("types.fieldCount", { count: fields.length })}</span>
          );
        },
      }),
    ];
  }, [t]);
}

function NewTypeDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation("structure");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="new-type-dialog" className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("create.title")}</DialogTitle>
          <DialogDescription>{t("create.description")}</DialogDescription>
        </DialogHeader>
        {/* Keyed by opening: every opening starts empty. */}
        {open ? <NewTypeForm onDone={() => onOpenChange(false)} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function NewTypeForm({ onDone }: { onDone: () => void }) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [label, setLabel] = useState("");
  const [name, setName] = useState("");
  const [nameTyped, setNameTyped] = useState(false);
  const [hierarchical, setHierarchical] = useState(false);
  const [hasArchive, setHasArchive] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const labelError =
    serverErrors.label ??
    (attempted ? labelProblem(label, t("create.errors.label", { max: MAX_TYPE_LABEL })) : null);
  const nameError =
    serverErrors.name ??
    ((attempted || name !== "") && !validTypeName(name)
      ? t("create.errors.name", { max: MAX_TYPE_NAME })
      : null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setAttempted(true);
    setFailure(null);
    if (labelProblem(label, "x") !== null || !validTypeName(name)) return;
    setBusy(true);
    try {
      const created = await createType({
        name,
        label: label.trim(),
        hierarchical,
        has_archive: hasArchive,
      });
      await qc.invalidateQueries({ queryKey: qk.types });
      toast.success(t("create.created", { label: created.label }));
      onDone();
      void navigate({ to: "/structure/types/$name", params: { name: created.name } });
    } catch (err) {
      if (err instanceof ApiError) {
        const fields = err.fieldErrors();
        setServerErrors(fields);
        if (!fields.name && !fields.label) setFailure(err.problem.detail || err.problem.title);
      } else {
        setFailure(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex flex-col gap-5">
      <FormField
        label={t("create.label")}
        hint={t("create.labelHint")}
        error={labelError ?? undefined}
      >
        {(control) => (
          <Input
            {...control}
            autoFocus
            value={label}
            autoComplete="off"
            onChange={(e) => {
              const value = e.currentTarget.value;
              setLabel(value);
              setServerErrors((errs) => without(errs, "label"));
              if (!nameTyped) {
                setName(nameFromLabel(value, MAX_TYPE_NAME));
                setServerErrors((errs) => without(errs, "name"));
              }
            }}
          />
        )}
      </FormField>
      <FormField
        label={t("create.name")}
        hint={
          name
            ? t("create.nameHint", { max: MAX_TYPE_NAME, name })
            : t("create.nameHintEmpty", { max: MAX_TYPE_NAME })
        }
        error={nameError ?? undefined}
      >
        {(control) => (
          <Input
            {...control}
            value={name}
            autoComplete="off"
            spellCheck={false}
            className="font-mono"
            onChange={(e) => {
              setName(e.currentTarget.value);
              setNameTyped(true);
              setServerErrors((errs) => without(errs, "name"));
            }}
          />
        )}
      </FormField>
      <SwitchRow label={t("create.hierarchical")} hint={t("create.hierarchicalHint")}>
        {(control) => (
          <Switch {...control} checked={hierarchical} onCheckedChange={setHierarchical} />
        )}
      </SwitchRow>
      <SwitchRow label={t("create.archive")} hint={t("create.archiveHint")}>
        {(control) => <Switch {...control} checked={hasArchive} onCheckedChange={setHasArchive} />}
      </SwitchRow>
      {failure ? (
        <p role="alert" className="text-sm text-destructive">
          {failure}
        </p>
      ) : null}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onDone}>
          {t("create.cancel")}
        </Button>
        <Button type="submit" disabled={busy}>
          {t("create.submit")}
        </Button>
      </DialogFooter>
    </form>
  );
}
