// The menu editor's Add items panel (07 §3 Menus): three tabs. Content searches
// `GET /content/{type}?q=` in every type the sidebar shows (the most recently updated
// without a query), Terms lists a taxonomy's terms with a filter, and Custom link takes an
// address under core's rules and an optional label. Content and terms are picked with
// checkboxes and added together; new entries go to the end of the menu with an empty
// label (the site then shows the target's current title).

import { keepPreviousData, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { PlusIcon } from "lucide-react";
import { useEffect, useId, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { isContentStatus, listContent } from "@/api/content";
import { qk } from "@/api/keys";
import { type TermDto, taxonomiesQuery, termsQuery } from "@/api/taxonomies";
import { StatusBadge } from "@/components/kit/status-badge";
import { flatten } from "@/components/kit/tree-model";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { type ContentTarget, contentTargetQuery, useNavContentTypes } from "@/content/find-item";
import { filterTerms, toTree } from "../taxonomies/terms-model";
import { FormField } from "../types/form-field";
import {
  MAX_MENU_ITEMS,
  MAX_MENU_LABEL,
  MAX_MENU_URL,
  type MenuEntry,
  validMenuUrl,
} from "./menu-model";
import { termTargetQuery } from "./targets";

/** How long the content search waits after the last keystroke. */
const SEARCH_DEBOUNCE_MS = 150;
/** Results per type. */
const PER_TYPE = 8;

export interface AddItemsProps {
  /** Adds the entries at the end of the menu. */
  onAdd: (entries: MenuEntry[]) => void;
  /** How many more entries the menu takes (`MAX_MENU_ITEMS` minus what it holds). */
  room: number;
  disabled: boolean;
}

export function AddItems({ onAdd, room, disabled }: AddItemsProps) {
  const { t } = useTranslation("structure");
  const [tab, setTab] = useState("content");
  const [status, setStatus] = useState("");
  const add = (entries: MenuEntry[]) => {
    onAdd(entries);
    setStatus(t("menus.add.added", { count: entries.length }));
  };
  return (
    <div data-slot="menu-add-items" className="flex flex-col gap-3">
      <Tabs value={tab} onValueChange={setTab} className="gap-3">
        <TabsList className="grid w-full grid-cols-3 group-data-horizontal/tabs:h-control">
          <TabsTrigger value="content">{t("menus.add.tabs.content")}</TabsTrigger>
          <TabsTrigger value="terms">{t("menus.add.tabs.terms")}</TabsTrigger>
          <TabsTrigger value="link">{t("menus.add.tabs.link")}</TabsTrigger>
        </TabsList>
        <TabsContent value="content">
          <ContentTab onAdd={add} room={room} disabled={disabled} />
        </TabsContent>
        <TabsContent value="terms">
          <TermsTab onAdd={add} room={room} disabled={disabled} />
        </TabsContent>
        <TabsContent value="link">
          <LinkTab onAdd={add} room={room} disabled={disabled} />
        </TabsContent>
      </Tabs>
      <p role="status" className="sr-only">
        {status}
      </p>
    </div>
  );
}

function ContentTab({ onAdd, room, disabled }: AddItemsProps) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const types = useNavContentTypes();
  const [search, setSearch] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setQ(search.trim()), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [search]);
  const [picked, setPicked] = useState<ReadonlyMap<string, ContentTarget>>(new Map());

  const filters = q ? { q, limit: PER_TYPE } : { order: "updated" as const, limit: PER_TYPE };
  const results = useQueries({
    queries: types.map((type) => ({
      queryKey: qk.content.list(type.name, filters),
      queryFn: ({ signal }: { signal: AbortSignal }) => listContent(type.name, filters, { signal }),
      placeholderData: keepPreviousData,
      staleTime: 30_000,
    })),
  });
  const pending = types.length === 0 || results.some((r) => r.isPending);
  const failed = results.length > 0 && results.every((r) => r.isError);
  const groups = types.flatMap((type, i) => {
    const items = results[i]?.data?.items ?? [];
    return items.length > 0 ? [{ type, items }] : [];
  });

  const toggle = (target: ContentTarget, on: boolean) =>
    setPicked((m) => {
      const next = new Map(m);
      if (on) next.set(target.id, target);
      else next.delete(target.id);
      return next;
    });
  const add = () => {
    const targets = [...picked.values()];
    for (const target of targets) {
      qc.setQueryData(contentTargetQuery(target.id, types).queryKey, target);
    }
    onAdd(targets.map((x) => ({ kind: "item", targetId: x.id, url: null, label: "" })));
    setPicked(new Map());
  };

  return (
    <div className="flex flex-col gap-3">
      <Input
        type="search"
        value={search}
        aria-label={t("menus.add.search")}
        placeholder={t("menus.add.searchPlaceholder")}
        onChange={(e) => setSearch(e.currentTarget.value)}
      />
      <PickList
        label={q ? t("menus.add.results") : t("menus.add.recent")}
        loading={pending && groups.length === 0}
        empty={
          failed
            ? t("menus.add.failed")
            : groups.length === 0
              ? q
                ? t("menus.add.noResults", { q })
                : t("menus.add.noContent")
              : null
        }
      >
        {groups.map(({ type, items }) => (
          <li key={type.name} className="flex flex-col gap-0.5">
            <h3 className="px-1 pt-1 text-xs font-medium text-muted-foreground">{type.label}</h3>
            <ul className="flex flex-col">
              {items.map((c) => {
                const target: ContentTarget = { id: c.id, title: c.title, status: c.status, type };
                return (
                  <PickRow
                    key={c.id}
                    checked={picked.has(c.id)}
                    onChange={(on) => toggle(target, on)}
                    disabled={disabled}
                    label={c.title || t("menus.add.untitled")}
                    extra={
                      isContentStatus(c.status) && c.status !== "published" ? (
                        <StatusBadge status={c.status} className="h-5 px-1.5" />
                      ) : null
                    }
                  />
                );
              })}
            </ul>
          </li>
        ))}
      </PickList>
      <AddButton count={picked.size} room={room} disabled={disabled} onClick={add} />
    </div>
  );
}

