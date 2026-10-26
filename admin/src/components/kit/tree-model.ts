// The tree behind `SortableTree` (menus, hierarchical terms and pages), as pure functions
// over a flat list of nodes. A node's siblings are the nodes with the same `parentId`, in
// array order; a node whose parent is missing counts as a root. Every change returns a
// new array, or the input itself when the change is refused, so callers can compare by
// reference.

export interface TreeNode<T> {
  id: string;
  parentId: string | null;
  data: T;
}

export type FlatNode<T> = TreeNode<T> & {
  depth: number;
  /** The position among its siblings. */
  index: number;
};

export interface TreePosition {
  parentId: string | null;
  /** Among the new parent's children, counted without the moved node. */
  index: number;
}

export interface DropProjection extends TreePosition {
  depth: number;
}

/** Depth-first, siblings in array order. Cycles (bad data) are cut, never looped. */
export function flatten<T>(nodes: TreeNode<T>[]): FlatNode<T>[] {
  const ids = new Set(nodes.map((n) => n.id));
  const children = new Map<string | null, TreeNode<T>[]>();
  for (const n of nodes) {
    const parent = n.parentId !== null && ids.has(n.parentId) ? n.parentId : null;
    const list = children.get(parent);
    if (list) list.push(n);
    else children.set(parent, [n]);
  }
  const out: FlatNode<T>[] = [];
  const seen = new Set<string>();
  const walk = (parent: string | null, depth: number) => {
    (children.get(parent) ?? []).forEach((n, index) => {
      if (seen.has(n.id)) return;
      seen.add(n.id);
      out.push({ ...n, parentId: parent, depth, index });
      walk(n.id, depth + 1);
    });
  };
  walk(null, 0);
  return out;
}

/** `id` and every node below it. */
export function subtreeIds<T>(nodes: TreeNode<T>[], id: string): Set<string> {
  const out = new Set<string>([id]);
  // Parents before children is not guaranteed by the array, so repeat until stable.
  let grew = true;
  while (grew) {
    grew = false;
    for (const n of nodes) {
      if (n.parentId !== null && out.has(n.parentId) && !out.has(n.id)) {
        out.add(n.id);
        grew = true;
      }
    }
  }
  return out;
}

/** How many levels sit below `id` in `flat` (0 for a leaf). */
export function subtreeHeight<T>(flat: FlatNode<T>[], id: string): number {
  const at = flat.findIndex((n) => n.id === id);
  const self = flat[at];
  if (!self) return 0;
  let height = 0;
  for (let i = at + 1; i < flat.length; i++) {
    const n = flat[i];
    if (!n || n.depth <= self.depth) break;
    height = Math.max(height, n.depth - self.depth);
  }
  return height;
}

/**
 * Moves `id` (with its subtree) to `to`. Refused (the input comes back) for an unknown
 * node or parent, and for a parent inside the node's own subtree.
 */
export function moveNode<T>(nodes: TreeNode<T>[], id: string, to: TreePosition): TreeNode<T>[] {
  const node = nodes.find((n) => n.id === id);
  if (!node) return nodes;
  if (to.parentId !== null) {
    if (!nodes.some((n) => n.id === to.parentId)) return nodes;
    if (subtreeIds(nodes, id).has(to.parentId)) return nodes;
  }
  const rest = nodes.filter((n) => n.id !== id);
  const siblings = flatten(nodes).filter((n) => n.parentId === to.parentId && n.id !== id);
  const moved = { ...node, parentId: to.parentId };
  const before = siblings[Math.max(0, to.index)];
  if (before) {
    const at = rest.findIndex((n) => n.id === before.id);
    return [...rest.slice(0, at), moved, ...rest.slice(at)];
  }
  const last = siblings.at(-1);
  if (!last) return [...rest, moved];
  const at = rest.findIndex((n) => n.id === last.id);
  return [...rest.slice(0, at + 1), moved, ...rest.slice(at + 1)];
}

