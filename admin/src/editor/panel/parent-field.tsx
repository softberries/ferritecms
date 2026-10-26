// Parent and order (hierarchical types): the parent is a combobox over
// `GET /content/{type}?q=` (debounced; without a query, the first 50 by title), never
// offering the item itself or its descendants as far as the loaded list shows (the
// server refuses a cycle anyway); "No parent" moves it to the top level. The order is
// `menu_order` (lower first), kept as typed until it is a whole number. A parent the
// viewer may not read shows as "not available" and stays unless another is picked.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { ChevronsUpDownIcon, CornerDownRightIcon } from "lucide-react";
import { useEffect, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "cn";
import { listContent } from "@/api/content";
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
import { parentRefQuery, parentState } from "@/content/parent-ref";
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
  // The same resolver as the list's parent cell: a parent this viewer may not read
  // settles as "not available" and the item keeps it unless another one is picked.
  const parentQuery = useQuery({
    ...parentRefQuery(type, parentId ?? ""),
    enabled: parentId !== null,
  });
  const items = useMemo(() => candidates.data?.items ?? [], [candidates.data]);
  const excluded = useMemo(() => notAParent(doc.item?.id ?? null, items), [doc.item?.id, items]);
  const choices = items.filter((i) => !excluded.has(i.id));
  const listed = items.find((i) => i.id === parentId);
  const parent = parentState(
    parentId,
    parentQuery,
    listed ? { title: listed.title, slug: listed.slug } : undefined,
  );
  const parentTitle =
    parent.kind === "none"
      ? t("parent.none")
      : parent.kind === "found"
        ? parent.ref.title || t("title.untitled")
        : parent.kind === "unavailable"
          ? t("parent.unavailable")
          : t("parent.loading");

  // The order as typed: only a whole number reaches the draft; anything else is put back
  // to the draft's value when the field is left.
  const order = doc.meta.menu_order;
  const [orderText, setOrderText] = useState(String(order));
  const [orderSynced, setOrderSynced] = useState(order);
  if (order !== orderSynced) {
    setOrderSynced(order);
    if (Number.parseInt(orderText, 10) !== order) setOrderText(String(order));
  }
  const orderValid = /^-?\d+$/.test(orderText.trim());

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
              <span
                id={`${labelId}-value`}
                className={cn(
                  "truncate",
                  (parent.kind === "unavailable" || parent.kind === "loading") &&
                    "text-muted-foreground italic",
                )}
              >
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
        {parent.kind === "unavailable" ? (
          <p data-slot="parent-unavailable" className="text-xs text-muted-foreground">
            {t("parent.unavailableHint")}
          </p>
        ) : null}
      </div>
      <div className="flex items-center justify-between gap-3">
        <div className="flex flex-col">
          <Label htmlFor={orderId}>{t("parent.order")}</Label>
          <span className="text-xs text-muted-foreground">{t("parent.orderHint")}</span>
        </div>
        <Input
          id={orderId}
          type="text"
          inputMode="numeric"
          disabled={disabled}
          value={orderText}
          aria-invalid={orderValid ? undefined : true}
          onChange={(e) => {
            const text = e.target.value;
            setOrderText(text);
            if (/^-?\d+$/.test(text.trim())) {
              const n = Number.parseInt(text, 10);
              setOrderSynced(n);
              doc.setMeta({ menu_order: n });
            }
          }}
          onBlur={() => {
            if (!orderValid) setOrderText(String(order));
          }}
          className="h-8 w-20 text-right tabular-nums"
        />
      </div>
    </div>
  );
}
