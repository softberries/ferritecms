// Audit log (`GET /system/audit`): newest first, 50 a page with Load more. Filters in
// the URL: the actor (a user picker over `GET /users?q=`), the action (a known action,
// any action of an area, or "Starts with" a typed prefix ending in a dot: the API takes
// an exact action or such a prefix) and the target (a kind, optionally one id). A table
// of time, actor, the action in words (`audit-text.ts`; unknown actions raw, with the
// code always shown), the target and the details as a key-value list. Logins, target ids
// and details are stored text and render as text.

import { keepPreviousData, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { PlusIcon, ScrollTextIcon, SearchXIcon, XIcon } from "lucide-react";
import { type ReactNode, useEffect, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AUDIT_PAGE, type AuditDto, listAudit } from "@/api/audit";
import { INLINE_ERRORS } from "@/api/client";
import { qk } from "@/api/keys";
import { listUsers, userQuery } from "@/api/users";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { ScrollFade } from "@/components/kit/scroll-fade";
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Chip } from "@/content/filters";
import { problemText } from "@/structure/taxonomies/terms-model";
import { UserAvatar } from "@/users/user-parts";
import {
  AUDIT_AREAS,
  TARGET_KINDS,
  actionsOf,
  auditActionText,
  detailEntries,
  isAuditAction,
  isAuditPrefix,
  targetKindText,
} from "./audit-text";
import type { TabProps } from "./jobs-tab";

/** The actor picker's search waits this long after the last key (as the users list). */
const PICKER_DEBOUNCE_MS = 250;
const PICKER_LIMIT = 20;

export function AuditTab({ search, setSearch }: TabProps) {
  const { t } = useTranslation("admin");
  const filters = useMemo(
    () => ({
      ...(search.actor ? { actor: search.actor } : {}),
      ...(search.action ? { action: search.action } : {}),
      ...(search.target_kind ? { target_kind: search.target_kind } : {}),
      ...(search.target_kind && search.target_id ? { target_id: search.target_id } : {}),
    }),
    [search.actor, search.action, search.target_kind, search.target_id],
  );
  const list = useInfiniteQuery({
    queryKey: qk.system.auditInfinite(filters),
    queryFn: ({ pageParam, signal }) =>
      listAudit(
        { ...filters, limit: AUDIT_PAGE, ...(pageParam ? { cursor: pageParam } : {}) },
        { signal },
      ),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    placeholderData: keepPreviousData,
  });
  const rows = useMemo(() => {
    const seen = new Map<string, AuditDto>();
    for (const page of list.data?.pages ?? []) {
      for (const r of page.items) if (!seen.has(r.id)) seen.set(r.id, r);
    }
    return [...seen.values()];
  }, [list.data]);
  const columns = useColumns();
  const filtered = Object.keys(filters).length > 0;
  const clear = () =>
    setSearch({
      actor: undefined,
      action: undefined,
      target_kind: undefined,
      target_id: undefined,
    });

  let empty: ReactNode;
  if (filtered) {
    empty = (
      <EmptyState
        icon={SearchXIcon}
        title={t("system.audit.noMatch.title")}
        body={t("system.audit.noMatch.body")}
        action={
          <Button variant="outline" onClick={clear}>
            {t("system.audit.clearFilters")}
          </Button>
        }
        className="border-0"
      />
    );
  } else {
    empty = (
      <EmptyState
        icon={ScrollTextIcon}
        title={t("system.audit.empty.title")}
        body={t("system.audit.empty.body")}
        className="border-0"
      />
    );
  }

  return (
    <div data-slot="system-audit" className="flex flex-col gap-4">
      <div role="search" className="min-w-0">
        <ScrollFade className="-m-1 min-w-0 p-1 lg:overflow-visible">
          <div className="flex w-max items-center gap-2 lg:w-auto lg:flex-wrap">
            <ActorPicker actor={search.actor} onSelect={(actor) => setSearch({ actor })} />
            <ActionPicker action={search.action} onSelect={(action) => setSearch({ action })} />
            <TargetPicker
              kind={search.target_kind}
              id={search.target_id}
              onSelect={(target_kind, target_id) => setSearch({ target_kind, target_id })}
            />
            {filtered ? (
              <Button variant="ghost" className="text-muted-foreground" onClick={clear}>
                {t("system.audit.clearFilters")}
              </Button>
            ) : null}
          </div>
        </ScrollFade>
      </div>
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} />
      ) : (
        <DataTable
          label={t("system.audit.table")}
          columns={columns}
          data={rows}
          getRowId={(r) => r.id}
          loading={list.isPending}
          align="top"
          columnClassName={{
            time: "hidden py-3 whitespace-nowrap md:table-cell",
            actor: "hidden max-w-0 py-3 md:table-cell md:w-[14%]",
            action: "w-full max-w-0 py-2.5 md:w-[46%]",
            target: "hidden max-w-0 py-3 lg:table-cell lg:w-[22%]",
          }}
          className={
            list.isPlaceholderData ? "opacity-60 transition-opacity" : "transition-opacity"
          }
          empty={empty}
          footer={
            list.hasNextPage ? (
              <div className="flex justify-center border-t px-4 py-3">
                <Button
                  variant="ghost"
                  disabled={list.isFetchingNextPage}
                  aria-busy={list.isFetchingNextPage || undefined}
                  onClick={() => {
                    if (!list.isFetchingNextPage) void list.fetchNextPage();
                  }}
                >
                  {list.isFetchingNextPage
                    ? t("system.audit.loadingMore")
                    : t("system.audit.loadMore")}
                </Button>
              </div>
            ) : null
          }
        />
      )}
    </div>
  );
}

