// "Upload" from the ⌘K palette: the palette asks for the file chooser and goes to the
// library, which opens it on arrival (or at once when it is already open). The request
// is one-shot and module-level, since the palette and the library never share a tree.

import { useEffect, useRef } from "react";

let pending = false;
const listeners = new Set<() => void>();

/** Asks the media library to open its file chooser. */
export function requestUpload(): void {
  pending = true;
  for (const fn of listeners) fn();
}

/** Calls `open` for every upload request while mounted (one waiting from before too). */
export function useUploadRequest(open: () => void, enabled: boolean): void {
  const latest = useRef(open);
  useEffect(() => {
    latest.current = open;
  });
  useEffect(() => {
    if (!enabled) return;
    const take = () => {
      if (!pending) return;
      pending = false;
      latest.current();
    };
    listeners.add(take);
    take();
    return () => {
      listeners.delete(take);
    };
  }, [enabled]);
}
