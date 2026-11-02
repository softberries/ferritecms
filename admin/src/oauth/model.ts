// What the AI-assistant screens share that is not a component: the guide's address,
// a client's monogram, the redirect check, a grant's permissions in words and each
// client's setup snippets.

import type { TFunction } from "i18next";
import { uiLocale } from "@/lib/locale";
import { anyWrite, areasOf, levelsOf } from "./scope-levels";

/** The guide on the docs site (12 §9). */
export const AI_GUIDE_URL = "https://softberries.github.io/ferritecms/guides/ai-assistants/";

/** The first letter of `name` (a whole code point), upper-cased; `?` for an empty name. */
export function monogram(name: string): string {
  const first = Array.from(name.trim())[0];
  return first ? first.toLocaleUpperCase() : "?";
}

/** Schemes the browser is never sent to, whatever the server answered. */
const REFUSED_SCHEMES = new Set(["javascript:", "data:", "vbscript:", "file:", "blob:"]);

/** `to` when it is an absolute URL the browser may be sent to, else `null`. */
export function safeRedirect(to: string): string | null {
  try {
    return REFUSED_SCHEMES.has(new URL(to).protocol) ? null : to;
  } catch {
    return null;
  }
}

/** "a, b and c" in the UI language. */
export function listOf(items: string[]): string {
  return new Intl.ListFormat(uiLocale(), { style: "long", type: "conjunction" }).format(items);
}

/** "Read only · 3 areas" or "Writes Content and Media · 5 areas". */
export function permissionSummary(scopes: readonly string[], t: TFunction<"ai">): string {
  const levels = levelsOf(scopes);
  const areas = areasOf(levels);
  const count = t("apps.areas", { count: areas.length });
  if (!anyWrite(levels)) return `${t("apps.readOnly")} · ${count}`;
  const writes = areas.filter((a) => levels[a] === "write").map((a) => t(`areas.${a}.label`));
  return `${t("apps.writeAreas", { areas: listOf(writes) })} · ${count}`;
}

/** The clients Settings → AI assistants has steps for, in tab order. */
export const AI_CLIENTS = ["claude", "chatgpt", "claudeCode", "cursor", "vscode"] as const;
export type AiClient = (typeof AI_CLIENTS)[number];

/** The command lines and config files each client takes, for the MCP URL `url`. */
export function clientSnippets(url: string) {
  return {
    claudeCode: `claude mcp add --transport http ferrite ${url}`,
    claudeCodeToken: `claude mcp add --transport http ferrite ${url} --header "Authorization: Bearer <token>"`,
    cursor: JSON.stringify({ mcpServers: { ferrite: { url } } }, null, 2),
    vscode: JSON.stringify({ servers: { ferrite: { type: "http", url } } }, null, 2),
  };
}
