// A reorderable list (11 §7): dnd-kit's classic sortable API. Each row has a drag handle
// (pointer: drag after 4 px; keyboard: Space or Enter picks up, arrows move, Space or
// Enter drops, Escape cancels) and a menu with Move up / Move down, the alternative to
// dragging on touch screens and the one E2E drives. dnd-kit speaks every step through
// its own live region, by the item's label (never its id); menu moves are spoken through
// the list's own polite status. Rows moved from the menu travel with the layout spring
// (11 §5); dragged rows follow dnd-kit's transform instead, and under reduced motion
// nothing animates.

import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type Announcements,
  type DragEndEvent,
  type UniqueIdentifier,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { cn } from "cn";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  GripVerticalIcon,
  MoreHorizontalIcon,
  type LucideIcon,
} from "lucide-react";
import { useReducedMotion } from "motion/react";
import * as m from "motion/react-m";
import {
  useCallback,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { DRAG_DISTANCE_PX, LAYOUT_SPRING, useSettling } from "./sortable-shared";

export interface SortableListProps<T> {
  items: T[];
  getId: (item: T) => string;
  /** The row's content, between the handle and the menu. */
  render: (item: T) => ReactNode;
  /** Called once per completed move with the whole list in its new order. */
  onReorder: (next: T[]) => void;
  /** The item's name, for the handle, the menu and what is announced. */
  label: (item: T) => string;
  /** The list's accessible name. */
  name?: string;
  className?: string;
  /** Extra classes for each row's frame (`items-start` for rows that grow tall). */
  rowClassName?: string;
}

export function SortableList<T>({
  items,
  getId,
  render,
  onReorder,
  label,
  name,
  className,
  rowClassName,
}: SortableListProps<T>) {
  const { t } = useTranslation("kit");
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: DRAG_DISTANCE_PX } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  const ids = useMemo(() => items.map(getId), [items, getId]);
  const [dragging, setDragging] = useState(false);
  const [settling, settle] = useSettling();
  const [status, setStatus] = useState("");
  const list = useRef<HTMLUListElement>(null);
  const lastOver = useRef<UniqueIdentifier | null>(null);
  const motionOn = !(useReducedMotion() ?? false) && !dragging && !settling;

  const labelOf = useCallback(
    (id: UniqueIdentifier) => {
      const item = items.find((i) => getId(i) === id);
      return item ? label(item) : "";
    },
    [items, getId, label],
  );
  const position = useCallback(
    (id: UniqueIdentifier | undefined) => ids.indexOf(String(id)) + 1,
    [ids],
  );

  const announcements = useMemo<Announcements>(() => {
    const count = ids.length;
    return {
      onDragStart: ({ active }) => {
        lastOver.current = active.id;
        return t("sortable.picked", {
          label: labelOf(active.id),
          position: position(active.id),
          count,
        });
      },
      // Only when the target changes: dnd-kit also reports the start position.
      onDragOver: ({ active, over }) => {
        if (!over || over.id === lastOver.current) return undefined;
        lastOver.current = over.id;
        return t("sortable.over", {
          label: labelOf(active.id),
          position: position(over.id),
          count,
        });
      },
      onDragEnd: ({ active, over }) =>
        t("sortable.dropped", {
          label: labelOf(active.id),
          position: position(over?.id ?? active.id),
          count,
        }),
      onDragCancel: ({ active }) =>
        t("sortable.cancelled", {
          label: labelOf(active.id),
          position: position(active.id),
          count,
        }),
    };
  }, [ids, labelOf, position, t]);

  const move = (from: number, to: number) => {
    if (from === to || from < 0 || to < 0 || to >= items.length) return;
    onReorder(arrayMove(items, from, to));
  };

  const onDragEnd = ({ active, over }: DragEndEvent) => {
    setDragging(false);
    settle();
    if (over) move(ids.indexOf(String(active.id)), ids.indexOf(String(over.id)));
  };

  const menuMove = (item: T, delta: -1 | 1) => {
    const from = items.indexOf(item);
    const to = from + delta;
    if (to < 0 || to >= items.length) return;
    setStatus(t("sortable.moved", { label: label(item), position: to + 1, count: items.length }));
    move(from, to);
  };

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      accessibility={{
        announcements,
        screenReaderInstructions: { draggable: t("sortable.instructions") },
      }}
      onDragStart={() => setDragging(true)}
      onDragEnd={onDragEnd}
      onDragCancel={() => {
        setDragging(false);
        settle();
      }}
    >
      <SortableContext items={ids} strategy={verticalListSortingStrategy}>
        <ul
          ref={list}
          aria-label={name}
          data-slot="sortable-list"
          className={cn("flex flex-col gap-1", className)}
        >
          {items.map((item, i) => {
            const id = getId(item);
            return (
              <SortableRow
                key={id}
                id={id}
                label={label(item)}
                layout={motionOn}
                container={list}
                {...(rowClassName ? { className: rowClassName } : {})}
                actions={[
                  {
                    key: "up",
                    label: t("sortable.moveUp"),
                    icon: ArrowUpIcon,
                    disabled: i === 0,
                    run: () => menuMove(item, -1),
                  },
                  {
                    key: "down",
                    label: t("sortable.moveDown"),
                    icon: ArrowDownIcon,
                    disabled: i === items.length - 1,
                    run: () => menuMove(item, 1),
                  },
                ]}
              >
                {render(item)}
              </SortableRow>
            );
          })}
        </ul>
      </SortableContext>
      <p data-slot="sortable-status" role="status" className="sr-only">
        {status}
      </p>
    </DndContext>
  );
}

