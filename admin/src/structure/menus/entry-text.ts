// What a menu editor row says (`menu-editor.tsx` `MenuRow`): its first line (the entry's
// own label, else what the site shows instead, muted) and the line under it (where it
// points). Without a label the first line already shows the target's title or the
// address, so the line under it says only the kind: the row never repeats itself.

import type { TFunction } from "i18next";
import type { MenuNode } from "./menu-model";
import type { Target } from "./targets";

type T = TFunction<"structure">;

export interface EntryText {
  /** The first line. */
  title: string;
  /** `true` when `title` is not the entry's own label. */
  fallback: boolean;
  /** The line under it. */
  line: string;
}

/** Where the entry points: the address, or the target's type and title, or its state. */
export function targetText(node: MenuNode, target: Target | undefined, t: T): string {
  if (node.data.kind === "url") return node.data.url ?? "";
  if (target?.state === "found")
    return `${target.kind} · ${target.title || t("menus.add.untitled")}`;
  if (target?.state === "missing") return t("menus.editor.targetMissing");
  if (target?.state === "failed") return t("menus.editor.targetFailed");
  return t("menus.editor.targetLoading");
}

export function entryText(node: MenuNode, target: Target | undefined, t: T): EntryText {
  const own = node.data.label.trim();
  const kind = t(`menus.editor.kind.${node.data.kind}`);
  const found = target?.state === "found" ? target : null;
  const sub = targetText(node, target, t);
  if (own) {
    // A found target names its own type or taxonomy ("Pages · About"); the rest say the kind.
    return { title: own, fallback: false, line: found ? sub : `${kind} · ${sub}` };
  }
  if (node.data.kind === "url") return { title: node.data.url ?? "", fallback: true, line: kind };
  if (found) {
    return { title: found.title || t("menus.add.untitled"), fallback: true, line: found.kind };
  }
  return { title: sub, fallback: true, line: kind };
}
