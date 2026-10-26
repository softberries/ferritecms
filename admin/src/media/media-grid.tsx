// The library's grid: responsive square tiles (`MediaThumb md` filling the tile), the
// title and type below, a selection checkbox that shows on hover or focus (always on
// touch screens and while anything is selected). A multi-select listbox for assistive
// tech: each tile is an option with `aria-selected`. Keys as in the `DataTable`:
// `j`/`k` (and the arrows, up/down by a row of tiles) move, `x` or Space selects,
// `enter` opens. Tiles
// enter, leave and move with the layout spring (11 §5); none under reduced motion.
// `single` (the media picker): a single-select listbox; a click or Space selects one
// tile, a double click or `enter` opens (picks) it, and there is no check box.

import { cn } from "cn";
import { CheckIcon } from "lucide-react";
import { AnimatePresence, useReducedMotion } from "motion/react";
import * as m from "motion/react-m";
import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { MediaDto } from "@/api/media";
import type { RowSelectionState } from "@/components/kit/data-table-features";
import { MediaThumb } from "@/components/kit/media-thumb";
import { Skeleton } from "@/components/ui/skeleton";
import { MOTION } from "@/design/tokens";
import { dimensions, displayTitle, typeLabel } from "./format";

const LAYOUT_SPRING = { type: "spring", stiffness: 520, damping: 42, mass: 1 } as const;
const SKELETON_TILES = 12;
const GRID =
  "grid grid-cols-[repeat(auto-fill,minmax(9.5rem,1fr))] gap-4 sm:grid-cols-[repeat(auto-fill,minmax(11rem,1fr))]";
/** The picker's dialog is narrower: smaller tiles, two to a row on a phone. */
const GRID_SINGLE =
  "grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-3 sm:grid-cols-[repeat(auto-fill,minmax(10rem,1fr))]";

export interface MediaGridProps {
  items: MediaDto[];
  /** The grid's accessible name. */
  label: string;
  loading: boolean;
  empty: ReactNode;
  footer?: ReactNode;
  selection: { state: RowSelectionState; onChange: (next: RowSelectionState) => void };
  onOpen: (media: MediaDto) => void;
  /** One selected tile at a time; a click selects instead of opening. */
  single?: boolean;
  className?: string;
}

