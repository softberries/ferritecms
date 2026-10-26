// Small helpers the structure screens (types, taxonomies, menus) share.

/** The filled destructive button of a confirm (readable in both schemes). */
export const DESTRUCTIVE =
  "bg-destructive text-background hover:bg-destructive/90 dark:bg-destructive dark:hover:bg-destructive/90";

/** `errs` without the message for `key` (a field the user has just changed). */
export const without = (errs: Record<string, string>, key: string): Record<string, string> =>
  Object.fromEntries(Object.entries(errs).filter(([k]) => k !== key));
