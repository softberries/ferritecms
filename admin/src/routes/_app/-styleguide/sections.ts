// The styleguide's sections, in page order (also the `?section=` values).

export const SECTIONS = [
  "colour",
  "type",
  "layout",
  "motion",
  "controls",
  "display",
  "navigation",
  "overlays",
  "patterns",
  "ordering",
  "data",
  "charts",
  "administration",
] as const;

export type SectionId = (typeof SECTIONS)[number];

export function isSection(value: unknown): value is SectionId {
  return typeof value === "string" && (SECTIONS as readonly string[]).includes(value);
}
