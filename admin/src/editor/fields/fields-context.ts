// What every input of one custom-fields form shares (`FieldsSection` provides it): where
// the messages come from, what an edit clears, and whether the form is read-only.

import { createContext, useContext } from "react";

export interface FieldsContextValue {
  /** The message to show at `path` (`fields.price`), if any. */
  errorFor: (path: string) => string | undefined;
  /** The input at `path` changed (its server message goes). */
  edited: (path: string) => void;
  disabled: boolean;
}

export const FieldsContext = createContext<FieldsContextValue>({
  errorFor: () => undefined,
  edited: () => undefined,
  disabled: false,
});

export const useFieldsContext = () => useContext(FieldsContext);

/** The ids a control is labelled and described by. */
export interface ControlIds {
  id: string;
  /** The label's id (for controls a `<label for>` cannot name, such as an editor). */
  labelId: string;
  /** Hint (and error, when there is one) ids for `aria-describedby`. */
  describedBy: string;
  invalid: boolean;
}