function ActorText({ entry }: { entry: AuditDto }) {
  const { t } = useTranslation("admin");
  return entry.actor ? (
    <span className="block truncate font-mono text-sm" title={entry.actor.login}>
      {entry.actor.login}
    </span>
  ) : (
    <span className="text-sm text-muted-foreground" title={t("system.audit.systemHint")}>
      {t("system.audit.system")}
    </span>
  );
}

function TargetText({ entry }: { entry: AuditDto }) {
  const { t } = useTranslation("admin");
  return (
    <span className="flex min-w-0 flex-col">
      <span className="text-sm">{targetKindText(t, entry.target_kind)}</span>
      {entry.target_id ? (
        <span className="truncate font-mono text-xs text-muted-foreground" title={entry.target_id}>
          {entry.target_id}
        </span>
      ) : null}
    </span>
  );
}

function Details({ details }: { details: unknown }) {
  const { t } = useTranslation("admin");
  const entries = detailEntries(details);
  if (entries.length === 0) return null;
  return (
    <ul
      aria-label={t("system.audit.details")}
      data-slot="audit-details"
      className="flex flex-col gap-0.5 rounded-md bg-muted/60 px-2.5 py-1.5"
    >
      {entries.map(([key, value]) => (
        <li key={key} className="flex min-w-0 gap-2 text-xs">
          <span className="shrink-0 font-mono text-muted-foreground">{key}</span>
          <span className="min-w-0 font-mono whitespace-pre-wrap text-foreground wrap-anywhere">
            {value}
          </span>
        </li>
      ))}
    </ul>
  );
}

function useColumns(): DataTableColumn<AuditDto>[] {
  const { t } = useTranslation("admin");
  return useMemo(() => {
    const col = dataTableColumns<AuditDto>();
    return [
      col.accessor("at", {
        id: "time",
        header: () => t("system.audit.columns.time"),
        cell: (c) => (
          <RelativeTime at={c.row.original.at} className="text-sm text-muted-foreground" />
        ),
      }),
      col.accessor("actor", {
        id: "actor",
        header: () => t("system.audit.columns.actor"),
        cell: (c) => <ActorText entry={c.row.original} />,
      }),
      col.accessor("action", {
        id: "action",
        header: () => t("system.audit.columns.action"),
        cell: (c) => {
          const r = c.row.original;
          const text = auditActionText(t, r.action);
          return (
            <div className="flex min-w-0 flex-col gap-1.5">
              <span className="flex min-w-0 flex-wrap items-baseline gap-x-2">
                {text !== r.action ? (
                  <span className="text-sm font-medium text-foreground">{text}</span>
                ) : null}
                <code
                  data-slot="audit-action-code"
                  className="font-mono text-xs break-all text-muted-foreground"
                >
                  {r.action}
                </code>
              </span>
              {/* Below `md` the time and actor columns are hidden, below `lg` the target:
                  here. */}
              <span className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground lg:hidden">
                <span className="contents md:hidden">
                  <RelativeTime at={r.at} />
                  <span aria-hidden="true">·</span>
                  <ActorText entry={r} />
                  <span aria-hidden="true">·</span>
                </span>
                <span className="min-w-0 wrap-anywhere">
                  {targetKindText(t, r.target_kind)}
                  {r.target_id ? ` ${r.target_id}` : ""}
                </span>
              </span>
              <Details details={r.details} />
            </div>
          );
        },
      }),
      col.accessor("target_kind", {
        id: "target",
        header: () => t("system.audit.columns.target"),
        cell: (c) => <TargetText entry={c.row.original} />,
      }),
    ];
  }, [t]);
}

