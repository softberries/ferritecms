// A term's sheet (07 §3 Taxonomies; needs `manage_terms`, the server checks again): name,
// slug (empty: the server derives it from the name, and a slug in use gets -2, -3, …),
// description and, in a hierarchical taxonomy, the parent. Saving an existing term
// re-reads it first (`staleGuard`, Review Focus 4). Also here: the parent combobox the
// sheet and the tree's "Move to…" share, and the delete confirm, worded from core
// (`taxonomy.rs` `delete_term`): a term with sub-terms is refused, so the confirm says
// how many and offers no delete; otherwise the term goes and the items filed under it
// lose it. Term deletes confirm instead of offering Undo (P35).

import { useQueryClient } from "@tanstack/react-query";
import { ChevronsUpDownIcon } from "lucide-react";
import { useId, useMemo, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "cn";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type TaxonomyDto, type TermDto, createTerm } from "@/api/taxonomies";
import { deleteTerm, getTerm, patchTerm } from "@/api/terms";
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
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { staleGuard } from "@/lib/stale-guard";
import { FormField } from "../types/form-field";
import { returnToRowMenu } from "./row-focus";
import { childCount, parentChoices, problemText } from "./terms-model";
import { DESTRUCTIVE, without } from "../shared";

const TOP = "__top";

export type TermSheetState =
  { kind: "create"; parentId: string | null } | { kind: "edit"; term: TermDto } | null;

/** A term's name with its ancestors, for labels (`Fruit › Apples`). */
const pathLabel = (path: string[]) => path.join(" › ");

/**
 * The parent combobox: "Top level" and every term `termId` may move under (never itself
 * or a term below it), in tree order and indented; typing filters by name.
 */
