// The sidebar's sections (and the palette's "Go to", "Create" and "Recent" groups). An item
// shows only when `me.capabilities` holds what its API needs; the server still
// enforces every call, the SPA only hides (constraints: capabilities in core).
// M2b-2 screens (Media, Comments, Structure) each register a `ScreenEntry` in
// `SCREEN_ENTRIES` when their route exists (ruling F-A); who sees an entry, where it goes
// and its chord are fixed here in `SCREENS`. Appearance, Plugins, Users, Settings and
// System follow in M2b-3.

import {
  FileTextIcon,
  FilesIcon,
  LayoutDashboardIcon,
  ShapesIcon,
  type LucideIcon,
} from "lucide-react";
import type { ContentTypeDto } from "@/api/types";
import type { MeDto } from "@/auth/session";
import type { ParseKeys, TFunction } from "i18next";
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

type Can = (cap: string) => boolean;

/** The M2b-2 screens a task can register, with the rule for who sees each. */
const SCREENS = {
  // Contributors too: the screen is read-only without `upload_media` (it decides).
  media: { section: "content", show: (can: Can) => can("read_admin"), shortcut: "g m" },
  comments: {
    section: "content",
    show: (can: Can) => can("moderate_comments"),
    shortcut: "g c",
  },
  types: { section: "structure", show: (can: Can) => can("manage_settings") },
  // `manage_terms` alone manages terms, not the taxonomies themselves (the screen decides).
  taxonomies: {
    section: "structure",
    show: (can: Can) => can("manage_settings") || can("manage_terms"),
  },
  menus: { section: "structure", show: (can: Can) => can("manage_menus") },
  redirects: { section: "structure", show: (can: Can) => can("manage_redirects") },
} as const satisfies Record<
  string,
  { section: "content" | "structure"; show: (can: Can) => boolean; shortcut?: string }
>;

export type ScreenId = keyof typeof SCREENS;

/** What a screen task registers once its route exists. */
export interface ScreenEntry {
  screen: ScreenId;
  /** The screen's admin path (`/media`). */
  to: string;
  /** A key of the `shell` namespace (`nav.media`). */
  label: ParseKeys<"shell">;
  icon: LucideIcon;
}

/** The registered screens; empty until the M2b-2 screen tasks add theirs. */
export const SCREEN_ENTRIES: ScreenEntry[] = [];

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
  entries: ScreenEntry[] = SCREEN_ENTRIES,
): NavSection[] {
  const can: Can = (cap) => me.capabilities.includes(cap);
  // Registered screens the viewer may see, in `SCREENS` order. Nothing without
  // `read_admin`: every admin API refuses that viewer.
  const screens = (section: "content" | "structure") =>
    (Object.keys(SCREENS) as ScreenId[]).flatMap((id): NavItem[] => {
      const rule: { section: string; show: (can: Can) => boolean; shortcut?: string } = SCREENS[id];
      const entry = entries.find((e) => e.screen === id);
      if (!entry || rule.section !== section || !can("read_admin") || !rule.show(can)) {
        return [];
      }
      return [
        {
          id,
          label: t(entry.label),
          to: entry.to,
          icon: entry.icon,
          ...(rule.shortcut ? { shortcut: rule.shortcut } : {}),
        },
      ];
    });
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
    items.push(...screens("content"));
    if (items.length > 0) {
      sections.push({ id: "content", label: t("nav.content"), items });
    }
  }
  const structure = screens("structure");
  if (structure.length > 0) {
    sections.push({ id: "structure", label: t("nav.structure"), items: structure });
  }
  return sections;
}

/** The nav item whose screen `pathname` (an admin path, no `/admin` base) shows. */
export function navItemAt(sections: NavSection[], pathname: string): NavItem | undefined {
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  return sections.flatMap((s) => s.items).find((item) => item.to === (path || "/"));
}