function TermsTab({ onAdd, room, disabled }: AddItemsProps) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const taxonomies = useQuery(taxonomiesQuery);
  const [chosen, setChosen] = useState<string | null>(null);
  const taxonomy = chosen ?? taxonomies.data?.[0]?.name ?? null;
  const tax = taxonomies.data?.find((x) => x.name === taxonomy);
  const terms = useQuery({ ...termsQuery(taxonomy ?? ""), enabled: taxonomy !== null });
  const [query, setQuery] = useState("");
  const [picked, setPicked] = useState<ReadonlyMap<string, TermDto>>(new Map());
  const shown = useMemo(() => {
    const list = terms.data ?? [];
    if (query.trim()) return filterTerms(list, query).map((term) => ({ term, depth: 0 }));
    return flatten(toTree(list)).map((n) => ({ term: n.data, depth: n.depth }));
  }, [terms.data, query]);

  const toggle = (term: TermDto, on: boolean) =>
    setPicked((m) => {
      const next = new Map(m);
      if (on) next.set(term.id, term);
      else next.delete(term.id);
      return next;
    });
  const add = () => {
    const list = [...picked.values()];
    for (const term of list) qc.setQueryData(termTargetQuery(term.id).queryKey, term);
    onAdd(list.map((x) => ({ kind: "term", targetId: x.id, url: null, label: "" })));
    setPicked(new Map());
  };
  const label = tax?.label ?? taxonomy ?? "";

  return (
    <div className="flex flex-col gap-3">
      {taxonomies.data?.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("menus.add.noTaxonomies")}</p>
      ) : (
        <>
          <div className="grid gap-2 sm:grid-cols-2">
            <Select
              value={taxonomy ?? ""}
              onValueChange={(v) => {
                setChosen(v);
                setQuery("");
              }}
            >
              <SelectTrigger className="w-full" aria-label={t("menus.add.taxonomy")}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent position="popper" className="max-h-80">
                {(taxonomies.data ?? []).map((x) => (
                  <SelectItem key={x.name} value={x.name}>
                    {x.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Input
              type="search"
              value={query}
              aria-label={t("menus.add.filterTerms", { label })}
              placeholder={t("menus.add.filterPlaceholder")}
              onChange={(e) => setQuery(e.currentTarget.value)}
            />
          </div>
          <PickList
            label={label}
            loading={taxonomies.isPending || terms.isPending}
            empty={
              terms.isError
                ? t("menus.add.failed")
                : shown.length === 0
                  ? query.trim()
                    ? t("menus.add.noTermMatch", { q: query.trim() })
                    : t("menus.add.noTerms", { label })
                  : null
            }
          >
            {shown.map(({ term, depth }) => (
              <PickRow
                key={term.id}
                checked={picked.has(term.id)}
                onChange={(on) => toggle(term, on)}
                disabled={disabled}
                label={term.name}
                depth={depth}
              />
            ))}
          </PickList>
          <AddButton count={picked.size} room={room} disabled={disabled} onClick={add} />
        </>
      )}
    </div>
  );
}

function LinkTab({ onAdd, room, disabled }: AddItemsProps) {
  const { t } = useTranslation("structure");
  const [url, setUrl] = useState("");
  const [label, setLabel] = useState("");
  const [attempted, setAttempted] = useState(false);
  const urlError = attempted && !validMenuUrl(url.trim()) ? t("menus.add.urlInvalid") : undefined;
  const labelError =
    [...label].length > MAX_MENU_LABEL
      ? t("menus.editor.problems.labelTooLong", { max: MAX_MENU_LABEL })
      : undefined;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    setAttempted(true);
    if (disabled || room < 1 || !validMenuUrl(url.trim()) || labelError) return;
    onAdd([{ kind: "url", targetId: null, url: url.trim(), label }]);
    setUrl("");
    setLabel("");
    setAttempted(false);
  };
  return (
    <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
      <FormField label={t("menus.add.url")} hint={t("menus.add.urlHint")} error={urlError}>
        {(control) => (
          <Input
            {...control}
            type="url"
            inputMode="url"
            value={url}
            maxLength={MAX_MENU_URL + 20}
            placeholder="https://"
            className="font-mono"
            spellCheck={false}
            onChange={(e) => setUrl(e.currentTarget.value)}
          />
        )}
      </FormField>
      <FormField
        label={t("menus.add.linkLabel")}
        hint={t("menus.add.linkLabelHint")}
        error={labelError}
      >
        {(control) => (
          <Input {...control} value={label} onChange={(e) => setLabel(e.currentTarget.value)} />
        )}
      </FormField>
      {room < 1 ? <Full room={room} /> : null}
      <Button
        type="submit"
        variant="outline"
        className="self-start"
        disabled={disabled || room < 1}
      >
        <PlusIcon data-icon="inline-start" />
        {t("menus.add.addLink")}
      </Button>
    </form>
  );
}

