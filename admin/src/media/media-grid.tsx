// The library's grid: responsive square tiles (`MediaThumb md` filling the tile), the
// title and type below, a selection checkbox that shows on hover or focus (always on
// touch screens and while anything is selected). Keys as in the `DataTable`: `j`/`k`
// (and the arrows, up/down by a row of tiles) move, `x` selects, `enter` opens. Tiles
// enter, leave and move with the layout spring (11 §5); none under reduced motion.

import { cn } from "cn";
import { AnimatePresence, useReducedMotion } from "motion/react";
import * as m from "motion/react-m";
import { useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { MediaDto } from "@/api/media";
import type { RowSelectionState } from "@/components/kit/data-table-features";
import { MediaThumb } from "@/components/kit/media-thumb";
import { Checkbox } from "@/components/ui/checkbox";
import { Skeleton } from "@/components/ui/skeleton";
import { MOTION } from "@/design/tokens";
import { dimensions, displayTitle, typeLabel } from "./format";

const LAYOUT_SPRING = { type: "spring", stiffness: 520, damping: 42, mass: 1 } as const;
const SKELETON_TILES = 12;
const GRID =
  "grid grid-cols-[repeat(auto-fill,minmax(9.5rem,1fr))] gap-4 sm:grid-cols-[repeat(auto-fill,minmax(11rem,1fr))]";

export interface MediaGridProps {
  items: MediaDto[];
  /** The grid's accessible name. */
  label: string;
  loading: boolean;
  empty: ReactNode;
  footer?: ReactNode;
  selection: { state: RowSelectionState; onChange: (next: RowSelectionState) => void };
  onOpen: (media: MediaDto) => void;
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
  className,
}: MediaGridProps) {
  const { t } = useTranslation("media");
  const reduce = useReducedMotion() ?? false;
  const list = useRef<HTMLUListElement>(null);
  const [active, setActive] = useState(0);
  const current = Math.min(active, Math.max(items.length - 1, 0));
  const anySelected = Object.values(selection.state).some(Boolean);

  const toggle = (id: string) => {
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
        <ul aria-hidden="true" className={GRID}>
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
      <ul ref={list} aria-label={label} className={GRID}>
        <AnimatePresence initial={false}>
          {items.map((item, index) => {
            const title = displayTitle(item);
            const selected = selection.state[item.id] === true;
            const dims = dimensions(item);
            return (
              <m.li
                key={item.id}
                data-tile=""
                data-state={selected ? "selected" : undefined}
                tabIndex={index === current ? 0 : -1}
                aria-label={title}
                onFocus={(e) => {
                  if (e.target === e.currentTarget) setActive(index);
                }}
                onKeyDown={(e) => onKeyDown(e, item, index)}
                onClick={(e) => {
                  if ((e.target as HTMLElement).closest("[role=checkbox]")) return;
                  onOpen(item);
                }}
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
                  <div
                    className={cn(
                      "absolute top-2 left-2 flex rounded-md bg-background/90 p-1 shadow-1 transition-opacity",
                      selected || anySelected
                        ? "opacity-100"
                        : "opacity-0 group-hover/tile:opacity-100 group-focus-within/tile:opacity-100 pointer-coarse:opacity-100",
                    )}
                  >
                    <Checkbox
                      aria-label={t("grid.select", { title })}
                      checked={selected}
                      onCheckedChange={() => toggle(item.id)}
                      className="pointer-coarse:size-6"
                    />
                  </div>
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