/** The last child of its previous sibling; a first child (or unknown id) stays put. */
export function indent<T>(nodes: TreeNode<T>[], id: string): TreeNode<T>[] {
  const flat = flatten(nodes);
  const self = flat.find((n) => n.id === id);
  if (!self || self.index === 0) return nodes;
  const prev = flat.find((n) => n.parentId === self.parentId && n.index === self.index - 1);
  if (!prev) return nodes;
  const count = flat.filter((n) => n.parentId === prev.id).length;
  return moveNode(nodes, id, { parentId: prev.id, index: count });
}

/** The next sibling of its parent; a root (or unknown id) stays put. */
export function outdent<T>(nodes: TreeNode<T>[], id: string): TreeNode<T>[] {
  const flat = flatten(nodes);
  const self = flat.find((n) => n.id === id);
  if (!self || self.parentId === null) return nodes;
  const parent = flat.find((n) => n.id === self.parentId);
  if (!parent) return nodes;
  return moveNode(nodes, id, { parentId: parent.parentId, index: parent.index + 1 });
}

/**
 * Where a drag of `activeId` over `overId` would land (dnd-kit's sortable-tree
 * projection). The active node's descendants travel with it, so they are left out of
 * `flat`. The depth follows the horizontal offset (`offsetX / indentPx` levels from the
 * node's own depth), clamped between the next row's depth and the previous row's depth
 * + 1, and so that the node's subtree stays within `maxDepth` (when the two disagree,
 * `maxDepth` wins). `canNest(parentId)` refuses a parent; the depth then steps back
 * toward the minimum. `null` for an unknown id.
 */
export function projectDrop<T>(
  flat: FlatNode<T>[],
  activeId: string,
  overId: string,
  offsetX: number,
  indentPx = 24,
  maxDepth = Number.POSITIVE_INFINITY,
  canNest?: (parentId: string) => boolean,
): DropProjection | null {
  const active = flat.find((n) => n.id === activeId);
  if (!active) return null;
  const height = subtreeHeight(flat, activeId);
  const below = new Set(
    flat
      .slice(flat.indexOf(active) + 1, flat.indexOf(active) + 1 + countBelow(flat, active))
      .map((n) => n.id),
  );
  const items = flat.filter((n) => !below.has(n.id));
  const from = items.indexOf(active);
  const to = items.findIndex((n) => n.id === overId);
  if (to < 0) return null;

  const moved = [...items];
  moved.splice(from, 1);
  moved.splice(to, 0, active);
  const prev = moved[to - 1];
  const next = moved[to + 1];

  const projected = active.depth + Math.round(offsetX / indentPx);
  const min = next ? next.depth : 0;
  const max = Math.max(0, Math.min(prev ? prev.depth + 1 : 0, maxDepth - height));
  let depth = Math.min(Math.max(projected, min), max);

  const parentAt = (d: number): string | null => {
    if (d === 0 || !prev) return null;
    if (d === prev.depth) return prev.parentId;
    if (d > prev.depth) return prev.id;
    return (
      moved
        .slice(0, to)
        .reverse()
        .find((n) => n.depth === d)?.parentId ?? null
    );
  };
  if (canNest) {
    const lowest = Math.min(min, depth);
    let d = depth;
    while (d > lowest) {
      const parent = parentAt(d);
      if (parent === null || canNest(parent)) break;
      d--;
    }
    depth = d;
  }
  const parentId = parentAt(depth);
  const index = moved
    .slice(0, to)
    .filter((n) => n.parentId === parentId && n.depth === depth).length;
  return { parentId, index, depth };
}

/** How many rows right after `node` in `flat` sit below it. */
function countBelow<T>(flat: FlatNode<T>[], node: FlatNode<T>): number {
  const at = flat.indexOf(node);
  let n = 0;
  while ((flat[at + 1 + n]?.depth ?? -1) > node.depth) n++;
  return n;
}
