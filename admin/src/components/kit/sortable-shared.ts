// What `SortableList` and `SortableTree` share besides their row.

import { useCallback, useState } from "react";

/** Pointer drags start after 4 px, so a click on the handle stays a click. */
export const DRAG_DISTANCE_PX = 4;

/**
 * After a drop the dragged row already sits where it lands; a layout animation from its
 * old slot would throw it back first. `settle()` turns layout motion off for the drop's
 * render and on again in the next frame.
 */
export function useSettling(): [boolean, () => void] {
  const [settling, setSettling] = useState(false);
  const settle = useCallback(() => {
    setSettling(true);
    requestAnimationFrame(() => setSettling(false));
  }, []);
  return [settling, settle];
}
