// A `relation` field: a combobox searching `GET /content/{type}?q=` across every content
// type the sidebar shows (the schema names no target type, P37), each result with its
// type and status. The value is the item's UUID. The chosen item is looked up by id in
// those types (the first that has it); one that is gone or unreadable shows
// "Unknown item <short id>" and keeps the value until another is chosen or it is cleared.

import { keepPreviousData, useQueries, useQuery } from "@tanstack/react-query";
import { ChevronsUpDownIcon, LinkIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { getContent, isContentStatus, listContent } from "@/api/content";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
import { StatusBadge } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { navFor } from "@/shell/nav";
import type { ControlIds } from "./fields-context";
import { isUuid } from "./fields-model";

/** How long the search waits after the last keystroke. */
const SEARCH_DEBOUNCE_MS = 150;
/** Results per type. */
const PER_TYPE = 8;

interface TypeRef {
  name: string;
  label: string;
}

/** The content types the sidebar shows this viewer, in its order. */
function useRelationTypes(): TypeRef[] {
  const me = useMe();
  const types = useQuery(typesQuery);
  return useMemo(() => {
    if (!me || !types.data) return [];
    const labels = new Map(types.data.map((ty) => [ty.name, ty.label]));
    return navFor(me, types.data)
      .flatMap((s) => s.items)
      .flatMap((i) =>
        i.contentType ? [{ name: i.contentType, label: labels.get(i.contentType) ?? i.label }] : [],
      );
  }, [me, types.data]);
}

interface Target {
  id: string;
  title: string;
  status: string;
  type: TypeRef;
}

/** The item `id` in the first of `types` that has it; `null` when none does. */
function useTarget(id: string | null, types: TypeRef[]) {
  return useQuery({
    queryKey: ["content", "relation", id ?? "", types.map((t) => t.name).join(",")],
    queryFn: async (): Promise<Target | null> => {
      for (const type of types) {
        try {
          const item = await getContent(type.name, id ?? "");
          return { id: item.id, title: item.title, status: item.status, type };
        } catch (e) {
          if (e instanceof ApiError && (e.status === 404 || e.status === 400)) continue;
          throw e;
        }
      }
      return null;
    },
    enabled: id !== null && types.length > 0,
    staleTime: 60_000,
  });
}

export function RelationField({
  ids,
  value,
  onChange,
  disabled,
}: {
  ids: ControlIds;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation("editor");
  const types = useRelationTypes();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setQ(search.trim()), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [search]);

  const stored = typeof value === "string" ? value : null;
  const id = stored !== null && isUuid(stored) ? stored.toLowerCase() : null;
  const target = useTarget(id, types);
  // Just picked: known before the lookup answers.
  const [picked, setPicked] = useState<Target | null>(null);
  const shown = picked?.id === id ? picked : (target.data ?? null);

  const filters = q ? { q, limit: PER_TYPE } : { order: "updated" as const, limit: PER_TYPE };
  const results = useQueries({
    queries: types.map((type) => ({
      queryKey: qk.content.list(type.name, filters),
      queryFn: ({ signal }: { signal: AbortSignal }) => listContent(type.name, filters, { signal }),
      enabled: open,
      placeholderData: keepPreviousData,
      staleTime: 30_000,
    })),
  });
  const fetching = results.some((r) => r.isFetching);

  const choose = (next: Target | null) => {
    setPicked(next);
    onChange(next ? next.id : null);
    setOpen(false);
    setSearch("");
  };

  let label: string;
  if (stored === null) label = t("fields.relationPlaceholder");
  else if (shown) label = shown.title || t("title.untitled");
  else if (id !== null && target.isPending) label = t("fields.relationLoading");
  else label = t("fields.relationUnknown", { id: stored.slice(0, 8) });

  return (
    <div className="flex min-w-0 flex-col gap-1.5">
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
            id={ids.id}
            aria-describedby={ids.describedBy}
            {...(ids.invalid ? { "aria-invalid": true } : {})}
            disabled={disabled}
            data-slot="relation-trigger"
            className="h-auto min-h-9 w-full justify-between gap-2 py-1.5 font-normal"
          >
            <span className="flex min-w-0 items-center gap-2">
              <LinkIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
              <span className="truncate" title={label}>
                {label}
              </span>
              {shown ? (
                <span className="shrink-0 text-xs text-muted-foreground">{shown.type.label}</span>
              ) : null}
            </span>
            <ChevronsUpDownIcon data-icon="inline-end" className="text-muted-foreground" />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-(--radix-popover-trigger-width) min-w-72 p-0">
          <Command shouldFilter={false}>
            <CommandInput
              value={search}
              onValueChange={setSearch}
              placeholder={t("fields.relationSearch")}
            />
            <CommandList>
              <CommandEmpty>
                {fetching ? t("fields.relationLoading") : t("fields.relationEmpty")}
              </CommandEmpty>
              {stored !== null && q === "" ? (
                <CommandGroup>
                  <CommandItem value="__none" onSelect={() => choose(null)}>
                    <span className="text-muted-foreground">{t("fields.relationClear")}</span>
                  </CommandItem>
                </CommandGroup>
              ) : null}
              {types.map((type, i) => {
                const items = results[i]?.data?.items ?? [];
                if (items.length === 0) return null;
                return (
                  <CommandGroup key={type.name} heading={type.label}>
                    {items.map((c) => (
                      <CommandItem
                        key={c.id}
                        value={c.id}
                        onSelect={() =>
                          choose({ id: c.id, title: c.title, status: c.status, type })
                        }
                      >
                        <span className="min-w-0 flex-1 truncate" title={c.title}>
                          {c.title || t("title.untitled")}
                        </span>
                        {isContentStatus(c.status) ? (
                          <StatusBadge status={c.status} className="h-5 px-1.5" />
                        ) : null}
                      </CommandItem>
                    ))}
                  </CommandGroup>
                );
              })}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {shown && isContentStatus(shown.status) ? (
        <span className="flex items-center gap-2 text-xs text-muted-foreground">
          <StatusBadge status={shown.status} className="h-5 px-1.5" />
        </span>
      ) : null}
    </div>
  );
}
