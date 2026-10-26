// What every level of the field builder shares: the errors to show, which rows are open,
// the row whose label takes the cursor when it appears, and whether editing is off.

import { createContext, useContext } from "react";

export interface BuilderContextValue {
  /** The `checkBuilder` errors to show, by `errorKey`. */
  errors: Record<string, string>;
  isOpen: (key: string) => boolean;
  setOpen: (key: string, open: boolean) => void;
  /** A row just added: its label input takes the cursor once. */
  focusKey: string | null;
  setFocusKey: (key: string | null) => void;
  disabled: boolean;
}

export const BuilderContext = createContext<BuilderContextValue | null>(null);

export function useBuilder(): BuilderContextValue {
  const value = useContext(BuilderContext);
  if (!value) throw new Error("useBuilder outside a field builder");
  return value;
}
