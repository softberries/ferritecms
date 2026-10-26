// Leaving with unsaved changes asks first (07 §3, Review Focus 3). Router navigation
// (a link, Back, the palette) is held by TanStack Router's `useBlocker` while the item
// does not hold the changes (`dirty`): the editor shows Save / Discard / Stay. Closing or
// reloading the tab asks through `beforeunload` only while neither the item nor the
// autosave slot holds them (`unload`, F38): once the slot has the draft, a reload brings
// it back through the restore banner, so the browser's prompt would only be in the way.
// Navigation that must pass (the 401 redirect, the hand-over after a first save, the
// trash) goes with `ignoreBlocker: true` (F37).

import { useBlocker } from "@tanstack/react-router";
import { useEffect, useRef } from "react";

export interface LeaveGuard {
  /** A navigation waits for the user's answer. */
  blocked: boolean;
  /** Lets the waiting navigation go on. */
  leave: () => void;
  /** Cancels the waiting navigation. */
  stay: () => void;
}

export interface LeaveGuardOptions {
  /** Whether closing or reloading the tab asks first; defaults to `dirty`. */
  unload?: boolean;
}

export function useLeaveGuard(dirty: boolean, options: LeaveGuardOptions = {}): LeaveGuard {
  const unload = options.unload ?? dirty;
  const state = useRef({ dirty, unload });
  useEffect(() => {
    state.current = { dirty, unload };
  }, [dirty, unload]);
  const blocker = useBlocker({
    // A change of the search or hash of this same screen is not leaving it.
    shouldBlockFn: ({ current, next }) => state.current.dirty && current.pathname !== next.pathname,
    enableBeforeUnload: () => state.current.unload,
    withResolver: true,
  });
  return {
    blocked: blocker.status === "blocked",
    leave: () => blocker.proceed?.(),
    stay: () => blocker.reset?.(),
  };
}
