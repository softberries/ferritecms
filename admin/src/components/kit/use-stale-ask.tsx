import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import type { StaleChoice } from "@/lib/stale-guard";
import { StaleDialog } from "./stale-dialog";

/**
 * `ask` for `staleGuard` plus the dialog element to render: `const { ask, dialog } = …`.
 * One question at a time: a second `ask` settles a pending one with `cancel`, and so
 * does unmounting, so no caller waits forever.
 */
export function useStaleAsk(resource: string): {
  ask: () => Promise<StaleChoice>;
  dialog: ReactNode;
} {
  const [open, setOpen] = useState(false);
  const resolver = useRef<((choice: StaleChoice) => void) | null>(null);
  const settle = useCallback((choice: StaleChoice) => {
    const resolve = resolver.current;
    resolver.current = null;
    resolve?.(choice);
  }, []);
  const ask = useCallback(
    () =>
      new Promise<StaleChoice>((resolve) => {
        settle("cancel");
        resolver.current = resolve;
        setOpen(true);
      }),
    [settle],
  );
  useEffect(
    () => () => {
      settle("cancel");
    },
    [settle],
  );
  const dialog = (
    <StaleDialog
      open={open}
      resource={resource}
      onChoose={(choice) => {
        setOpen(false);
        settle(choice);
      }}
    />
  );
  return { ask, dialog };
}
