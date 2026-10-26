// The upload queue of image files dropped on an item's body. It lives with the route,
// above the editor screen, because the screen remounts while uploads run: the first
// save of a new item moves it from `/new` to `/$id` (the queue goes with the hand-over),
// and every reopen on `/$id` (Take theirs, Discard, a revision restore and its Undo, a
// backup restore, "changed elsewhere") starts a new screen. The next screen shows the
// running uploads again (`useImageDrops`), so an upload is never cut off by a remount
// (final review A1). Leaving the route cancels what still runs; the editor's leave
// guard asks first.

import { useEffect, useRef, useState } from "react";
import { uploadMedia } from "@/api/upload";
import { createUploadQueue, type UploadQueue } from "@/media/upload-queue";
import { UPLOAD_CONCURRENCY } from "@/media/use-upload-queue";

/** What a new item's `/new` screen hands to its `/$id` screen with the running uploads. */
export interface CarriedUploads {
  queue: UploadQueue;
  /**
   * Where each running upload's placeholder was (the body is handed over unchanged);
   * `null`: its place was deleted meanwhile, so the image is not inserted.
   */
  at: Record<string, number | null>;
}

export function newBodyUploadQueue(): UploadQueue {
  return createUploadQueue({
    upload: uploadMedia,
    concurrency: UPLOAD_CONCURRENCY,
    onDone: () => undefined,
  });
}

/**
 * The route's queue: `adopted` (from a hand-over) or a new one. It is cancelled when the
 * route goes, unless `keep` was called (the queue was handed to the next route).
 */
export function useBodyUploads(adopted: UploadQueue | null): {
  queue: UploadQueue;
  keep: () => void;
} {
  const [queue] = useState(() => adopted ?? newBodyUploadQueue());
  const kept = useRef(false);
  const mounted = useRef(0);
  useEffect(() => {
    mounted.current += 1;
    return () => {
      mounted.current -= 1;
      // After StrictMode's simulated unmount the effect runs again at once: only a real
      // unmount cancels.
      queueMicrotask(() => {
        if (mounted.current === 0 && !kept.current) queue.dispose();
      });
    };
  }, [queue]);
  return {
    queue,
    keep: () => {
      kept.current = true;
    },
  };
}