export function TermParentPicker({
  terms,
  termId,
  value,
  onChange,
  disabled = false,
  error,
}: {
  terms: readonly TermDto[];
  /** The term being moved (`null` for a new one). */
  termId: string | null;
  value: string | null;
  onChange: (parentId: string | null) => void;
  disabled?: boolean;
  error?: string | undefined;
}) {
  const { t } = useTranslation("structure");
  const id = useId();
  const [open, setOpen] = useState(false);
  const choices = useMemo(() => parentChoices(terms, termId), [terms, termId]);
  const current = choices.find((c) => c.term.id === value);
  const shown =
    value === null
      ? t("taxonomies.terms.topLevel")
      : current
        ? pathLabel(current.path)
        : t("taxonomies.terms.unknownParent");
  return (
    <div data-slot="term-parent" className="flex min-w-0 flex-col gap-1.5">
      <Label id={`${id}-label`}>{t("taxonomies.terms.parent")}</Label>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="outline"
            disabled={disabled}
            aria-labelledby={`${id}-label ${id}-value`}
            aria-describedby={error ? `${id}-error` : `${id}-hint`}
            {...(error ? { "aria-invalid": true } : {})}
            className="justify-between font-normal"
          >
            <span id={`${id}-value`} className="truncate" title={shown}>
              {shown}
            </span>
            <ChevronsUpDownIcon data-icon="inline-end" className="text-muted-foreground" />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-(--radix-popover-trigger-width) min-w-64 p-0">
          <Command>
            <CommandInput placeholder={t("taxonomies.terms.parentSearch")} />
            <CommandList>
              <CommandEmpty>{t("taxonomies.terms.parentEmpty")}</CommandEmpty>
              <CommandGroup>
                <CommandItem
                  value={TOP}
                  data-checked={value === null}
                  keywords={[t("taxonomies.terms.topLevel")]}
                  onSelect={() => {
                    onChange(null);
                    setOpen(false);
                  }}
                >
                  <span className="text-muted-foreground">{t("taxonomies.terms.topLevel")}</span>
                </CommandItem>
                {choices.map((c) => (
                  <CommandItem
                    key={c.term.id}
                    value={c.term.id}
                    data-checked={c.term.id === value}
                    keywords={[c.term.name, c.term.slug]}
                    onSelect={() => {
                      onChange(c.term.id);
                      setOpen(false);
                    }}
                    title={pathLabel(c.path)}
                  >
                    <span
                      className="truncate"
                      style={{ paddingInlineStart: `${Math.min(c.depth, 6) * 12}px` }}
                    >
                      {c.term.name}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {error ? (
        <p id={`${id}-error`} data-slot="form-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : (
        <p id={`${id}-hint`} className="text-xs text-muted-foreground">
          {t("taxonomies.terms.parentHint")}
        </p>
      )}
    </div>
  );
}

export function TermSheet({
  taxonomy,
  terms,
  state,
  origin = null,
  onClose,
}: {
  taxonomy: TaxonomyDto;
  terms: readonly TermDto[];
  state: TermSheetState;
  /** The tree row whose menu opened it: focus goes back to that menu on close. */
  origin?: string | null;
  onClose: () => void;
}) {
  const { t } = useTranslation("structure");
  const editing = state?.kind === "edit" ? state.term : null;
  return (
    <Sheet open={state !== null} onOpenChange={(o) => (o ? undefined : onClose())}>
      <SheetContent
        side="right"
        data-slot="term-sheet"
        onCloseAutoFocus={returnToRowMenu(origin)}
        className="w-full gap-0 overflow-y-auto p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-md"
      >
        <SheetHeader className="gap-1 border-b p-5 pr-12">
          <SheetTitle className="truncate text-lg" title={editing?.name}>
            {editing
              ? t("taxonomies.terms.editTitle", { name: editing.name })
              : t("taxonomies.terms.newTitle", { label: taxonomy.label })}
          </SheetTitle>
          <SheetDescription>
            {editing ? t("taxonomies.terms.editDescription") : t("taxonomies.terms.newDescription")}
          </SheetDescription>
        </SheetHeader>
        {state ? (
          <TermFormBody
            key={state.kind === "edit" ? state.term.id : `new:${state.parentId ?? ""}`}
            taxonomy={taxonomy}
            terms={terms}
            state={state}
            onClose={onClose}
          />
        ) : null}
      </SheetContent>
    </Sheet>
  );
}

function TermFormBody({
  taxonomy,
  terms,
  state,
  onClose,
}: {
  taxonomy: TaxonomyDto;
  terms: readonly TermDto[];
  state: NonNullable<TermSheetState>;
  onClose: () => void;
}) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const [base, setBase] = useState<TermDto | null>(state.kind === "edit" ? state.term : null);
  const [name, setName] = useState(base?.name ?? "");
  const [slug, setSlug] = useState(base?.slug ?? "");
  const [description, setDescription] = useState(base?.description ?? "");
  const [parentId, setParentId] = useState<string | null>(
    state.kind === "edit" ? (state.term.parent_id ?? null) : state.parentId,
  );
  const [attempted, setAttempted] = useState(false);
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const stale = useStaleAsk(base?.name ?? "");
  const key = qk.terms(taxonomy.name);

  const nameError =
    serverErrors.name ?? (attempted && name.trim() === "" ? t("taxonomies.terms.nameEmpty") : null);

  const reset = (dto: TermDto) => {
    setBase(dto);
    setName(dto.name);
    setSlug(dto.slug);
    setDescription(dto.description);
    setParentId(dto.parent_id ?? null);
    setAttempted(false);
    setServerErrors({});
    setFailure(null);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setAttempted(true);
    setFailure(null);
    if (name.trim() === "") return;
    setBusy(true);
    try {
      const parent = taxonomy.hierarchical ? parentId : null;
      let saved: TermDto;
      if (base === null) {
        saved = await createTerm(taxonomy.name, {
          name: name.trim(),
          description,
          parent_id: parent,
          ...(slug.trim() ? { slug: slug.trim() } : {}),
        });
        qc.setQueryData<TermDto[]>(key, (list) => (list ? [...list, saved] : list));
        toast.success(t("taxonomies.terms.created", { name: saved.name }));
      } else {
        const guard = await staleGuard({ reread: () => getTerm(base.id), base, ask: stale.ask });
        if (!guard.proceed) {
          if (guard.reloaded) {
            reset(guard.reloaded);
            toast.info(t("form.reloaded"));
          }
          return;
        }
        const newSlug = slug.trim();
        saved = await patchTerm(base.id, {
          name: name.trim(),
          description,
          parent_id: parent,
          // Empty keeps the current slug.
          ...(newSlug && newSlug !== base.slug ? { slug: newSlug } : {}),
        });
        qc.setQueryData<TermDto[]>(key, (list) =>
          list?.map((x) => (x.id === saved.id ? saved : x)),
        );
        toast.success(t("taxonomies.terms.saved", { name: saved.name }));
      }
      void qc.invalidateQueries({ queryKey: key });
      onClose();
    } catch (err) {
      const fields = err instanceof ApiError ? err.fieldErrors() : {};
      setServerErrors(fields);
      if (!fields.name && !fields.slug && !fields.parent_id) {
        setFailure(
          err instanceof ApiError && err.status === 404
            ? t("taxonomies.terms.gone")
            : problemText(err),
        );
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      noValidate
      onSubmit={(e) => void submit(e)}
      className="flex min-h-0 flex-1 flex-col"
      data-slot="term-form"
    >
      <div className="flex flex-col gap-5 p-5">
        <FormField label={t("taxonomies.terms.name")} error={nameError ?? undefined}>
          {(control) => (
            <Input
              {...control}
              autoFocus
              value={name}
              autoComplete="off"
              disabled={busy}
              onChange={(e) => {
                setName(e.currentTarget.value);
                setServerErrors((errs) => without(errs, "name"));
              }}
            />
          )}
        </FormField>
        <FormField
          label={t("taxonomies.terms.slug")}
          hint={
            base
              ? t("taxonomies.terms.slugHintEdit", { taxonomy: taxonomy.name })
              : t("taxonomies.terms.slugHintNew")
          }
          error={serverErrors.slug}
        >
          {(control) => (
            <Input
              {...control}
              value={slug}
              autoComplete="off"
              spellCheck={false}
              disabled={busy}
              className="font-mono"
              placeholder={base ? base.slug : t("taxonomies.terms.slugPlaceholder")}
              onChange={(e) => {
                setSlug(e.currentTarget.value);
                setServerErrors((errs) => without(errs, "slug"));
              }}
            />
          )}
        </FormField>
        <FormField
          label={t("taxonomies.terms.description")}
          hint={t("taxonomies.terms.descriptionHint")}
        >
          {(control) => (
            <Textarea
              {...control}
              value={description}
              rows={4}
              disabled={busy}
              onChange={(e) => setDescription(e.currentTarget.value)}
            />
          )}
        </FormField>
        {taxonomy.hierarchical ? (
          <TermParentPicker
            terms={terms}
            termId={base?.id ?? null}
            value={parentId}
            onChange={(p) => {
              setParentId(p);
              setServerErrors((errs) => without(errs, "parent_id"));
            }}
            disabled={busy}
            error={serverErrors.parent_id}
          />
        ) : null}
        {failure ? (
          <p role="alert" className="text-sm text-destructive">
            {failure}
          </p>
        ) : null}
      </div>
      <SheetFooter className="flex-row justify-end gap-2 border-t p-5">
        <Button type="button" variant="outline" onClick={onClose}>
          {t("create.cancel")}
        </Button>
        <Button type="submit" disabled={busy}>
          {busy ? t("form.saving") : base ? t("form.save") : t("taxonomies.terms.create")}
        </Button>
      </SheetFooter>
      {stale.dialog}
    </form>
  );
}

/** The server's reason, in the screen's words when it is a known one. */
function deleteRefusal(
  err: unknown,
  t: (key: "taxonomies.terms.delete.refusedChildren") => string,
) {
  if (err instanceof ApiError && err.problem.detail === "has children") {
    return t("taxonomies.terms.delete.refusedChildren");
  }
  return problemText(err);
}

export function DeleteTermDialog({
  taxonomy,
  terms,
  term,
  origin = null,
  onClose,
}: {
  taxonomy: TaxonomyDto;
  terms: readonly TermDto[];
  term: TermDto | null;
  /** The tree row whose menu opened it: focus goes back to that menu on close. */
  origin?: string | null;
  onClose: () => void;
}) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const [busy, setBusy] = useState(false);
  const children = term ? childCount(terms, term.id) : 0;
  const key = qk.terms(taxonomy.name);

  const remove = async () => {
    if (!term) return;
    setBusy(true);
    try {
      await deleteTerm(term.id);
      qc.setQueryData<TermDto[]>(key, (list) => list?.filter((x) => x.id !== term.id));
      void qc.invalidateQueries({ queryKey: key });
      toast.success(t("taxonomies.terms.delete.deleted", { name: term.name }));
      onClose();
    } catch (err) {
      toast.error(t("taxonomies.terms.delete.failed", { name: term.name }), {
        description: deleteRefusal(err, t),
      });
      void qc.invalidateQueries({ queryKey: key });
      onClose();
    } finally {
      setBusy(false);
    }
  };

  return (
    <AlertDialog open={term !== null} onOpenChange={(o) => (o ? undefined : onClose())}>
      <AlertDialogContent data-slot="delete-term-dialog" onCloseAutoFocus={returnToRowMenu(origin)}>
        <AlertDialogHeader>
          <AlertDialogTitle className={cn("break-words")}>
            {children > 0
              ? t("taxonomies.terms.delete.blockedTitle", { name: term?.name ?? "" })
              : t("taxonomies.terms.delete.title", { name: term?.name ?? "" })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {children > 0
              ? t("taxonomies.terms.delete.hasChildren", { count: children })
              : t("taxonomies.terms.delete.body")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>
            {children > 0 ? t("taxonomies.delete.close") : t("delete.cancel")}
          </AlertDialogCancel>
          {children > 0 ? null : (
            <Button
              variant="destructive"
              className={DESTRUCTIVE}
              disabled={busy}
              onClick={() => void remove()}
            >
              {t("taxonomies.terms.delete.confirm")}
            </Button>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
