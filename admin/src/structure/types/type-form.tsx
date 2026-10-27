// A content type's screen (07 §3 Content types): its settings (label, archive; name and
// hierarchy are fixed once created), the field builder and deleting the type. Saving
// checks the builder against core's schema rules first, then names what the change does
// to stored items (`describeChange`, Review Focus 2; each kind of change says what it does)
// and asks, then re-reads the type (`staleGuard`, Review Focus 4: someone else's change
// asks Reload / Overwrite) and sends one `PATCH /types/{name}` with the whole `fields` list
// when it changed. A relation's target types are checked against the registered types
// here; the server's refusal of them is shown at that field. The server decides what a
// built-in type allows and whether a type can be deleted; its reason is shown (a built-in
// type's delete says it cannot be done instead of offering it). Leaving with unsaved
// changes asks first.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useRouter } from "@tanstack/react-router";
import { CircleAlertIcon, Trash2Icon } from "lucide-react";
import { useCallback, useMemo, useRef, useState } from "react";
import { type TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type ContentTypeDto, deleteType, getType, patchType, typesQuery } from "@/api/types";
import { PageHeader } from "@/components/kit/page-header";
import { SaveState, type SaveStateValue } from "@/components/kit/save-state";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { useLeaveGuard } from "@/editor/leave-guard";
import { useShortcut } from "@/lib/shortcuts";
import { stableEqual, staleGuard } from "@/lib/stale-guard";
import { BuilderContext, type BuilderContextValue } from "./builder-context";
import {
  type BuilderRow,
  checkBuilder,
  fromSchema,
  markSaved,
  pathToFirstError,
  serverFieldError,
  toSchema,
} from "./builder-model";
import { FieldBuilder } from "./field-builder";
import { FormField, SwitchRow } from "@/components/kit/form-field";
import { type SchemaChange, describeChange, isEmptyChange } from "./schema-change";
import { MAX_TYPE_LABEL, labelProblem } from "./type-rules";
import { DESTRUCTIVE } from "@/lib/destructive";
import { uiLocale } from "@/lib/locale";

const MODAL = '[data-slot="dialog-overlay"], [data-slot="alert-dialog-overlay"]';

/** The server's reason for refusing a delete, in the screen's words when it is a known one. */
function refusal(error: unknown, t: TFunction<"structure">): string {
  if (!(error instanceof ApiError)) return error instanceof Error ? error.message : String(error);
  const detail = error.problem.detail ?? "";
  if (detail === "built-in type") return t("delete.reasons.builtIn");
  if (detail === "type has content") return t("delete.reasons.hasContent");
  // `types.rs`: another type's relation field lists this one (P52).
  const target = /^type is a relation target of type "(.+)"$/.exec(detail);
  if (target?.[1]) return t("delete.reasons.relationTarget", { type: target[1] });
  return detail || error.problem.title;
}

