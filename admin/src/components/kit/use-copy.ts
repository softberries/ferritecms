// Copying a value that is not secret, with the outcome as state ("copied" for 1.5 s).

import { useCallback, useEffect, useState } from "react";

export type CopyState = "idle" | "copied" | "failed";

/** Copies `value`; the state goes back to `idle` 1.5 s after a success. */
export function useCopy(value: string): { state: CopyState; copy: () => void } {
  const [state, setState] = useState<CopyState>("idle");
  useEffect(() => {
    if (state !== "copied") return;
    const timer = window.setTimeout(() => setState("idle"), 1_500);
    return () => window.clearTimeout(timer);
  }, [state]);
  const copy = useCallback(() => {
    navigator.clipboard.writeText(value).then(
      () => setState("copied"),
      () => setState("failed"),
    );
  }, [value]);
  return { state, copy };
}
