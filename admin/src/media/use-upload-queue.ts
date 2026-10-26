// The library's upload queue as React state: one queue per screen (cancelled when the
// screen goes, `dispose`), its rows through `useSyncExternalStore`, and `onDone` for
// every finished upload (the latest callback, without re-creating the queue).

import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import type { MediaDto } from "@/api/media";
import { uploadMedia } from "@/api/upload";
import { type UploadEntry, type UploadQueue, createUploadQueue } from "./upload-queue";

/** At most this many uploads run at once from one tab (Global Constraints). */
export const UPLOAD_CONCURRENCY = 3;

export function useUploadQueue(onDone: (media: MediaDto) => void): {
  queue: UploadQueue;
  entries: readonly UploadEntry[];
} {
  const [{ queue, listeners }] = useState(() => {
    const listeners = new Set<(media: MediaDto) => void>();
    const queue = createUploadQueue({
      upload: uploadMedia,
      concurrency: UPLOAD_CONCURRENCY,
      onDone: (media) => {
        for (const fn of listeners) fn(media);
      },
    });
    return { queue, listeners };
  });
  useEffect(() => {
    listeners.add(onDone);
    return () => {
      listeners.delete(onDone);
    };
  }, [listeners, onDone]);
  useEffect(() => () => queue.dispose(), [queue]);
  const subscribe = useCallback((fn: () => void) => queue.subscribe(fn), [queue]);
  const snapshot = useCallback(() => queue.entries(), [queue]);
  const entries = useSyncExternalStore(subscribe, snapshot);
  return { queue, entries };
}