export function MediaGrid({
  items,
  label,
  loading,
  empty,
  footer,
  selection,
  onOpen,
  single = false,
  className,
}: MediaGridProps) {
  const { t } = useTranslation("media");
  const reduce = useReducedMotion() ?? false;
  const list = useRef<HTMLUListElement>(null);
  const hintId = useId();
  const [active, setActive] = useState(0);
  const current = Math.min(active, Math.max(items.length - 1, 0));
  const anySelected = Object.values(selection.state).some(Boolean);

  const toggle = (id: string) => {
    if (single) {
      selection.onChange({ [id]: true });
      return;
    }
    const next = { ...selection.state };
    if (next[id]) delete next[id];
    else next[id] = true;
    selection.onChange(next);
  };

  const tiles = () => Array.from(list.current?.querySelectorAll<HTMLElement>("[data-tile]") ?? []);
  const focusTile = (index: number) => {
    const next = Math.max(0, Math.min(items.length - 1, index));
    setActive(next);
    tiles()[next]?.focus();
  };
  /** Tiles per row, from where they wrap. */
  const columns = () => {
    const all = tiles();
    const top = all[0]?.offsetTop;
    const n = all.findIndex((el) => el.offsetTop !== top);
    return n <= 0 ? all.length : n;
  };

  const onKeyDown = (e: KeyboardEvent<HTMLLIElement>, item: MediaDto, index: number) => {
    if (e.target !== e.currentTarget) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    switch (e.key) {
      case "j":
      case "ArrowRight":
        e.preventDefault();
        focusTile(index + 1);
        break;
      case "k":
      case "ArrowLeft":
        e.preventDefault();
        focusTile(index - 1);
        break;
      case "ArrowDown":
        e.preventDefault();
        focusTile(index + columns());
        break;
      case "ArrowUp":
        e.preventDefault();
        focusTile(index - columns());
        break;
      case "x":
      case " ":
        e.preventDefault();
        toggle(item.id);
        break;
      case "Enter":
        e.preventDefault();
        onOpen(item);
        break;
    }
  };

  if (loading) {
    return (
      <div aria-busy="true" className={className}>
        <span className="sr-only">{t("grid.loading")}</span>
        <ul aria-hidden="true" className={single ? GRID_SINGLE : GRID}>
          {Array.from({ length: SKELETON_TILES }, (_, i) => (
            <li key={i} className="flex flex-col gap-2 p-1.5">
              <Skeleton className="aspect-square w-full rounded-lg" />
              <Skeleton className="h-3.5 w-3/4" />
              <Skeleton className="h-3 w-1/3" />
            </li>
          ))}
        </ul>
      </div>
    );
  }
  if (items.length === 0) return <div className={className}>{empty}</div>;

  return (
    <div data-slot="media-grid" className={cn("flex flex-col gap-4", className)}>
      <p id={hintId} className="sr-only">
        {t("grid.hint")}
      </p>
      <ul
        ref={list}
        role="listbox"
        aria-multiselectable={single ? "false" : "true"}
        aria-label={label}
        aria-describedby={hintId}
        className={single ? GRID_SINGLE : GRID}
      >
        <AnimatePresence initial={false}>
          {items.map((item, index) => {
            const title = displayTitle(item);
            const selected = selection.state[item.id] === true;
            const dims = dimensions(item);
            return (
              <m.li
                key={item.id}
                data-tile=""
                role="option"
                aria-selected={selected}
                data-state={selected ? "selected" : undefined}
                tabIndex={index === current ? 0 : -1}
                aria-label={title}
                onFocus={(e) => {
                  if (e.target === e.currentTarget) setActive(index);
                }}
                onKeyDown={(e) => onKeyDown(e, item, index)}
                onClick={(e) => {
                  if ((e.target as HTMLElement).closest("[data-slot=tile-check]")) return;
                  if (single) toggle(item.id);
                  else onOpen(item);
                }}
                {...(single ? { onDoubleClick: () => onOpen(item) } : {})}
                className="group/tile relative flex min-w-0 cursor-pointer flex-col gap-2 rounded-xl p-1.5 transition-colors hover:bg-muted/60 data-[state=selected]:bg-primary/6"
                {...(reduce
                  ? {}
                  : {
                      layout: "position" as const,
                      initial: { opacity: 0, scale: 0.96 },
                      animate: { opacity: 1, scale: 1 },
                      exit: { opacity: 0, scale: 0.96 },
                      transition: {
                        opacity: { duration: MOTION.durBase, ease: MOTION.easeOut },
                        scale: { duration: MOTION.durBase, ease: MOTION.easeOut },
                        layout: LAYOUT_SPRING,
                      },
                    })}
              >
                <div
                  className={cn(
                    "relative rounded-lg ring-2 ring-transparent transition-shadow",
                    selected && "ring-primary",
                  )}
                >
                  <MediaThumb media={item} size="md" fill caption={false} />
                  {/* A pointer target only: assistive tech selects the option itself
                      (`aria-selected`, Space or `x`), so no nested control. At least
                      44 px on touch screens, where it is the way to select. */}
                  {single ? null : (
                    <span
                      aria-hidden="true"
                      data-slot="tile-check"
                      onClick={(e) => {
                        e.stopPropagation();
                        toggle(item.id);
                      }}
                      className={cn(
                        "absolute top-0 left-0 flex size-10 cursor-pointer items-start justify-start p-2 transition-opacity pointer-coarse:size-14",
                        selected || anySelected
                          ? "opacity-100"
                          : "opacity-0 group-hover/tile:opacity-100 group-focus-within/tile:opacity-100 pointer-coarse:opacity-100",
                      )}
                    >
                      <span className="flex rounded-md bg-background/90 p-1 shadow-1">
                        <span
                          data-checked={selected || undefined}
                          className="flex size-4 items-center justify-center rounded-[4px] border border-input bg-background text-primary-foreground data-checked:border-primary data-checked:bg-primary pointer-coarse:size-6"
                        >
                          {selected ? (
                            <CheckIcon className="size-3.5 pointer-coarse:size-4" />
                          ) : null}
                        </span>
                      </span>
                    </span>
                  )}
                  {single && selected ? (
                    <span
                      aria-hidden="true"
                      className="absolute top-2 right-2 flex size-6 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-1"
                    >
                      <CheckIcon className="size-4" />
                    </span>
                  ) : null}
                </div>
                <div className="flex min-w-0 flex-col gap-0.5 px-0.5">
                  <span
                    className="min-w-0 truncate text-sm font-medium text-foreground"
                    title={title}
                  >
                    {title}
                  </span>
                  <span className="truncate text-xs text-muted-foreground tabular-nums">
                    {[typeLabel(item), dims].filter(Boolean).join(" · ")}
                  </span>
                </div>
              </m.li>
            );
          })}
        </AnimatePresence>
      </ul>
      {footer}
    </div>
  );
}