/** A filter chip that opens a popover; with a value, a second chip clears it. */
function FilterChip({
  label,
  value,
  chosenLabel,
  clearLabel,
  onClear,
  open,
  onOpenChange,
  children,
  wide = false,
}: {
  label: string;
  value: ReactNode | undefined;
  chosenLabel: string;
  clearLabel: string;
  onClear: () => void;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
  wide?: boolean;
}) {
  const chosen = value !== undefined;
  return (
    <div className="flex items-center">
      <Popover open={open} onOpenChange={onOpenChange}>
        <PopoverTrigger asChild>
          <Chip
            pressed={chosen}
            dashed
            aria-label={chosen ? chosenLabel : label}
            className={chosen ? "rounded-r-none border-r-0 pr-2" : undefined}
          >
            {chosen ? (
              <>
                <span className="text-muted-foreground">{label}</span>
                <span className="max-w-48 truncate">{value}</span>
              </>
            ) : (
              <>
                <PlusIcon data-icon="inline-start" />
                {label}
              </>
            )}
          </Chip>
        </PopoverTrigger>
        <PopoverContent align="start" className={wide ? "w-80 p-0" : "w-72 p-0"}>
          {children}
        </PopoverContent>
      </Popover>
      {chosen ? (
        <Chip
          pressed
          aria-label={clearLabel}
          onClick={onClear}
          className="rounded-l-none border-l-primary/20 px-2"
        >
          <XIcon />
        </Chip>
      ) : null}
    </div>
  );
}

function ActorPicker({
  actor,
  onSelect,
}: {
  actor: string | undefined;
  onSelect: (actor: string | undefined) => void;
}) {
  const { t } = useTranslation("admin");
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setQ(text.trim()), PICKER_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [text]);
  const users = useQuery({
    queryKey: ["users", "list", "audit-picker", q],
    queryFn: ({ signal }) => listUsers({ limit: PICKER_LIMIT, ...(q ? { q } : {}) }, { signal }),
    placeholderData: keepPreviousData,
    enabled: open,
    meta: INLINE_ERRORS,
  });
  // The chosen actor's name (the URL holds only the id).
  const chosen = useQuery({
    ...userQuery(actor ?? ""),
    enabled: actor !== undefined,
    meta: INLINE_ERRORS,
  });
  const name = actor ? (chosen.data?.display_name ?? actor.slice(0, 8)) : undefined;

  return (
    <FilterChip
      label={t("system.audit.actor.label")}
      value={name}
      chosenLabel={t("system.audit.actor.chosen", { name: name ?? "" })}
      clearLabel={t("system.audit.actor.clear")}
      onClear={() => onSelect(undefined)}
      open={open}
      onOpenChange={setOpen}
    >
      <Command
        label={t("system.audit.actor.search")}
        shouldFilter={false}
        loop
        className="**:data-[slot=input-group]:outline-none!"
      >
        <CommandInput
          value={text}
          onValueChange={setText}
          placeholder={t("system.audit.actor.search")}
        />
        <CommandList>
          {users.isPending ? (
            <p className="px-3 py-6 text-center text-sm text-muted-foreground">
              {t("system.audit.actor.loading")}
            </p>
          ) : users.isError ? (
            <p role="alert" className="px-3 py-6 text-center text-sm text-destructive">
              {problemText(users.error)}
            </p>
          ) : users.data.items.length === 0 ? (
            <p className="px-3 py-6 text-center text-sm text-muted-foreground">
              {t("system.audit.actor.none")}
            </p>
          ) : (
            <CommandGroup>
              {users.data.items.map((u) => (
                <CommandItem
                  key={u.id}
                  value={u.id}
                  data-checked={u.id === actor}
                  onSelect={() => {
                    onSelect(u.id === actor ? undefined : u.id);
                    setOpen(false);
                  }}
                >
                  <UserAvatar user={u} />
                  <span className="flex min-w-0 flex-col">
                    <span className="truncate" title={u.display_name}>
                      {u.display_name}
                    </span>
                    <span className="truncate font-mono text-xs text-muted-foreground">
                      {u.login}
                    </span>
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          )}
        </CommandList>
      </Command>
    </FilterChip>
  );
}

function ActionPicker({
  action,
  onSelect,
}: {
  action: string | undefined;
  onSelect: (action: string | undefined) => void;
}) {
  const { t } = useTranslation("admin");
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");
  const typed = text.trim();
  const pick = (value: string) => {
    onSelect(value === action ? undefined : value);
    setOpen(false);
    setText("");
  };
  const shown =
    action === undefined
      ? undefined
      : action.endsWith(".")
        ? t("system.audit.action.prefix", { prefix: action })
        : auditActionText(t, action);
  return (
    <FilterChip
      label={t("system.audit.action.label")}
      value={shown}
      chosenLabel={t("system.audit.action.chosen", { action: shown ?? "" })}
      clearLabel={t("system.audit.action.clear")}
      onClear={() => onSelect(undefined)}
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (!o) setText("");
      }}
      wide
    >
      <Command
        label={t("system.audit.action.search")}
        loop
        className="**:data-[slot=input-group]:outline-none!"
      >
        <CommandInput
          value={text}
          onValueChange={setText}
          placeholder={t("system.audit.action.search")}
        />
        <CommandList>
          <CommandEmpty>
            <span className="block px-3 text-muted-foreground">
              {t("system.audit.action.hint")}
            </span>
          </CommandEmpty>
          {isAuditPrefix(typed) ? (
            <CommandGroup forceMount>
              <CommandItem forceMount value={`prefix ${typed}`} onSelect={() => pick(typed)}>
                {t("system.audit.action.startsWith", { prefix: typed })}
              </CommandItem>
            </CommandGroup>
          ) : null}
          {action && !action.endsWith(".") && !isAuditAction(action) ? (
            <CommandGroup>
              <CommandItem value={action} data-checked onSelect={() => pick(action)}>
                <code className="font-mono text-xs">{action}</code>
              </CommandItem>
            </CommandGroup>
          ) : null}
          {AUDIT_AREAS.map((area) => (
            <CommandGroup key={area} heading={t(`system.audit.areaTitles.${area}`)}>
              <CommandItem
                value={`${area}. ${t(`system.audit.areaTitles.${area}`)}`}
                data-checked={action === `${area}.`}
                onSelect={() => pick(`${area}.`)}
              >
                <span className="min-w-0 flex-1 truncate">
                  {t("system.audit.action.any", { area: t(`system.audit.areas.${area}`) })}
                </span>
                <code className="font-mono text-xs text-muted-foreground">{`${area}.`}</code>
              </CommandItem>
              {actionsOf(area).map((a) => (
                <CommandItem
                  key={a}
                  value={a}
                  keywords={[auditActionText(t, a)]}
                  data-checked={action === a}
                  onSelect={() => pick(a)}
                >
                  <span className="min-w-0 flex-1 truncate">{auditActionText(t, a)}</span>
                  <code className="font-mono text-xs text-muted-foreground">{a}</code>
                </CommandItem>
              ))}
            </CommandGroup>
          ))}
        </CommandList>
      </Command>
    </FilterChip>
  );
}