/** A scrolling list of checkboxes, its loading and empty states. */
function PickList({
  label,
  loading,
  empty,
  children,
}: {
  label: string;
  loading: boolean;
  empty: string | null;
  children: ReactNode;
}) {
  if (loading) {
    return (
      <div className="flex flex-col gap-2 py-1" role="status" aria-label={label}>
        {[0, 1, 2, 3].map((i) => (
          <Skeleton key={i} className="h-6" />
        ))}
      </div>
    );
  }
  if (empty !== null) {
    return <p className="rounded-lg bg-muted px-3 py-3 text-sm text-muted-foreground">{empty}</p>;
  }
  return (
    <ul
      aria-label={label}
      data-slot="menu-pick-list"
      className="flex max-h-80 flex-col gap-1 overflow-y-auto rounded-lg border p-1"
    >
      {children}
    </ul>
  );
}

function PickRow({
  checked,
  onChange,
  disabled,
  label,
  extra,
  depth = 0,
}: {
  checked: boolean;
  onChange: (on: boolean) => void;
  disabled: boolean;
  label: string;
  extra?: ReactNode;
  depth?: number;
}) {
  const id = useId();
  return (
    <li
      className="flex min-h-11 items-center gap-2.5 rounded-md px-2 hover:bg-muted sm:min-h-9"
      style={{ paddingInlineStart: 8 + depth * 16 }}
    >
      <Checkbox
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(on) => onChange(on === true)}
      />
      <Label htmlFor={id} className="min-w-0 flex-1 font-normal">
        <span className="truncate" title={label}>
          {label}
        </span>
      </Label>
      {extra}
    </li>
  );
}

function AddButton({
  count,
  room,
  disabled,
  onClick,
}: {
  count: number;
  room: number;
  disabled: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation("structure");
  return (
    <div className="flex flex-col gap-2">
      {count > room ? <Full room={room} /> : null}
      <Button
        variant="outline"
        className="self-start"
        disabled={disabled || count === 0 || count > room}
        onClick={onClick}
      >
        <PlusIcon data-icon="inline-start" />
        {count > 0 ? t("menus.add.add", { count }) : t("menus.add.addNone")}
      </Button>
    </div>
  );
}

function Full({ room }: { room: number }) {
  const { t } = useTranslation("structure");
  return (
    <p className="text-sm text-destructive">
      {t("menus.add.full", { max: MAX_MENU_ITEMS, left: Math.max(0, room) })}
    </p>
  );
}
