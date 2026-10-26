// Creating and editing a taxonomy (07 §3 Taxonomies; needs `manage_settings`, the screens
// show these only then and the server checks again). A new taxonomy takes a label, a name
// derived from it until typed (core's type name rules; it cannot change), whether its
// terms nest (fixed once created) and the content types it applies to. Editing changes
// the label and the content types through `staleGuard` (someone else's change asks
// Reload / Overwrite); deleting is refused by the server for a built-in taxonomy and for
// one that still has terms, and its reason is shown.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useRouter } from "@tanstack/react-router";
import { CircleAlertIcon, Trash2Icon } from "lucide-react";
import { useId, useMemo, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import {
  type TaxonomyDto,
  createTaxonomy,
  deleteTaxonomy,
  getTaxonomy,
  patchTaxonomy,
} from "@/api/taxonomies";
import { type ContentTypeDto, typesQuery } from "@/api/types";
import { useStaleAsk } from "@/components/kit/use-stale-ask";
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
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { staleGuard } from "@/lib/stale-guard";
import { nameFromLabel } from "../types/builder-model";
import { FormField, SwitchRow } from "../types/form-field";
import { MAX_TYPE_LABEL, MAX_TYPE_NAME, labelProblem, validTypeName } from "../types/type-rules";
import { problemText } from "./terms-model";
import { DESTRUCTIVE } from "@/lib/destructive";
import { without } from "../shared";

/** Core keeps at most this many content types per taxonomy (`check_content_types`). */
export const MAX_TAXONOMY_TYPES = 20;
/** Listed elsewhere (the media library); kept when stored, never offered. */
const NOT_OFFERED = new Set(["media"]);
const BUILT_IN_ORDER = ["post", "page"];

/** The types a taxonomy can apply to: built-ins first (post, page), then by label. */
function offeredTypes(types: readonly ContentTypeDto[]): ContentTypeDto[] {
  const rank = (ty: ContentTypeDto) => {
    const i = BUILT_IN_ORDER.indexOf(ty.name);
    return i >= 0 ? i : ty.built_in ? BUILT_IN_ORDER.length : BUILT_IN_ORDER.length + 1;
  };
  return types
    .filter((ty) => !NOT_OFFERED.has(ty.name))
    .sort((a, b) => rank(a) - rank(b) || a.label.localeCompare(b.label));
}

/** The content types as checkboxes; `value` keeps entries that are not offered. */
function ContentTypesField({
  value,
  onChange,
  disabled,
  error,
}: {
  value: string[];
  onChange: (next: string[]) => void;
  disabled: boolean;
  error?: string | undefined;
}) {
  const { t } = useTranslation("structure");
  const id = useId();
  const types = useQuery(typesQuery);
  const offered = useMemo(() => offeredTypes(types.data ?? []), [types.data]);
  const toggle = (name: string, on: boolean) =>
    onChange(on ? [...value.filter((v) => v !== name), name] : value.filter((v) => v !== name));
  return (
    <fieldset
      data-slot="taxonomy-types"
      className="flex min-w-0 flex-col gap-2"
      aria-describedby={`${id}-hint${error ? ` ${id}-error` : ""}`}
    >
      <legend className="mb-1.5 text-sm font-medium">{t("taxonomies.form.types")}</legend>
      {types.isPending ? (
        <div className="flex flex-col gap-2">
          <Skeleton className="h-5 w-40" />
          <Skeleton className="h-5 w-32" />
        </div>
      ) : (
        <ul className="grid gap-x-4 gap-y-1 sm:grid-cols-2">
          {offered.map((ty) => {
            const boxId = `${id}-${ty.name}`;
            return (
              <li key={ty.name} className="flex min-h-11 items-center gap-2.5 sm:min-h-9">
                <Checkbox
                  id={boxId}
                  checked={value.includes(ty.name)}
                  disabled={disabled}
                  onCheckedChange={(on) => toggle(ty.name, on === true)}
                />
                <Label htmlFor={boxId} className="min-w-0 font-normal">
                  <span className="truncate" title={ty.label}>
                    {ty.label}
                  </span>
                  <code
                    className="truncate font-mono text-xs text-muted-foreground"
                    title={ty.name}
                  >
                    {ty.name}
                  </code>
                </Label>
              </li>
            );
          })}
        </ul>
      )}
      <p id={`${id}-hint`} className="text-xs text-muted-foreground">
        {t("taxonomies.form.typesHint", { max: MAX_TAXONOMY_TYPES })}
      </p>
      {error ? (
        <p id={`${id}-error`} data-slot="form-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </fieldset>
  );
}

function Failure({ message }: { message: string }) {
  return (
    <p
      role="alert"
      className="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/8 px-3 py-2 text-sm text-foreground"
    >
      <CircleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-destructive" />
      {message}
    </p>
  );
}

export function NewTaxonomyDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation("structure");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="new-taxonomy-dialog" className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("taxonomies.create.title")}</DialogTitle>
          <DialogDescription>{t("taxonomies.create.description")}</DialogDescription>
        </DialogHeader>
        {/* Mounted per opening: every opening starts empty. */}
        {open ? <NewTaxonomyForm onDone={() => onOpenChange(false)} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function NewTaxonomyForm({ onDone }: { onDone: () => void }) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [label, setLabel] = useState("");
  const [name, setName] = useState("");
  const [nameTyped, setNameTyped] = useState(false);
  const [hierarchical, setHierarchical] = useState(false);
  const [types, setTypes] = useState<string[]>([]);
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
      const created = await createTaxonomy({
        name,
        label: label.trim(),
        hierarchical,
        content_types: types,
      });
      await qc.invalidateQueries({ queryKey: qk.taxonomies });
      toast.success(t("taxonomies.create.created", { label: created.label }));
      onDone();
      void navigate({ to: "/structure/taxonomies/$name", params: { name: created.name } });
    } catch (err) {
      reportApiError(err);
      const fields = err instanceof ApiError ? err.fieldErrors() : {};
      setServerErrors(fields);
      if (!fields.name && !fields.label && !fields.content_types) setFailure(problemText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex flex-col gap-5">
      <FormField
        label={t("taxonomies.form.label")}
        hint={t("taxonomies.form.labelHint")}
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
        label={t("taxonomies.form.name")}
        hint={
          name
            ? t("taxonomies.form.nameHint", { max: MAX_TYPE_NAME, name })
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
      <SwitchRow
        label={t("taxonomies.form.hierarchical")}
        hint={t("taxonomies.form.hierarchicalHint")}
      >
        {(control) => (
          <Switch {...control} checked={hierarchical} onCheckedChange={setHierarchical} />
        )}
      </SwitchRow>
      <ContentTypesField
        value={types}
        onChange={(next) => {
          setTypes(next);
          setServerErrors((errs) => without(errs, "content_types"));
        }}
        disabled={busy}
        error={serverErrors.content_types}
      />
      {failure ? <Failure message={failure} /> : null}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onDone}>
          {t("create.cancel")}
        </Button>
        <Button type="submit" disabled={busy}>
          {t("taxonomies.create.submit")}
        </Button>
      </DialogFooter>
    </form>
  );
}

/** The server's reason for refusing a delete, in the screen's words when it is a known one. */
function refusal(
  error: unknown,
  t: (key: "taxonomies.delete.builtIn" | "taxonomies.delete.hasTerms") => string,
) {
  if (error instanceof ApiError) {
    if (error.problem.detail === "built-in taxonomy") return t("taxonomies.delete.builtIn");
    if (error.problem.detail === "taxonomy has terms") return t("taxonomies.delete.hasTerms");
  }
  return problemText(error);
}

export function TaxonomySettingsDialog({
  open,
  onOpenChange,
  taxonomy,
  termCount,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  taxonomy: TaxonomyDto;
  /** How many terms it has, when known (deleting needs none). */
  termCount: number | undefined;
}) {
  const { t } = useTranslation("structure");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="taxonomy-settings-dialog" className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("taxonomies.settings.title", { label: taxonomy.label })}</DialogTitle>
          <DialogDescription>{t("taxonomies.settings.description")}</DialogDescription>
        </DialogHeader>
        {open ? (
          <TaxonomySettingsForm
            taxonomy={taxonomy}
            termCount={termCount}
            onDone={() => onOpenChange(false)}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function TaxonomySettingsForm({
  taxonomy,
  termCount,
  onDone,
}: {
  taxonomy: TaxonomyDto;
  termCount: number | undefined;
  onDone: () => void;
}) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const router = useRouter();
  const navigate = useNavigate();
  const [base, setBase] = useState(taxonomy);
  const [label, setLabel] = useState(taxonomy.label);
  const [types, setTypes] = useState<string[]>(taxonomy.content_types);
  const [attempted, setAttempted] = useState(false);
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const stale = useStaleAsk(base.label);

  const labelError =
    serverErrors.label ??
    (attempted ? labelProblem(label, t("create.errors.label", { max: MAX_TYPE_LABEL })) : null);
  const sameTypes =
    types.length === base.content_types.length &&
    types.every((ty) => base.content_types.includes(ty));
  const dirty = label !== base.label || !sameTypes;

  const reset = (dto: TaxonomyDto) => {
    setBase(dto);
    setLabel(dto.label);
    setTypes(dto.content_types);
    setAttempted(false);
    setServerErrors({});
    setFailure(null);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setAttempted(true);
    setFailure(null);
    if (labelProblem(label, "x") !== null) return;
    if (!dirty) {
      onDone();
      return;
    }
    setBusy(true);
    try {
      const guard = await staleGuard({
        reread: () => getTaxonomy(base.name),
        base,
        ask: stale.ask,
      });
      if (!guard.proceed) {
        if (guard.reloaded) {
          reset(guard.reloaded);
          toast.info(t("form.reloaded"));
        }
        return;
      }
      const saved = await patchTaxonomy(base.name, { label: label.trim(), content_types: types });
      void qc.invalidateQueries({ queryKey: qk.taxonomies });
      // The screen's header, the crumb and the document title follow. Awaited: the
      // dialog's next opening starts from the saved taxonomy (no false stale prompt).
      await router.invalidate();
      toast.success(t("form.savedToast", { label: saved.label }));
      onDone();
    } catch (err) {
      reportApiError(err);
      const fields = err instanceof ApiError ? err.fieldErrors() : {};
      setServerErrors(fields);
      if (!fields.label && !fields.content_types) setFailure(problemText(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setConfirmDelete(false);
    setBusy(true);
    setDeleteError(null);
    try {
      await deleteTaxonomy(base.name);
      void qc.invalidateQueries({ queryKey: qk.taxonomies });
      toast.success(t("taxonomies.delete.deleted", { label: base.label }));
      onDone();
      void navigate({ to: "/structure/taxonomies" });
    } catch (err) {
      reportApiError(err);
      setDeleteError(refusal(err, t));
    } finally {
      setBusy(false);
    }
  };

  const deleteBlocked = base.built_in
    ? t("taxonomies.delete.builtIn")
    : termCount !== undefined && termCount > 0
      ? t("taxonomies.delete.termsLeft", { count: termCount })
      : null;

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex flex-col gap-5">
      <FormField label={t("taxonomies.form.label")} error={labelError ?? undefined}>
        {(control) => (
          <Input
            {...control}
            value={label}
            autoComplete="off"
            disabled={busy}
            onChange={(e) => {
              setLabel(e.currentTarget.value);
              setServerErrors((errs) => without(errs, "label"));
            }}
          />
        )}
      </FormField>
      <div className="grid gap-4 sm:grid-cols-2">
        <div className="flex min-w-0 flex-col gap-1">
          <span className="text-sm font-medium">{t("taxonomies.form.name")}</span>
          <code className="font-mono text-sm break-all">{base.name}</code>
          <p className="text-xs text-muted-foreground">{t("taxonomies.form.nameFixed")}</p>
        </div>
        <div className="flex min-w-0 flex-col gap-1">
          <span className="text-sm font-medium">{t("taxonomies.form.structure")}</span>
          <span className="text-sm">
            {base.hierarchical ? t("taxonomies.nested") : t("taxonomies.flat")}
          </span>
          <p className="text-xs text-muted-foreground">{t("taxonomies.form.structureFixed")}</p>
        </div>
      </div>
      <ContentTypesField
        value={types}
        onChange={(next) => {
          setTypes(next);
          setServerErrors((errs) => without(errs, "content_types"));
        }}
        disabled={busy}
        error={serverErrors.content_types}
      />
      {failure ? <Failure message={failure} /> : null}
      {deleteError ? (
        <div data-slot="taxonomy-delete-refused">
          <Failure message={t("taxonomies.delete.refused", { reason: deleteError })} />
        </div>
      ) : null}
      <DialogFooter className="sm:justify-between">
        <Button
          type="button"
          variant="outline"
          disabled={busy}
          onClick={() => setConfirmDelete(true)}
          className="text-destructive hover:border-destructive/50 hover:text-destructive"
        >
          <Trash2Icon data-icon="inline-start" />
          {t("taxonomies.delete.button")}
        </Button>
        <div className="flex flex-col-reverse gap-2 sm:flex-row">
          <Button type="button" variant="outline" onClick={onDone}>
            {t("create.cancel")}
          </Button>
          <Button type="submit" disabled={busy}>
            {busy ? t("form.saving") : t("form.save")}
          </Button>
        </div>
      </DialogFooter>
      {stale.dialog}
      <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialogContent data-slot="delete-taxonomy-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("taxonomies.delete.confirmTitle", { label: base.label })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {deleteBlocked ?? t("taxonomies.delete.confirmBody")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              {deleteBlocked ? t("taxonomies.delete.close") : t("delete.cancel")}
            </AlertDialogCancel>
            {deleteBlocked ? null : (
              <Button variant="destructive" className={DESTRUCTIVE} onClick={() => void remove()}>
                {t("taxonomies.delete.confirm")}
              </Button>
            )}
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </form>
  );
}
