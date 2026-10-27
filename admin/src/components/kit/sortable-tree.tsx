// A reorderable tree (menus, hierarchical terms; 11 §7) over `tree-model.ts`: the rows
// are the flattened tree, indented by depth. While a row is dragged its descendants
// travel with it (hidden until the drop) and the row itself is drawn as a dashed guide at
// the depth the drop would give it: horizontal movement changes the level
// (`projectDrop`, 24 px a level), clamped by the neighbours and `maxDepth`. Each row's
// menu has Move up / Move down (among its siblings) and Indent / Outdent: the keyboard
// and touch way to change the order and the level (keyboard dragging only moves rows up
// and down). Motion and announcements as in `SortableList`. An `unordered` tree (terms,
// P35) keeps its siblings in the caller's order (by name): a move there is a new parent
// only, and the caller gives the row menu (`actions`, e.g. Move to…) in place of Move up /
// Move down / Indent / Outdent. An ordered tree may add its own items around those
// (`leadingActions`, `trailingActions`; menus: Edit before, Remove after).

import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type Announcements,
  type DragOverEvent,
  type DragStartEvent,
  type UniqueIdentifier,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { getEventCoordinates } from "@dnd-kit/utilities";
import { cn } from "cn";
import { ArrowDownIcon, ArrowUpIcon, IndentIcon, OutdentIcon } from "lucide-react";
import { useReducedMotion } from "motion/react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { SortableRow, type RowAction } from "./sortable-list";
import { DRAG_DISTANCE_PX, useSettling } from "./sortable-shared";
import {
  flatten,
  indent,
  moveNode,
  outdent,
  projectDrop,
  subtreeHeight,
  subtreeIds,
  type FlatNode,
  type TreeNode,
  type TreePosition,
} from "./tree-model";

/** One level of nesting, on screen and for the drag projection. */
const INDENT_PX = 24;

export interface TreeMove extends TreePosition {
  id: string;
}

export interface SortableTreeProps<T> {
  nodes: TreeNode<T>[];
  /** The row's content, between the handle and the menu. */
  render: (node: FlatNode<T>) => ReactNode;
  /** Called once per completed move with every node, and which one moved where. */
  onChange: (next: TreeNode<T>[], moved: TreeMove) => void;
  /** The deepest level a row may reach (0 = no nesting). */
  maxDepth: number;
  /** The node's name, for the handle, the menu and what is announced. */
  label: (data: T) => string;
  /** Whether `parent` may hold children (default: every node may). */
  canNest?: (parent: TreeNode<T>) => boolean;
  /** The tree's accessible name. */
  name?: string;
  className?: string;
  /**
   * Siblings have no order of their own (the caller sorts them): a drop or a menu action
   * that keeps the parent is no move, and what is announced names the parent only.
   */
  unordered?: boolean;
  /** The row menu in place of Move up / Move down / Indent / Outdent. */
  actions?: (node: FlatNode<T>) => RowAction[];
  /** Row menu items before Move up / Move down / Indent / Outdent (e.g. Edit). */
  leadingActions?: (node: FlatNode<T>) => RowAction[];
  /** Row menu items after them (e.g. Remove). */
  trailingActions?: (node: FlatNode<T>) => RowAction[];
}

