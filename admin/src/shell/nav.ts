// The sidebar's sections (and the palette's "Go to", "Create" and "Recent" groups). An item
// shows only when `me.capabilities` holds what its API needs; the server still
// enforces every call, the SPA only hides (constraints: capabilities in core).
// Sections for Media, Comments, Appearance, Plugins, Users, Settings and System are
// added when their screens exist (M2b-2/3).

import {
  FileTextIcon,
  FilesIcon,
  LayoutDashboardIcon,
  ShapesIcon,
  type LucideIcon,
} from "lucide-react";
import type { ContentTypeDto } from "@/api/types";
import type { MeDto } from "@/auth/session";
import type { TFunction } from "i18next";
import { i18n } from "@/i18n";

export interface NavItem {
  id: string;
  label: string;
  /** An admin path under the router's `/admin` base (`/`, `/content/post`). */
  to: string;
  icon: LucideIcon;
  /** The content type a content item lists (`post`); the palette searches these. */
  contentType?: string;
  /** A `shortcuts.ts` key string (`g d`); registered by the shell. */
  shortcut?: string;
  badge?: number;
}

export interface NavSection {
  id: string;
  label: string;
  items: NavItem[];
}

/** Types the content screens list: everything but `media` (its own screen, M2b-2). */
const NOT_CONTENT = new Set(["media"]);
const BUILT_IN_ORDER = ["post", "page"];
const TYPE_ICONS: Record<string, LucideIcon> = { post: FileTextIcon, page: FilesIcon };
/** 11 §6: `g p` Posts, `g g` Pages (the lists exist from Task 13). */
const TYPE_SHORTCUTS: Record<string, string> = { post: "g p", page: "g g" };

/** `t` defaults to the shared instance; the shell passes its hook's `t` (language changes). */
export function navFor(
  me: MeDto,
  types: ContentTypeDto[],
  t: TFunction<"shell"> = i18n.getFixedT(null, "shell"),
): NavSection[] {
  const can = (cap: string) => me.capabilities.includes(cap);
  const sections: NavSection[] = [
    {
      id: "overview",
      label: t("nav.overview"),
      items: [
        {
          id: "dashboard",
          label: t("nav.dashboard"),
          to: "/",
          icon: LayoutDashboardIcon,
          shortcut: "g d",
        },
      ],
    },
  ];

  if (can("read_admin")) {
    const shown = types.filter((ty) => !NOT_CONTENT.has(ty.name));
    const builtIn = BUILT_IN_ORDER.flatMap((name) => shown.filter((ty) => ty.name === name));
    const custom = shown
      .filter((ty) => !BUILT_IN_ORDER.includes(ty.name))
      .sort((a, b) => a.label.localeCompare(b.label));
    const items = [...builtIn, ...custom].map((t): NavItem => {
      const shortcut = TYPE_SHORTCUTS[t.name];
      return {
        id: `content-${t.name}`,
        label: t.label,
        to: `/content/${t.name}`,
        icon: TYPE_ICONS[t.name] ?? ShapesIcon,
        contentType: t.name,
        ...(shortcut ? { shortcut } : {}),
      };
    });
    if (items.length > 0) {
      sections.push({ id: "content", label: t("nav.content"), items });
    }
  }
  return sections;
}

/** The nav item whose screen `pathname` (an admin path, no `/admin` base) shows. */
export function navItemAt(sections: NavSection[], pathname: string): NavItem | undefined {
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  return sections.flatMap((s) => s.items).find((item) => item.to === (path || "/"));
}