export interface RowAction {
  key: string;
  label: string;
  icon: LucideIcon;
  disabled: boolean;
  run: () => void;
  /** Starts a new group in the menu. */
  separated?: boolean;
}

export interface SortableRowProps {
  id: string;
  label: string;
  actions: RowAction[];
  /** Animate position changes with the layout spring (off while dragging). */
  layout: boolean;
  /** The list; the menu's trigger is found again in it after a move. */
  container: RefObject<HTMLElement | null>;
  /** Extra props for the row (a tree's `aria-level`, its indent). */
  rowProps?: Record<string, unknown>;
  /** The left inset of the row's frame (a tree's depth). */
  inset?: number;
  /** Shown as the drop target (the tree's indent guide). */
  placeholder?: boolean;
  /**
   * Drop the horizontal part of dnd-kit's transform: the tree places its dragged row by
   * `inset` (the projected depth), so the pointer's x would offset it twice.
   */
  lockX?: boolean;
  /** Extra classes for the row's frame. */
  className?: string;
  children: ReactNode;
}

/** One row: handle, content, menu. Shared by `SortableList` and `SortableTree`. */
export function SortableRow({
  id,
  label,
  actions,
  layout,
  container,
  rowProps,
  inset = 0,
  placeholder = false,
  lockX = false,
  className,
  children,
}: SortableRowProps) {
  const { t } = useTranslation("kit");
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id, attributes: { roleDescription: t("sortable.roleDescription") } });
  const style: CSSProperties = {
    transform: CSS.Translate.toString(lockX && transform ? { ...transform, x: 0 } : transform),
    transition,
    marginInlineStart: inset,
  };
  // A menu move re-inserts the row in the DOM, which drops focus if it lands after
  // Radix has focused the trigger; focus this row's menu button a frame later instead.
  const refocus = (e: Event) => {
    e.preventDefault();
    requestAnimationFrame(() => {
      // Unless focus has gone somewhere since (a click on another row's menu).
      const current = document.activeElement;
      if (current && current !== document.body) return;
      const triggers = container.current?.querySelectorAll<HTMLElement>("[data-row-menu]") ?? [];
      Array.from(triggers)
        .find((el) => el.dataset.rowMenu === id)
        ?.focus();
    });
  };
  return (
    <m.li
      layout={layout ? "position" : false}
      transition={LAYOUT_SPRING}
      data-sortable-row=""
      className="list-none"
      {...rowProps}
    >
      <div
        ref={setNodeRef}
        style={style}
        data-dragging={isDragging || undefined}
        data-placeholder={placeholder || undefined}
        className={cn(
          "flex min-h-row items-center gap-1 rounded-lg border bg-card py-1 pr-1 pl-1 text-sm shadow-1",
          isDragging && "relative z-10 shadow-2",
          placeholder && "border-dashed border-primary/60 bg-primary/4 shadow-none",
          className,
        )}
      >
        <Button
          ref={setActivatorNodeRef}
          variant="ghost"
          size="icon"
          className="cursor-grab touch-none text-muted-foreground active:cursor-grabbing"
          {...attributes}
          {...listeners}
          aria-label={t("sortable.handle", { label })}
        >
          <GripVerticalIcon />
        </Button>
        <div className="min-w-0 flex-1">{children}</div>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              data-row-menu={id}
              aria-label={t("sortable.actions", { label })}
            >
              <MoreHorizontalIcon />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-44" onCloseAutoFocus={refocus}>
            {actions.map((a) => (
              <RowMenuItem key={a.key} action={a} />
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </m.li>
  );
}

function RowMenuItem({ action }: { action: RowAction }) {
  const Icon = action.icon;
  return (
    <>
      {action.separated ? <DropdownMenuSeparator /> : null}
      <DropdownMenuItem disabled={action.disabled} onSelect={action.run}>
        <Icon />
        {action.label}
      </DropdownMenuItem>
    </>
  );
}