export function SortableTree<T>({
  nodes,
  render,
  onChange,
  maxDepth,
  label,
  canNest,
  name,
  className,
  unordered = false,
  actions,
  leadingActions,
  trailingActions,
}: SortableTreeProps<T>) {
  const { t } = useTranslation("kit");
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: DRAG_DISTANCE_PX } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  const [activeId, setActiveId] = useState<string | null>(null);
  const [overId, setOverId] = useState<string | null>(null);
  const [offsetX, setOffsetX] = useState(0);
  // The level follows the pointer's own horizontal travel since the drag began, not
  // dnd-kit's `delta`: dnd-kit takes the dragged node's layout shift off its delta, and
  // this tree shifts that very node (the guide sits at the projected level), so its
  // delta undid each level change the next frame and the guide flickered between two
  // levels (CI run 40404431115). A keyboard drag only moves up and down: no listener,
  // its x stays 0.
  const stopPointer = useRef<(() => void) | null>(null);
  useEffect(() => () => stopPointer.current?.(), []);
  const followPointer = (activator: Event) => {
    stopPointer.current?.();
    stopPointer.current = null;
    const start = getEventCoordinates(activator);
    if (activator instanceof KeyboardEvent || start === null) return;
    const onMove = (e: PointerEvent) => setOffsetX(e.clientX - start.x);
    window.addEventListener("pointermove", onMove);
    stopPointer.current = () => window.removeEventListener("pointermove", onMove);
  };
  const [status, setStatus] = useState("");
  const list = useRef<HTMLUListElement>(null);
  const lastOver = useRef<UniqueIdentifier | null>(null);
  const [settling, settle] = useSettling();
  const motionOn = !(useReducedMotion() ?? false) && activeId === null && !settling;

  const flat = useMemo(() => flatten(nodes), [nodes]);
  const byId = useMemo(() => new Map(nodes.map((n) => [n.id, n])), [nodes]);
  const nameOf = (id: UniqueIdentifier | null | undefined) => {
    const node = id == null ? undefined : byId.get(String(id));
    return node ? label(node.data) : "";
  };
  const nestable = (parentId: string) => {
    const parent = byId.get(parentId);
    return parent !== undefined && (canNest?.(parent) ?? true);
  };

  // While dragging, the active row's descendants are out of the list.
  const visible = useMemo(() => {
    if (activeId === null) return flat;
    const below = subtreeIds(nodes, activeId);
    below.delete(activeId);
    return flat.filter((n) => !below.has(n.id));
  }, [flat, nodes, activeId]);
  const ids = useMemo(() => visible.map((n) => n.id), [visible]);
  const projection =
    activeId !== null && overId !== null
      ? projectDrop(flat, activeId, overId, offsetX, INDENT_PX, maxDepth, nestable)
      : null;

  /** What a finished move says, from the tree after it. */
  const describe = (next: TreeNode<T>[], id: string, levelChanged: boolean) => {
    const after = flatten(next);
    const self = after.find((n) => n.id === id);
    if (!self) return "";
    if (unordered) {
      return self.parentId === null
        ? t("sortable.nowRoot", { label: nameOf(id) })
        : t("sortable.nowUnder", { label: nameOf(id), parent: nameOf(self.parentId) });
    }
    const count = after.filter((n) => n.parentId === self.parentId).length;
    const values = { label: nameOf(id), position: self.index + 1, count };
    if (!levelChanged) return t("sortable.moved", values);
    return self.parentId === null
      ? t("sortable.movedRoot", values)
      : t("sortable.movedUnder", { ...values, parent: nameOf(self.parentId) });
  };

  const commit = (next: TreeNode<T>[], id: string) => {
    if (next === nodes) return;
    const before = flat.find((n) => n.id === id);
    const after = flatten(next).find((n) => n.id === id);
    if (!before || !after) return;
    if (before.parentId === after.parentId && (unordered || before.index === after.index)) {
      return;
    }
    setStatus(describe(next, id, before.parentId !== after.parentId));
    onChange(next, { id, parentId: after.parentId, index: after.index });
  };

  const reset = () => {
    stopPointer.current?.();
    stopPointer.current = null;
    settle();
    setActiveId(null);
    setOverId(null);
    setOffsetX(0);
  };

  const announcements: Announcements = {
    onDragStart: ({ active }) => {
      lastOver.current = active.id;
      return t("sortable.picked", {
        label: nameOf(active.id),
        position: ids.indexOf(String(active.id)) + 1,
        count: ids.length,
      });
    },
    // Only when the target changes: dnd-kit also reports the start position.
    onDragOver: ({ active, over }) => {
      if (!over || over.id === lastOver.current) return undefined;
      lastOver.current = over.id;
      return t("sortable.over", {
        label: nameOf(active.id),
        position: ids.indexOf(String(over.id)) + 1,
        count: ids.length,
      });
    },
    onDragEnd: ({ active }) => {
      if (!projection) return undefined;
      if (unordered) {
        const label = nameOf(active.id);
        const before = flat.find((n) => n.id === String(active.id));
        if (before?.parentId === projection.parentId) return t("sortable.stayed", { label });
        return projection.parentId === null
          ? t("sortable.nowRoot", { label })
          : t("sortable.nowUnder", { label, parent: nameOf(projection.parentId) });
      }
      const values = {
        label: nameOf(active.id),
        position: projection.index + 1,
        count:
          flat.filter((n) => n.parentId === projection.parentId && n.id !== active.id).length + 1,
      };
      return projection.parentId === null
        ? t("sortable.movedRoot", values)
        : t("sortable.movedUnder", { ...values, parent: nameOf(projection.parentId) });
    },
    onDragCancel: ({ active }) =>
      t("sortable.cancelled", {
        label: nameOf(active.id),
        position: ids.indexOf(String(active.id)) + 1,
        count: ids.length,
      }),
  };

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      accessibility={{
        announcements,
        screenReaderInstructions: {
          draggable: unordered
            ? t("sortable.unorderedTreeInstructions")
            : t("sortable.treeInstructions"),
        },
      }}
      onDragStart={({ active, activatorEvent }: DragStartEvent) => {
        followPointer(activatorEvent);
        setActiveId(String(active.id));
        setOverId(String(active.id));
      }}
      onDragOver={({ over }: DragOverEvent) => setOverId(over ? String(over.id) : null)}
      onDragEnd={() => {
        if (activeId !== null && projection) {
          commit(
            moveNode(nodes, activeId, {
              parentId: projection.parentId,
              index: projection.index,
            }),
            activeId,
          );
        }
        reset();
      }}
      onDragCancel={reset}
    >
      <SortableContext items={ids} strategy={verticalListSortingStrategy}>
        <ul
          ref={list}
          aria-label={name}
          data-slot="sortable-tree"
          className={cn("flex flex-col gap-1", className)}
        >
          {visible.map((node) => {
            const active = node.id === activeId;
            const depth = active && projection ? projection.depth : node.depth;
            const siblings = flat.filter((n) => n.parentId === node.parentId).length;
            const prev = flat.find(
              (n) => n.parentId === node.parentId && n.index === node.index - 1,
            );
            const canIndent =
              prev !== undefined &&
              node.depth + 1 + subtreeHeight(flat, node.id) <= maxDepth &&
              nestable(prev.id);
            return (
              <SortableRow
                key={node.id}
                id={node.id}
                label={label(node.data)}
                layout={motionOn}
                container={list}
                inset={depth * INDENT_PX}
                placeholder={active}
                lockX
                rowProps={{ "aria-level": node.depth + 1 }}
                actions={
                  actions
                    ? actions(node)
                    : [
                        ...(leadingActions?.(node) ?? []),
                        {
                          key: "up",
                          label: t("sortable.moveUp"),
                          icon: ArrowUpIcon,
                          disabled: node.index === 0,
                          run: () =>
                            commit(
                              moveNode(nodes, node.id, {
                                parentId: node.parentId,
                                index: node.index - 1,
                              }),
                              node.id,
                            ),
                        },
                        {
                          key: "down",
                          label: t("sortable.moveDown"),
                          icon: ArrowDownIcon,
                          disabled: node.index === siblings - 1,
                          run: () =>
                            commit(
                              moveNode(nodes, node.id, {
                                parentId: node.parentId,
                                index: node.index + 1,
                              }),
                              node.id,
                            ),
                        },
                        {
                          key: "indent",
                          label: t("sortable.indent"),
                          icon: IndentIcon,
                          disabled: !canIndent,
                          separated: true,
                          run: () => commit(indent(nodes, node.id), node.id),
                        },
                        {
                          key: "outdent",
                          label: t("sortable.outdent"),
                          icon: OutdentIcon,
                          disabled: node.parentId === null,
                          run: () => commit(outdent(nodes, node.id), node.id),
                        },
                        ...(trailingActions?.(node) ?? []),
                      ]
                }
              >
                {render(node)}
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
