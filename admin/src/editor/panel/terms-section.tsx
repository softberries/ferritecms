// One section per taxonomy of the type (07 §3): a hierarchical taxonomy (categories)
// is a checkbox tree, a flat one (tags) chips plus a searchable combobox. Creating a
// term inline is offered only when `me.capabilities` has `manage_terms` (the server
// checks again); the new term joins the cached list and the selection at once. The
// selection is the draft's `terms` (one list for every taxonomy, in order).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronsUpDownIcon, PlusIcon, XIcon } from "lucide-react";
import { useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type TaxonomyDto, type TermDto, createTerm, termsQuery } from "@/api/taxonomies";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { uiLocale } from "@/lib/locale";
import { termTree, toggleTerm } from "./publishing";

export interface TermsSectionProps {
  taxonomy: TaxonomyDto;
  /** Every selected term id (all taxonomies), in order. */
  selected: string[];
  onChange: (terms: string[]) => void;
  /** `me.capabilities` includes `manage_terms`. */
  canCreate: boolean;
  disabled: boolean;
  /** On a multilingual site, the item's locale: only its terms are offered, and new
   * ones are made in it (M6-1). */
  locale?: string | undefined;
}

export function TermsSection(props: TermsSectionProps) {
  const { t } = useTranslation("editor");
  const headingId = useId();
  const terms = useQuery(termsQuery(props.taxonomy.name));
  const locale = props.locale;
  const data = useMemo(
    () => (locale && terms.data ? terms.data.filter((x) => x.locale === locale) : terms.data),
    [terms.data, locale],
  );
  return (
    <section
      aria-labelledby={headingId}
      data-slot="panel-terms"
      data-taxonomy={props.taxonomy.name}
      className="flex flex-col gap-2"
    >
      <h3 id={headingId} className="text-sm font-medium">
        {props.taxonomy.label}
      </h3>
      {terms.isPending ? (
        <p className="text-xs text-muted-foreground">{t("terms.loading")}</p>
      ) : terms.isError ? (
        <p className="text-xs text-destructive">{t("terms.failed")}</p>
      ) : props.taxonomy.hierarchical ? (
        <TermTree {...props} terms={data ?? []} />
      ) : (
        <TagPicker {...props} terms={data ?? []} />
      )}
    </section>
  );
}

/** Creates a term and adds it to the cached list and the selection. */
function useCreateTerm(
  taxonomy: TaxonomyDto,
  locale: string | undefined,
  onCreated: (term: TermDto) => void,
) {
  const { t } = useTranslation("editor");
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) =>
      createTerm(taxonomy.name, { name, ...(locale ? { locale } : {}) }),
    onSuccess: (term) => {
      qc.setQueryData<TermDto[]>(qk.terms(taxonomy.name), (old) => [...(old ?? []), term]);
      onCreated(term);
    },
    onError: (e) => {
      const detail = e instanceof ApiError ? e.problem.errors?.[0]?.detail || e.problem.detail : "";
      toast.error(detail ? t("terms.createFailedWith", { detail }) : t("terms.createFailed"));
    },
  });
}

function TermTree({
  taxonomy,
  terms,
  selected,
  onChange,
  canCreate,
  disabled,
  locale,
}: TermsSectionProps & { terms: TermDto[] }) {
  const { t } = useTranslation("editor");
  const tree = useMemo(() => termTree(terms, uiLocale()), [terms]);
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const create = useCreateTerm(taxonomy, locale, (term) => {
    onChange([...selected, term.id]);
    setName("");
    setAdding(false);
  });
  return (
    <div className="flex flex-col gap-2">
      {tree.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          {t("terms.none", { label: taxonomy.label })}
        </p>
      ) : (
        <div
          role="group"
          aria-label={taxonomy.label}
          className="-mx-1.5 flex max-h-56 flex-col overflow-y-auto py-0.5"
        >
          {tree.map(({ term, depth }) => {
            const checked = selected.includes(term.id);
            const id = `term-${term.id}`;
            return (
              <div key={term.id} style={{ paddingInlineStart: `${depth * 1.25}rem` }}>
                <label
                  htmlFor={id}
                  className="flex min-h-8 cursor-pointer items-center gap-2.5 rounded-md px-1.5 text-sm hover:bg-muted/60 has-disabled:cursor-default has-disabled:hover:bg-transparent"
                >
                  <Checkbox
                    id={id}
                    checked={checked}
                    disabled={disabled}
                    onCheckedChange={() => onChange(toggleTerm(selected, term.id))}
                  />
                  <span className="truncate">{term.name}</span>
                </label>
              </div>
            );
          })}
        </div>
      )}
      {canCreate && !disabled ? (
        adding ? (
          <form
            className="flex items-center gap-1.5"
            onSubmit={(e) => {
              e.preventDefault();
              const trimmed = name.trim();
              if (trimmed !== "") create.mutate(trimmed);
            }}
          >
            <Input
              autoFocus
              value={name}
              aria-label={t("terms.newName", { label: taxonomy.label })}
              placeholder={t("terms.newPlaceholder")}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  e.stopPropagation();
                  setAdding(false);
                }
              }}
              className="h-8 flex-1"
            />
            <Button type="submit" size="sm" disabled={name.trim() === "" || create.isPending}>
              {t("terms.add")}
            </Button>
          </form>
        ) : (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="self-start text-muted-foreground"
            onClick={() => setAdding(true)}
          >
            <PlusIcon data-icon="inline-start" />
            {t("terms.newTerm", { label: taxonomy.label })}
          </Button>
        )
      ) : null}
    </div>
  );
}

