// The list's toolbar: the search field (`/` focuses it; the URL follows 200 ms after the
// last keystroke), the Mine chip, one term picker per taxonomy of the type (a Popover +
// Command over `GET /taxonomies/{t}/terms`, at most 200 terms) and the order menu. A
// search is not combinable with Mine or a term (07 §1), and the server orders search
// results by relevance, so while one runs those chips are shown disabled with a tooltip
// that says why (they stay in the URL and come back when the search is cleared).

import { useQuery } from "@tanstack/react-query";
import { cn } from "cn";
import { ArrowDownUpIcon, PlusIcon, SearchIcon, UserRoundIcon, XIcon } from "lucide-react";
import {
  forwardRef,
  useEffect,
  useRef,
  useState,
  type ComponentProps,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { type TaxonomyDto, type TermDto, termsQuery } from "@/api/taxonomies";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import { Kbd } from "@/components/ui/kbd";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useShortcut } from "@/lib/shortcuts";
import {
  LIST_ORDERS,
  type ListOrder,
  type ListSearch,
  SEARCH_DEBOUNCE_MS,
  hasFilters,
  searchDisables,
} from "./list-search";

/** The term picker shows at most this many terms (07 §3; one page holds every term). */
export const TERM_PICKER_LIMIT = 200;

export interface ListFiltersProps {
  /** The type's label, lower-cased for the placeholder ("Search posts…"). */
  label: string;
  search: ListSearch;
  taxonomies: TaxonomyDto[];
  /** Replaces the URL's filters (`undefined` members are removed). */
  onChange: (
    patch: Partial<Record<keyof ListSearch, ListSearch[keyof ListSearch] | undefined>>,
  ) => void;
}

export function ListFilters({ label, search, taxonomies, onChange }: ListFiltersProps) {
  const { t } = useTranslation("content");
  const disabled = searchDisables(search);
  const why = t("filters.disabledBySearch");
  return (
    <div role="search" className="flex flex-col gap-3 md:flex-row md:items-center">
      <SearchField
        label={label}
        value={search.q ?? ""}
        onSearch={(q) => onChange({ q: q || undefined })}
      />
      {/* Phones: one row that scrolls sideways (with an edge fade), "Clear filters" first
          so it is never off-screen; wider screens wrap. The padding keeps focus rings
          inside the scroll container's clip. */}
      <ScrollFade className="-m-1 min-w-0 p-1 md:overflow-visible">
        <div className="flex w-max items-center gap-2 md:w-auto md:flex-wrap">
          <Chip
            pressed={search.mine === true}
            disabledReason={disabled ? why : undefined}
            onClick={() => onChange({ mine: search.mine ? undefined : true })}
          >
            <UserRoundIcon data-icon="inline-start" />
            {t("filters.mine")}
          </Chip>
          {taxonomies.map((tax) => (
            <TermPicker
              key={tax.name}
              taxonomy={tax}
              selected={search.term}
              disabledReason={disabled ? why : undefined}
              onSelect={(term) => onChange({ term })}
            />
          ))}
          <OrderMenu
            order={search.order ?? "updated"}
            disabledReason={disabled ? t("filters.orderBySearch") : undefined}
            onChange={(order) => onChange({ order: order === "updated" ? undefined : order })}
          />
          {hasFilters(search) ? (
            <Button
              variant="ghost"
              className="text-muted-foreground max-md:order-first"
              onClick={() => onChange({ q: undefined, mine: undefined, term: undefined })}
            >
              {t("filters.clear")}
            </Button>
          ) : null}
        </div>
      </ScrollFade>
    </div>
  );
}

/** The search box: local text, the URL updated `SEARCH_DEBOUNCE_MS` after the last key. */
function SearchField({
  label,
  value,
  onSearch,
}: {
  label: string;
  value: string;
  onSearch: (q: string) => void;
}) {
  const { t } = useTranslation("content");
  const input = useRef<HTMLInputElement>(null);
  const [text, setText] = useState(value);
  // The last query this field sent: when the URL catches up with it, keystrokes typed
  // since then are kept. Any other URL change (a "Clear search" button, back/forward)
  // replaces the text.
  const [sent, setSent] = useState(value);
  const [synced, setSynced] = useState(value);
  if (value !== synced) {
    setSynced(value);
    if (value !== sent) {
      setSent(value);
      setText(value);
    }
  }
  const latest = useRef(onSearch);
  useEffect(() => {
    latest.current = onSearch;
  });
  useEffect(() => {
    const q = text.trim();
    if (q === synced.trim()) return;
    const id = window.setTimeout(() => {
      setSent(q);
      latest.current(q);
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [text, synced]);

  useShortcut({
    id: "list.search",
    keys: "/",
    label: t("shortcuts.search"),
    group: t("shortcuts.group"),
    run: () => input.current?.focus(),
  });

  return (
    <InputGroup className="w-full md:w-72 lg:w-80">
      <InputGroupAddon>
        <SearchIcon />
      </InputGroupAddon>
      <InputGroupInput
        ref={input}
        type="search"
        value={text}
        placeholder={t("filters.searchPlaceholder", { label })}
        aria-label={t("filters.searchLabel", { label })}
        aria-keyshortcuts="/"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && text) {
            e.preventDefault();
            setText("");
          }
        }}
        className="[&::-webkit-search-cancel-button]:hidden"
      />
      <InputGroupAddon align="inline-end">
        {text ? (
          <InputGroupButton
            size="icon-xs"
            aria-label={t("filters.clearSearch")}
            onClick={() => {
              setText("");
              input.current?.focus();
            }}
          >
            <XIcon />
          </InputGroupButton>
        ) : (
          <Kbd className="pointer-coarse:hidden" aria-hidden="true">
            /
          </Kbd>
        )}
      </InputGroupAddon>
    </InputGroup>
  );
}