const ANY = "__any";

function TargetPicker({
  kind,
  id,
  onSelect,
}: {
  kind: string | undefined;
  id: string | undefined;
  onSelect: (kind: string | undefined, id: string | undefined) => void;
}) {
  const { t } = useTranslation("admin");
  const formId = useId();
  const [open, setOpen] = useState(false);
  const [draftKind, setDraftKind] = useState(kind ?? "");
  const [draftId, setDraftId] = useState(id ?? "");
  const kinds =
    kind && !(TARGET_KINDS as readonly string[]).includes(kind)
      ? [...TARGET_KINDS, kind]
      : [...TARGET_KINDS];
  const shown = kind ? `${targetKindText(t, kind)}${id ? ` ${id}` : ""}` : undefined;
  return (
    <FilterChip
      label={t("system.audit.target.label")}
      value={shown}
      chosenLabel={t("system.audit.target.chosen", { target: shown ?? "" })}
      clearLabel={t("system.audit.target.clear")}
      onClear={() => onSelect(undefined, undefined)}
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (o) {
          setDraftKind(kind ?? "");
          setDraftId(id ?? "");
        }
      }}
    >
      <form
        className="flex flex-col gap-3 p-3"
        onSubmit={(e) => {
          e.preventDefault();
          const k = draftKind || undefined;
          const i = draftId.trim();
          onSelect(k, k && i ? i : undefined);
          setOpen(false);
        }}
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={`${formId}-kind`}>{t("system.audit.target.kind")}</Label>
          <Select value={draftKind || ANY} onValueChange={(v) => setDraftKind(v === ANY ? "" : v)}>
            <SelectTrigger id={`${formId}-kind`} className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent position="popper">
              <SelectItem value={ANY}>{t("system.audit.target.pick")}</SelectItem>
              {kinds.map((k) => (
                <SelectItem key={k} value={k}>
                  {targetKindText(t, k)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={`${formId}-id`}>{t("system.audit.target.id")}</Label>
          <Input
            id={`${formId}-id`}
            value={draftId}
            maxLength={255}
            disabled={!draftKind}
            autoComplete="off"
            spellCheck={false}
            aria-describedby={`${formId}-id-hint`}
            onChange={(e) => setDraftId(e.currentTarget.value)}
            className="font-mono"
          />
          <p id={`${formId}-id-hint`} className="text-xs text-muted-foreground">
            {t("system.audit.target.idHint")}
          </p>
        </div>
        <Button type="submit" className="self-end">
          {t("system.audit.target.apply")}
        </Button>
      </form>
    </FilterChip>
  );
}
