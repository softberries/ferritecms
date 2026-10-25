// The locale for numbers and dates: the document's `lang` (set by the admin's
// index.html), else the browser's.

export function uiLocale(): string {
  if (typeof document !== "undefined" && document.documentElement.lang) {
    return document.documentElement.lang;
  }
  return typeof navigator !== "undefined" ? navigator.language : "en";
}
