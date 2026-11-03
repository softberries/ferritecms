// What the webhook screens derive from the API's rows: the URL's host, the events
// summary, the catalogue by group, and which endpoints the banner names.

import type { TFunction } from "i18next";
import type { EventCatalogueDto, WebhookDto } from "@/api/webhooks";

export type EventType = EventCatalogueDto["types"][number];

/** The host of `url` (the list shows it; the full URL is in the tooltip), else `url`. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/** The catalogue's groups in its order, each with its types. */
export function groupCatalogue(
  types: readonly EventType[],
): { group: string; types: EventType[] }[] {
  const out: { group: string; types: EventType[] }[] = [];
  for (const t of types) {
    const found = out.find((g) => g.group === t.group);
    if (found) found.types.push(t);
    else out.push({ group: t.group, types: [t] });
  }
  return out;
}

const KNOWN_GROUPS = ["content", "media", "comment", "user", "import", "health"] as const;
type KnownGroup = (typeof KNOWN_GROUPS)[number];
const isKnownGroup = (g: string): g is KnownGroup =>
  (KNOWN_GROUPS as readonly string[]).includes(g);

/** A group's label; an unknown group (a newer server) as it is. */
export function groupLabel(t: TFunction<"webhooks">, group: string): string {
  return isKnownGroup(group) ? t(`events.groups.${group}`) : group;
}

/** The group of a type or a pattern (`content.published`, `content.*`). */
export const groupOf = (pattern: string): string => pattern.split(".")[0] ?? pattern;

/** "All events", or per group "Content (all)" / "Content (3)", joined. */
export function eventsSummary(t: TFunction<"webhooks">, events: readonly string[]): string {
  if (events.includes("*")) return t("events.all");
  const groups: string[] = [];
  for (const p of events) {
    const g = groupOf(p);
    if (!groups.includes(g)) groups.push(g);
  }
  return groups
    .map((g) =>
      events.includes(`${g}.*`)
        ? t("events.group", { group: groupLabel(t, g) })
        : t("events.groupCount", {
            group: groupLabel(t, g),
            count: events.filter((p) => groupOf(p) === g).length,
          }),
    )
    .join(", ");
}

/** Whether the patterns subscribe to any `content.*` event. */
export function hasContentEvents(events: readonly string[]): boolean {
  return events.some((p) => p === "*" || groupOf(p) === "content");
}

/** Whether `type` is chosen by `patterns` (itself, its group, or `*`). */
export function covers(patterns: readonly string[], type: string): boolean {
  return (
    patterns.includes("*") || patterns.includes(type) || patterns.includes(`${groupOf(type)}.*`)
  );
}

/** Endpoints disabled automatically (`failing`, `gone`) and paused ones, for the banner. */
export function needsAttention(list: readonly WebhookDto[]): { disabled: number; paused: number } {
  return {
    disabled: list.filter(
      (w) =>
        w.state === "disabled" && (w.disabled_reason === "failing" || w.disabled_reason === "gone"),
    ).length,
    paused: list.filter((w) => w.state === "paused").length,
  };
}

/** A `datetime-local` value (local time, no seconds) as RFC 3339 UTC, else `null`. */
export function localToRfc3339(value: string): string | null {
  if (!value) return null;
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

export interface WebhookDraft {
  name: string;
  url: string;
  events: string[];
  /** `[]`: every type (sent as `null`). */
  contentTypes: string[];
}

export type DraftErrors = Partial<
  Record<"name" | "url" | "events" | "content_types" | "secret", string>
>;

/** The form's own checks (the server checks again). */
export function checkDraft(
  t: (k: "form.nameRequired" | "form.urlRequired" | "form.eventsRequired") => string,
  d: WebhookDraft,
): DraftErrors {
  const out: DraftErrors = {};
  if (d.name.trim() === "") out.name = t("form.nameRequired");
  if (d.url.trim() === "") out.url = t("form.urlRequired");
  if (d.events.length === 0) out.events = t("form.eventsRequired");
  return out;
}

/** The content-type filter as the API takes it: `null` for every type, and none when no
 * content event is chosen. */
export function contentTypesOf(d: WebhookDraft): string[] | null {
  return hasContentEvents(d.events) && d.contentTypes.length > 0 ? d.contentTypes : null;
}