type ChipProps = Omit<ComponentProps<"button">, "disabled"> & {
  pressed?: boolean;
  /** Shown as a tooltip; the chip is `aria-disabled` and ignores clicks. */
  disabledReason?: string | undefined;
  /** A dashed outline while nothing is chosen (a "+ Category" chip). */
  dashed?: boolean;
};

/** A filter chip: dashed while empty, tinted when it narrows the list. */
const Chip = forwardRef<HTMLButtonElement, ChipProps>(function Chip(
  { pressed, disabledReason, dashed, className, onClick, children, ...props },
  ref,
) {
  const button = (
    <button
      ref={ref}
      type="button"
      aria-pressed={pressed}
      aria-disabled={disabledReason ? true : undefined}
      data-slot="filter-chip"
      onClick={(e) => {
        if (disabledReason) {
          e.preventDefault();
          return;
        }
        onClick?.(e);
      }}
      className={cn(
        "inline-flex h-control shrink-0 items-center gap-1.5 rounded-full border px-3 text-sm font-medium whitespace-nowrap transition-colors [&_svg]:size-4 [&_svg]:shrink-0",
        pressed
          ? "border-primary/50 bg-primary/10 text-foreground [&_svg]:text-primary"
          : cn(
              "bg-background text-muted-foreground hover:bg-muted hover:text-foreground dark:bg-input/30",
              dashed ? "border-dashed border-input" : "border-border",
            ),
        disabledReason && "cursor-not-allowed opacity-50 hover:bg-background",
        className,
      )}
      {...props}
    >
      {children}
    </button>
  );
  if (!disabledReason) return button;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <TooltipContent>{disabledReason}</TooltipContent>
    </Tooltip>
  );
});

function TermPicker({
  taxonomy,
  selected,
  disabledReason,
  onSelect,
}: {
  taxonomy: TaxonomyDto;
  selected: string | undefined;
  disabledReason: string | undefined;
  onSelect: (term: string | undefined) => void;
}) {
  const { t } = useTranslation("content");
  const [open, setOpen] = useState(false);
  const terms = useQuery(termsQuery(taxonomy.name));
  const chosen: TermDto | undefined = selected
    ? terms.data?.find((term) => term.id === selected)
    : undefined;
  const shown = (terms.data ?? []).slice(0, TERM_PICKER_LIMIT);

  const trigger: ReactNode = chosen ? (
    <>
      <span className="text-muted-foreground">{taxonomy.label}</span>
      <span className="max-w-40 truncate" title={chosen.name}>
        {chosen.name}
      </span>
    </>
  ) : (
    <>
      <PlusIcon data-icon="inline-start" />
      {taxonomy.label}
    </>
  );

  return (
    <div className="flex items-center">
      <Popover open={open && !disabledReason} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Chip
            pressed={chosen !== undefined}
            dashed
            disabledReason={disabledReason}
            aria-label={
              chosen
                ? t("filters.termChosen", { taxonomy: taxonomy.label, term: chosen.name })
                : t("filters.termPick", { taxonomy: taxonomy.label })
            }
            className={chosen ? "rounded-r-none border-r-0 pr-2" : undefined}
          >
            {trigger}
          </Chip>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-64 p-0">
          {/* No ring on the field: the open popover is the focused context (as the ⌘K
              palette, Task 11). */}
          <Command loop className="**:data-[slot=input-group]:outline-none!">
            <CommandInput placeholder={t("filters.termSearch", { taxonomy: taxonomy.label })} />
            <CommandList>
              {terms.isPending ? (
                <p className="px-3 py-6 text-center text-sm text-muted-foreground">
                  {t("filters.termsLoading")}
                </p>
              ) : shown.length === 0 ? (
                <p className="px-3 py-6 text-center text-sm text-muted-foreground">
                  {t("filters.termsNone", { taxonomy: taxonomy.label })}
                </p>
              ) : (
                <CommandGroup>
                  {shown.map((term) => (
                    <CommandItem
                      key={term.id}
                      value={`${term.name} ${term.slug}`}
                      data-checked={term.id === selected}
                      onSelect={() => {
                        onSelect(term.id === selected ? undefined : term.id);
                        setOpen(false);
                      }}
                    >
                      <span className="truncate" title={term.name}>
                        {term.name}
                      </span>
                    </CommandItem>
                  ))}
                </CommandGroup>
              )}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {chosen ? (
        <Chip
          pressed
          disabledReason={disabledReason}
          aria-label={t("filters.termClear", { taxonomy: taxonomy.label })}
          onClick={() => onSelect(undefined)}
          className="rounded-l-none border-l-primary/20 px-2"
        >
          <XIcon />
        </Chip>
      ) : null}
    </div>
  );
}

function OrderMenu({
  order,
  disabledReason,
  onChange,
}: {
  order: ListOrder;
  disabledReason: string | undefined;
  onChange: (order: ListOrder) => void;
}) {
  const { t } = useTranslation("content");
  const [open, setOpen] = useState(false);
  return (
    <DropdownMenu modal={false} open={open && !disabledReason} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Chip
          disabledReason={disabledReason}
          aria-label={t("order.label", { order: t(`order.${order}`) })}
        >
          <ArrowDownUpIcon data-icon="inline-start" />
          <span className="text-muted-foreground max-sm:hidden">{t("order.sortBy")}</span>
          <span className="text-foreground">{t(`order.${order}`)}</span>
        </Chip>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-52">
        <DropdownMenuLabel>{t("order.sortBy")}</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={order} onValueChange={(v) => onChange(v as ListOrder)}>
          {LIST_ORDERS.map((o) => (
            <DropdownMenuRadioItem key={o} value={o}>
              {t(`order.${o}`)}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
