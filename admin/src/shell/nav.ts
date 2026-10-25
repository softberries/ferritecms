// The sidebar's sections (and, from Task 11, the palette's "Go to" group). An item
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
import { i18n } from "@/i18n";

export interface NavItem {
  id: string;
  label: string;
  /** An admin path under the router's `/admin` base (`/`, `/content/post`). */
  to: string;
  icon: LucideIcon;
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

export function navFor(me: MeDto, types: ContentTypeDto[]): NavSection[] {
  const can = (cap: string) => me.capabilities.includes(cap);
  const sections: NavSection[] = [
    {
      id: "overview",
      label: i18n.t("shell:nav.overview"),
      items: [
        {
          id: "dashboard",
          label: i18n.t("shell:nav.dashboard"),
          to: "/",
          icon: LayoutDashboardIcon,
          shortcut: "g d",
        },
      ],
    },
  ];

  if (can("read_admin")) {
    const shown = types.filter((t) => !NOT_CONTENT.has(t.name));
    const builtIn = BUILT_IN_ORDER.flatMap((name) => shown.filter((t) => t.name === name));
    const custom = shown
      .filter((t) => !BUILT_IN_ORDER.includes(t.name))
      .sort((a, b) => a.label.localeCompare(b.label));
    const items = [...builtIn, ...custom].map((t): NavItem => ({
      id: `content-${t.name}`,
      label: t.label,
      to: `/content/${t.name}`,
      icon: TYPE_ICONS[t.name] ?? ShapesIcon,
    }));
    if (items.length > 0) {
      sections.push({ id: "content", label: i18n.t("shell:nav.content"), items });
    }
  }
  return sections;
}
