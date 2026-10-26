// The hand-over from a just-created item's `/new` screen to its `/$id` screen. It lives
// apart from `editor-state.ts` because the `/$id` route's loader reads it, and the loader
// is in the entry chunk: importing it from `editor-state.ts` would pull TipTap into
// every admin page.

import type { JSONContent } from "@tiptap/core";
import type { MetaDraft } from "./editor-state";

/** What a just-created item hands to its `/$id` screen (the route remounts the editor). */
export interface Handoff {
  id: string;
  title: string;
  body: JSONContent;
  selection: { from: number; to: number } | null;
  focus: "title" | "body" | null;
  /** What the first save sent (the dirty baseline of the `/$id` screen). */
  saved: { title: string; body: string };
  /** The panel members as they were (edits made while the first save was in flight). */
  meta: MetaDraft;
}

let pendingHandoff: Handoff | null = null;

/** Stashes the live editor state of a just-created item for its `/$id` screen. */
export function handOff(h: Handoff): void {
  pendingHandoff = h;
}

/** The waiting hand-over for `id`, else `null` (the `/$id` loader then skips its fetch). */
export function peekHandoff(id: string): Handoff | null {
  return pendingHandoff?.id === id ? pendingHandoff : null;
}

/** Drops the hand-over for `id` once its screen has mounted. */
export function clearHandoff(id: string): void {
  if (pendingHandoff?.id === id) pendingHandoff = null;
}
