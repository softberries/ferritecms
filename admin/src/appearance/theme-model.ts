// The Themes screen's rules that are not components (`themes-screen.tsx`, `theme-card.tsx`).

import type { ThemeDto } from "@/api/themes";

/** After an activation, node state is read again this often, for this long (exact values). */
export const NODE_POLL_MS = 2_000;
export const NODE_POLL_FOR_MS = 20_000;

/** Whether every live node reports serving `name` without a fallback. */
export function nodesSettled(themes: readonly ThemeDto[] | undefined, name: string): boolean {
  const active = themes?.find((th) => th.active);
  return (
    active?.name === name &&
    active.nodes.length > 0 &&
    active.nodes.every((n) => n.name === name && !n.fallback)
  );
}

/** `href` when it is an `http:`/`https:` URL, else `null` (then shown as text). */
export function safeHomepage(href: string | null | undefined): string | null {
  if (!href) return null;
  try {
    const url = new URL(href);
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
}
