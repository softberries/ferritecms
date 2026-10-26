// Parent and order (hierarchical types): the parent is a combobox over
// `GET /content/{type}?q=` (debounced; without a query, the first 50 by title), never
// offering the item itself or its descendants as far as the loaded list shows (the
// server refuses a cycle anyway); "No parent" moves it to the top level. The order is
// `menu_order` (lower first).

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { ChevronsUpDownIcon, CornerDownRightIcon } from "lucide-react";
import { useEffect, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { getContent, listContent } from "@/api/content";
import { qk } from "@/api/keys";
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
import type { EditorDocument } from "../editor-state";
import { notAParent } from "./publishing";

/** How long the parent search waits after the last keystroke. */
export const PARENT_SEARCH_DEBOUNCE_MS = 150;

const CANDIDATE_STATUSES = "draft,pending,scheduled,published,private";

export function ParentField({
  doc,
  type,
  disabled,
}: {
  doc: EditorDocument;
  type: string;
  disabled: boolean;
}) {
  const { t } = useTranslation("editor");
  const labelId = useId();
  const orderId = useId();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setQ(search.trim()), PARENT_SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [search]);

  const filters = q
    ? { q, status: CANDIDATE_STATUSES, limit: 20 }
    : { status: CANDIDATE_STATUSES, order: "title" as const, limit: 50 };
  const candidates = useQuery({
    queryKey: qk.content.list(type, filters),
    queryFn: ({ signal }) => listContent(type, filters, { signal }),
    enabled: open,
    placeholderData: keepPreviousData,
    staleTime: 30_000,
  });
  const parentId = doc.meta.parent_id;
  const parent = useQuery({
    queryKey: qk.content.item(type, parentId ?? ""),
    queryFn: () => getContent(type, parentId ?? ""),
    enabled: parentId !== null,
    staleTime: 60_000,
  });
  const items = useMemo(() => candidates.data?.items ?? [], [candidates.data]);
  const excluded = useMemo(() => notAParent(doc.item?.id ?? null, items), [doc.item?.id, items]);
  const choices = items.filter((i) => !excluded.has(i.id));
  const parentTitle =
    parentId === null
      ? t("parent.none")
      : (items.find((i) => i.id === parentId)?.title ?? parent.data?.title ?? "…") ||
        t("title.untitled");

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1.5">
        <Label id={labelId}>{t("parent.label")}</Label>
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
              disabled={disabled}
              aria-labelledby={`${labelId} ${labelId}-value`}
              data-slot="parent-trigger"
              className="h-8 justify-between font-normal"
            >
              <span id={`${labelId}-value`} className="truncate">
                {parentTitle}
              </span>
              <ChevronsUpDownIcon data-icon="inline-end" className="text-muted-foreground" />
            </Button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-(--radix-popover-trigger-width) min-w-64 p-0">
            <Command shouldFilter={false}>
              <CommandInput
                value={search}
                onValueChange={setSearch}
                placeholder={t("parent.search")}
              />
              <CommandList>
                <CommandEmpty>
                  {candidates.isFetching ? t("parent.loading") : t("parent.empty")}
                </CommandEmpty>
                <CommandGroup>
                  {q === "" ? (
                    <CommandItem
                      value="__none"
                      onSelect={() => {
                        doc.setMeta({ parent_id: null });
                        setOpen(false);
                      }}
                    >
                      <span className="text-muted-foreground">{t("parent.none")}</span>
                    </CommandItem>
                  ) : null}
                  {choices.map((c) => (
                    <CommandItem
                      key={c.id}
                      value={c.id}
                      onSelect={() => {
                        doc.setMeta({ parent_id: c.id });
                        setOpen(false);
                      }}
                    >
                      {c.parent_id ? (
                        <CornerDownRightIcon className="text-muted-foreground" />
                      ) : null}
                      <span className="truncate">{c.title || t("title.untitled")}</span>
                      <span className="ml-auto truncate font-mono text-xs text-muted-foreground">
                        {c.slug}
                      </span>
                    </CommandItem>
                  ))}
                </CommandGroup>
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      </div>
      <div className="flex items-center justify-between gap-3">
        <div className="flex flex-col">
          <Label htmlFor={orderId}>{t("parent.order")}</Label>
          <span className="text-xs text-muted-foreground">{t("parent.orderHint")}</span>
        </div>
        <Input
          id={orderId}
          type="number"
          inputMode="numeric"
          step={1}
          disabled={disabled}
          value={String(doc.meta.menu_order)}
          onChange={(e) => {
            const n = Number.parseInt(e.target.value, 10);
            doc.setMeta({ menu_order: Number.isFinite(n) ? n : 0 });
          }}
          className="h-8 w-20 text-right tabular-nums"
        />
      </div>
    </div>
  );
}
