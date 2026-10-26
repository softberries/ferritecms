import { useCallback, useRef, useState, type ReactNode } from "react";
import type { StaleChoice } from "@/lib/stale-guard";
import { StaleDialog } from "./stale-dialog";

/** `ask` for `staleGuard` plus the dialog element to render: `const { ask, dialog } = …`. */
export function useStaleAsk(resource: string): {
  ask: () => Promise<StaleChoice>;
  dialog: ReactNode;
} {
  const [open, setOpen] = useState(false);
  const resolver = useRef<((choice: StaleChoice) => void) | null>(null);
  const ask = useCallback(
    () =>
      new Promise<StaleChoice>((resolve) => {
        resolver.current = resolve;
        setOpen(true);
      }),
    [],
  );
  const dialog = (
    <StaleDialog
      open={open}
      resource={resource}
      onChoose={(choice) => {
        setOpen(false);
        resolver.current?.(choice);
        resolver.current = null;
      }}
    />
  );
  return { ask, dialog };
}
