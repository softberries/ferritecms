// Focus after an overlay opened from a tree row's menu closes: back on that row's menu
// button (`SortableRow`'s `data-row-menu`), where the keyboard user was. When the row is
// gone (the term was deleted) the overlay's own focus return applies.

/** Handler for an overlay's `onCloseAutoFocus`: refocuses row `id`'s menu button. */
export function returnToRowMenu(id: string | null): (e: Event) => void {
  return (e) => {
    if (id === null) return;
    const button = document.querySelector<HTMLElement>(`[data-row-menu="${CSS.escape(id)}"]`);
    if (!button) return;
    e.preventDefault();
    button.focus();
  };
}