function TagPicker({
  taxonomy,
  terms,
  selected,
  onChange,
  canCreate,
  disabled,
  locale,
}: TermsSectionProps & { terms: TermDto[] }) {
  const { t } = useTranslation("editor");
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const byId = useMemo(() => new Map(terms.map((term) => [term.id, term])), [terms]);
  const chosen = selected.map((id) => byId.get(id)).filter((x): x is TermDto => !!x);
  const create = useCreateTerm(taxonomy, locale, (term) => {
    onChange([...selected, term.id]);
    setSearch("");
  });
  const typed = search.trim();
  const exact = terms.some((term) => term.name.toLowerCase() === typed.toLowerCase());
  const available = terms.filter((term) => !selected.includes(term.id));
  return (
    <div className="flex flex-col gap-2">
      {chosen.length > 0 ? (
        <ul aria-label={taxonomy.label} className="flex flex-wrap gap-1.5">
          {chosen.map((term) => (
            <li
              key={term.id}
              className="inline-flex h-7 items-center gap-0.5 rounded-md border bg-muted/40 pr-0.5 pl-2 text-xs font-medium pointer-coarse:h-11.5 pointer-coarse:pr-0"
            >
              {term.name}
              {!disabled ? (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  aria-label={t("terms.remove", { name: term.name })}
                  onClick={() => onChange(selected.filter((id) => id !== term.id))}
                  className="text-muted-foreground"
                >
                  <XIcon />
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      {!disabled ? (
        <Popover
          open={open}
          onOpenChange={(o) => {
            setOpen(o);
            if (!o) setSearch("");
          }}
        >
          <PopoverTrigger asChild>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="justify-between self-stretch font-normal text-muted-foreground"
            >
              {t("terms.addTo", { label: taxonomy.label })}
              <ChevronsUpDownIcon data-icon="inline-end" />
            </Button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-(--radix-popover-trigger-width) min-w-60 p-0">
            <Command>
              <CommandInput
                value={search}
                onValueChange={setSearch}
                placeholder={t("terms.search", { label: taxonomy.label })}
              />
              <CommandList>
                <CommandEmpty>{canCreate ? t("terms.emptyCreate") : t("terms.empty")}</CommandEmpty>
                {available.length > 0 ? (
                  <CommandGroup>
                    {available.map((term) => (
                      <CommandItem
                        key={term.id}
                        value={`${term.name} ${term.id}`}
                        onSelect={() => {
                          onChange([...selected, term.id]);
                          setSearch("");
                        }}
                      >
                        {term.name}
                      </CommandItem>
                    ))}
                  </CommandGroup>
                ) : null}
                {canCreate && typed !== "" && !exact ? (
                  <CommandGroup forceMount>
                    <CommandItem
                      forceMount
                      value={`__create ${typed}`}
                      disabled={create.isPending}
                      onSelect={() => create.mutate(typed)}
                    >
                      <PlusIcon />
                      {t("terms.create", { name: typed })}
                    </CommandItem>
                  </CommandGroup>
                ) : null}
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      ) : chosen.length === 0 ? (
        <p className="text-xs text-muted-foreground">{t("terms.noneChosen")}</p>
      ) : null}
    </div>
  );
}