export function TypeForm({ initial }: { initial: ContentTypeDto }) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const navigate = useNavigate();
  const router = useRouter();
  const [base, setBase] = useState(initial);
  const [label, setLabel] = useState(initial.label);
  const [hasArchive, setHasArchive] = useState(initial.has_archive);
  const [rows, setRowsState] = useState<BuilderRow[]>(() => fromSchema(initial.fields));
  // The server's refusal of a relation's types, at that field, until the rows change.
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const setRows = useCallback((next: BuilderRow[] | ((rows: BuilderRow[]) => BuilderRow[])) => {
    setRowsState(next);
    setServerErrors({});
  }, []);
  const allTypes = useQuery(typesQuery);
  const registered = useMemo(() => allTypes.data?.map((ty) => ty.name), [allTypes.data]);
  const [attempted, setAttempted] = useState(false);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<Date | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [labelServerError, setLabelServerError] = useState<string | null>(null);
  const [openKeys, setOpenKeys] = useState<ReadonlySet<string>>(() => new Set());
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const [pendingChange, setPendingChange] = useState<SchemaChange | null>(null);
  const changeAnswer = useRef<((ok: boolean) => void) | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const labelInput = useRef<HTMLInputElement>(null);
  const stale = useStaleAsk(base.label);

  const baseSchema = useMemo(() => toSchema(fromSchema(base.fields)), [base.fields]);
  const schema = useMemo(() => toSchema(rows), [rows]);
  const fieldsChanged = !stableEqual(schema, baseSchema);
  const dirty = label !== base.label || hasArchive !== base.has_archive || fieldsChanged;

  const allErrors = useMemo(() => checkBuilder(rows, registered), [rows, registered]);
  // Before the first save attempt only a name typed wrong shows (feedback while typing);
  // after it, every problem. The server's own always shows.
  const errors = useMemo(() => {
    if (attempted) return { ...serverErrors, ...allErrors };
    const empty = t("errors.nameEmpty");
    return {
      ...serverErrors,
      ...Object.fromEntries(
        Object.entries(allErrors).filter(([k, m]) => k.endsWith(":name") && m !== empty),
      ),
    };
  }, [allErrors, serverErrors, attempted, t]);
  const labelError =
    labelServerError ??
    (attempted ? labelProblem(label, t("create.errors.label", { max: MAX_TYPE_LABEL })) : null);

  const reset = (dto: ContentTypeDto) => {
    setBase(dto);
    setLabel(dto.label);
    setHasArchive(dto.has_archive);
    setRows(fromSchema(dto.fields));
    setOpenKeys(new Set());
    setAttempted(false);
    setFailure(null);
    setLabelServerError(null);
  };

  const askChange = (change: SchemaChange) =>
    new Promise<boolean>((resolve) => {
      changeAnswer.current = resolve;
      setPendingChange(change);
    });
  const answerChange = (ok: boolean) => {
    setPendingChange(null);
    changeAnswer.current?.(ok);
    changeAnswer.current = null;
  };

  const showFirstError = (errs: Record<string, string>, labelBad: boolean) => {
    if (labelBad) {
      requestAnimationFrame(() => labelInput.current?.focus());
      return;
    }
    const path = pathToFirstError(rows, errs);
    setOpenKeys((s) => new Set([...s, ...path]));
    // Once the rows on the way are open: the first invalid input takes the cursor; a
    // level's own problem (no sub-fields, too many) has no input and is scrolled to.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const scope = "[data-slot=type-fields]";
        const input = document.querySelector<HTMLElement>(`${scope} [aria-invalid=true]`);
        if (input) input.focus();
        else
          document
            .querySelector(`${scope} [data-slot=form-error]`)
            ?.scrollIntoView({ block: "center" });
      }),
    );
  };

  const save = async () => {
    if (saving || deleting) return;
    setAttempted(true);
    setFailure(null);
    setLabelServerError(null);
    const labelBad = labelProblem(label, "x") !== null;
    if (Object.keys(allErrors).length > 0 || labelBad) {
      showFirstError(allErrors, labelBad);
      toast.error(t("form.invalid"));
      return;
    }
    if (!dirty) return;
    if (fieldsChanged) {
      const change = describeChange(baseSchema, schema);
      if (!isEmptyChange(change) && !(await askChange(change))) return;
    }
    setSaving(true);
    try {
      const guard = await staleGuard({ reread: () => getType(base.name), base, ask: stale.ask });
      if (!guard.proceed) {
        if (guard.reloaded) {
          reset(guard.reloaded);
          toast.info(t("form.reloaded"));
        }
        return;
      }
      const saved = await patchType(base.name, {
        label: label.trim(),
        has_archive: hasArchive,
        ...(fieldsChanged ? { fields: schema } : {}),
      });
      setBase(saved);
      setLabel(saved.label);
      setHasArchive(saved.has_archive);
      setRows((r) => markSaved(r));
      setAttempted(false);
      setSavedAt(new Date());
      void qc.invalidateQueries({ queryKey: qk.types });
      // The crumb and the document title follow a new label.
      void router.invalidate();
      toast.success(t("form.savedToast", { label: saved.label }));
    } catch (e) {
      reportApiError(e);
      const message = e instanceof ApiError ? e.problem.detail || e.problem.title : null;
      const fieldErrors = e instanceof ApiError ? e.fieldErrors() : {};
      const atField = fieldErrors.fields ? serverFieldError(rows, fieldErrors.fields) : null;
      if (fieldErrors.label) setLabelServerError(fieldErrors.label);
      else if (atField) {
        const errs = { [atField.key]: atField.message };
        setServerErrors(errs);
        showFirstError(errs, false);
      } else setFailure(e instanceof ApiError && e.status === 404 ? t("form.notFound") : message);
      toast.error(t("form.notSaved"), message ? { description: message } : {});
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    setConfirmDelete(false);
    setDeleting(true);
    setDeleteError(null);
    try {
      await deleteType(base.name);
      void qc.invalidateQueries({ queryKey: qk.types });
      toast.success(t("delete.deleted", { label: base.label }));
      void navigate({ to: "/structure/types", ignoreBlocker: true });
    } catch (e) {
      reportApiError(e);
      setDeleteError(refusal(e, t));
    } finally {
      setDeleting(false);
    }
  };

  const leave = useLeaveGuard(dirty && !deleting);
  useShortcut({
    id: "types.save",
    keys: "mod+s",
    label: t("shortcuts.save"),
    group: t("shortcuts.group"),
    allowInInput: true,
    run: () => {
      if (document.querySelector(MODAL) !== null) return;
      void save();
    },
  });

  const setOpen = useCallback((key: string, open: boolean) => {
    setOpenKeys((s) => {
      const next = new Set(s);
      if (open) next.add(key);
      else next.delete(key);
      return next;
    });
  }, []);
  const builder: BuilderContextValue = useMemo(
    () => ({
      errors,
      isOpen: (key) => openKeys.has(key),
      setOpen,
      focusKey,
      setFocusKey,
      disabled: saving,
    }),
    [errors, openKeys, setOpen, focusKey, saving],
  );

  const state: SaveStateValue | null = saving
    ? "saving"
    : failure !== null || labelServerError !== null
      ? "error"
      : dirty
        ? "dirty"
        : savedAt
          ? "saved"
          : null;

  return (
    <div data-slot="type-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-6">
      <PageHeader
        title={base.label}
        description={
          <span className="flex flex-wrap items-center gap-2">
            <code className="font-mono text-sm text-muted-foreground">{base.name}</code>
            {base.built_in ? <Badge variant="secondary">{t("types.builtIn")}</Badge> : null}
          </span>
        }
        actions={
          <>
            {state ? <SaveState state={state} {...(savedAt ? { at: savedAt } : {})} /> : null}
            <Button onClick={() => void save()} disabled={!dirty || saving || deleting}>
              {saving ? t("form.saving") : t("form.save")}
            </Button>
          </>
        }
      />

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:grid-rows-[auto_1fr]">
        {/* The builder first in the markup: phones and the keyboard reach it first; the
            grid places it on the left on wide screens. */}
        <Card
          data-slot="type-fields"
          className="lg:col-start-1 lg:row-span-2 lg:row-start-1 lg:self-start"
        >
          <CardHeader>
            <CardTitle role="heading" aria-level={2} className="text-md">
              {t("builder.title")}
            </CardTitle>
            <CardDescription>{t("builder.description")}</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            {base.fields == null && rows.length === 0 ? (
              <p className="rounded-lg bg-muted px-3 py-2 text-sm text-muted-foreground">
                {t("builder.noSchema")}
              </p>
            ) : null}
            {failure ? (
              <p
                role="alert"
                data-slot="save-failure"
                className="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/8 px-3 py-2 text-sm text-foreground"
              >
                <CircleAlertIcon
                  aria-hidden="true"
                  className="mt-0.5 size-4 shrink-0 text-destructive"
                />
                {failure}
              </p>
            ) : null}
            <BuilderContext value={builder}>
              <FieldBuilder rows={rows} onChange={setRows} depth={0} parent={null} />
            </BuilderContext>
          </CardContent>
        </Card>

        <Card data-slot="type-settings" className="lg:col-start-2 lg:row-start-1">
          <CardHeader>
            <CardTitle role="heading" aria-level={2} className="text-md">
              {t("form.settings")}
            </CardTitle>
            <CardDescription>{t("form.settingsDescription")}</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-5">
            <FormField label={t("form.label")} error={labelError ?? undefined}>
              {(control) => (
                <Input
                  {...control}
                  ref={labelInput}
                  value={label}
                  maxLength={MAX_TYPE_LABEL + 20}
                  disabled={saving}
                  onChange={(e) => {
                    setLabel(e.currentTarget.value);
                    setLabelServerError(null);
                  }}
                />
              )}
            </FormField>
            <div className="flex flex-col gap-1">
              <span className="text-sm font-medium">{t("form.name")}</span>
              <code className="font-mono text-sm break-all">{base.name}</code>
              <p className="text-xs text-muted-foreground">{t("form.nameFixed")}</p>
            </div>
            <SwitchRow label={t("form.hierarchical")} hint={t("form.hierarchicalFixed")}>
              {(control) => <Switch {...control} checked={base.hierarchical} disabled />}
            </SwitchRow>
            <SwitchRow label={t("form.archive")} hint={t("form.archiveHint", { name: base.name })}>
              {(control) => (
                <Switch
                  {...control}
                  checked={hasArchive}
                  disabled={saving}
                  onCheckedChange={setHasArchive}
                />
              )}
            </SwitchRow>
            {base.built_in ? (
              <p className="rounded-lg bg-muted px-3 py-2 text-xs text-muted-foreground">
                {t("form.builtIn")}
              </p>
            ) : null}
          </CardContent>
        </Card>

        <Card data-slot="type-danger" className="lg:col-start-2 lg:row-start-2 lg:self-start">
          <CardHeader>
            <CardTitle role="heading" aria-level={2} className="text-md">
              {t("delete.title")}
            </CardTitle>
            <CardDescription>
              {base.built_in ? t("delete.builtIn") : t("delete.body")}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col items-start gap-3">
            {deleteError ? (
              <p
                role="alert"
                data-slot="delete-refused"
                className="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/8 px-3 py-2 text-sm text-foreground"
              >
                <CircleAlertIcon
                  aria-hidden="true"
                  className="mt-0.5 size-4 shrink-0 text-destructive"
                />
                <span>
                  <span className="font-medium">{t("delete.refused", { label: base.label })}</span>{" "}
                  {deleteError}
                </span>
              </p>
            ) : null}
            <Button
              variant="outline"
              disabled={deleting || saving}
              onClick={() => setConfirmDelete(true)}
              className="text-destructive hover:border-destructive/50 hover:text-destructive"
            >
              <Trash2Icon data-icon="inline-start" />
              {t("delete.button")}
            </Button>
          </CardContent>
        </Card>
      </div>

      <ChangeDialog change={pendingChange} onAnswer={answerChange} />
      {stale.dialog}
      <AlertDialog open={confirmDelete} onOpenChange={(o) => setConfirmDelete(o)}>
        <AlertDialogContent data-slot="delete-type-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("delete.confirmTitle", { label: base.label })}</AlertDialogTitle>
            <AlertDialogDescription>
              {base.built_in ? t("delete.builtIn") : t("delete.confirmBody", { name: base.name })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              {base.built_in ? t("delete.close") : t("delete.cancel")}
            </AlertDialogCancel>
            {base.built_in ? null : (
              <Button variant="destructive" className={DESTRUCTIVE} onClick={() => void remove()}>
                {t("delete.confirm")}
              </Button>
            )}
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog open={leave.blocked} onOpenChange={(o) => (o ? undefined : leave.stay())}>
        <AlertDialogContent data-slot="type-leave-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("form.leave.title")}</AlertDialogTitle>
            <AlertDialogDescription>{t("form.leave.body")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("form.leave.stay")}</AlertDialogCancel>
            <Button variant="destructive" className={DESTRUCTIVE} onClick={leave.leave}>
              {t("form.leave.leave")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

/** What the schema change does to stored items, before the save (Review Focus 2). */
function ChangeDialog({
  change,
  onAnswer,
}: {
  change: SchemaChange | null;
  onAnswer: (ok: boolean) => void;
}) {
  const { t } = useTranslation("structure");
  const list = new Intl.ListFormat(uiLocale(), { type: "conjunction" });
  // Each kind of change with what it does to stored items, in its own words.
  const groups: { key: keyof SchemaChange; title: string; hint: string; items: string[] }[] = change
    ? [
        {
          key: "removed" as const,
          title: t("change.removed"),
          hint: t("change.removedHint"),
          items: change.removed,
        },
        {
          key: "renamed" as const,
          title: t("change.renamed"),
          hint: t("change.renamedHint"),
          items: change.renamed.map(([from, to]) => t("change.renamedPair", { from, to })),
        },
        {
          key: "newlyRequired" as const,
          title: t("change.newlyRequired"),
          hint: t("change.newlyRequiredHint"),
          items: change.newlyRequired,
        },
        {
          key: "kindChanged" as const,
          title: t("change.kindChanged"),
          hint: t("change.kindChangedHint"),
          items: change.kindChanged,
        },
        {
          key: "optionsRemoved" as const,
          title: t("change.optionsRemoved"),
          hint: t("change.optionsRemovedHint"),
          items: change.optionsRemoved.map(([field, options]) =>
            t("change.optionsRemovedItem", { field, options: list.format(options) }),
          ),
        },
        {
          key: "targetsNarrowed" as const,
          title: t("change.targetsNarrowed"),
          hint: t("change.targetsNarrowedHint"),
          items: change.targetsNarrowed,
        },
      ].filter((g) => g.items.length > 0)
    : [];
  return (
    <AlertDialog
      open={change !== null}
      onOpenChange={(o) => {
        if (!o) onAnswer(false);
      }}
    >
      <AlertDialogContent data-slot="schema-change-dialog" className="sm:max-w-lg">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("change.title")}</AlertDialogTitle>
          <AlertDialogDescription>{t("change.body")}</AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex max-h-[50vh] flex-col gap-4 overflow-y-auto text-sm">
          {groups.map((g) => (
            <section key={g.key} data-change={g.key} className="flex flex-col gap-1.5">
              <h3 className="font-medium text-foreground">{g.title}</h3>
              <p className="text-muted-foreground">{g.hint}</p>
              <ul className="flex flex-wrap gap-1.5">
                {g.items.map((item) => (
                  <li key={item} className="max-w-full">
                    {/* A block of its own: an inline chip's padding and border stood out of
                        its line box, and the scroller clipped the last row's. */}
                    <code className="inline-block max-w-full rounded-md border bg-muted px-1.5 py-0.5 font-mono text-xs break-all">
                      {item}
                    </code>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("change.cancel")}</AlertDialogCancel>
          <Button onClick={() => onAnswer(true)}>{t("change.confirm")}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
