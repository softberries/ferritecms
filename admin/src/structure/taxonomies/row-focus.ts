// Focus after an overlay opened from a term's row closes: back on that row's menu button
// (`SortableRow`'s `data-row-menu`, or the flat table's delete button,
// `data-term-delete`), where the keyboard user was. When the row is gone (the term was
// deleted) focus goes to the row now in its place, or the one before
// (`neighbourTarget`, taken before the delete), never to `body`.

/** A term row's own focus target: its menu button (tree) or delete button (table). */
const ROW_TARGETS = "[data-row-menu], [data-term-delete]";

function targetOf(id: string): string {
  const v = CSS.escape(id);
  return `[data-row-menu="${v}"], [data-term-delete="${v}"]`;
}

/**
 * The focus target of the row after term `id`'s row (else the one before), as a
 * selector, from the page as it is now; `null` when it is the only row or not shown.
 */
export function neighbourTarget(id: string): string | null {
  const all = Array.from(document.querySelectorAll<HTMLElement>(ROW_TARGETS));
  const at = all.findIndex((el) => (el.dataset.rowMenu ?? el.dataset.termDelete) === id);
  if (at < 0) return null;
  const next = all[at + 1] ?? all[at - 1];
  const nextId = next?.dataset.rowMenu ?? next?.dataset.termDelete;
  return nextId === undefined ? null : targetOf(nextId);
}

/**
 * Handler for an overlay's `onCloseAutoFocus`: focuses row `id`'s target, else the
 * element `fallback` selects; with neither on the page, the overlay's own focus return
 * applies.
 */
export function returnToRowMenu(id: string | null, fallback: string | null = null) {
  return (e: Event): void => {
    const button =
      (id === null ? null : document.querySelector<HTMLElement>(targetOf(id))) ??
      (fallback === null ? null : document.querySelector<HTMLElement>(fallback));
    if (!button) return;
    e.preventDefault();
    button.focus();
  };
}
