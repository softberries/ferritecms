// The sidebar's sections (and the palette's "Go to", "Create" and "Recent" groups). An item
// shows only when `me.capabilities` holds what its API needs; the server still
// enforces every call, the SPA only hides (constraints: capabilities in core).
// Screens beyond the content lists (Media, Comments, Structure, and M2b-3's
// Administration: Users, Appearance, Plugins, Webhooks, Import, Settings, System) each register a
// `ScreenEntry` in `SCREEN_ENTRIES` when their route exists (ruling F-A); who sees an
// entry, its section and its chord are fixed here in `SCREENS`.

import {
  HeartPulseIcon,
  BlocksIcon,
  FileTextIcon,
  FilesIcon,
  HistoryIcon,
  ImageIcon,
  ImportIcon,
  LayoutDashboardIcon,
  ListTreeIcon,
  MessageSquareIcon,
  PaletteIcon,
  PlugIcon,
  ServerCogIcon,
  SettingsIcon,
  ShapesIcon,
  SignpostIcon,
  TagsIcon,
  UsersIcon,
  WebhookIcon,
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

/** The sidebar sections a registered screen can go in, in sidebar order. */
type ScreenSection = "content" | "structure" | "admin";

/** The screens a task can register, with the rule for who sees each. */
const SCREENS = {
  // Contributors too: the screen is read-only without `upload_media` (it decides).
  media: { section: "content", show: (can: Can) => can("read_admin"), shortcut: "g m" },
  comments: {
    section: "content",
    show: (can: Can) => can("moderate_comments"),
    shortcut: "g c",
  },
  // Content health (M6-4, design 16 §11.2): editors and admins; `g h` is free in 11 §6.
  health: { section: "content", show: (can: Can) => can("edit_others"), shortcut: "g h" },
  types: { section: "structure", show: (can: Can) => can("manage_settings") },
  // `manage_terms` alone manages terms, not the taxonomies themselves (the screen decides).
  taxonomies: {
    section: "structure",
    show: (can: Can) => can("manage_settings") || can("manage_terms"),
  },
  menus: { section: "structure", show: (can: Can) => can("manage_menus") },
  redirects: { section: "structure", show: (can: Can) => can("manage_redirects") },
  // Administration (M2b-3), after Structure. Account is not here: every role reaches it
  // from the user menu and the palette.
  users: { section: "admin", show: (can: Can) => can("manage_users"), shortcut: "g u" },
  appearance: { section: "admin", show: (can: Can) => can("manage_themes"), shortcut: "g a" },
  // Not `g p`: that is Posts (11 §6).
  plugins: { section: "admin", show: (can: Can) => can("manage_plugins"), shortcut: "g l" },
  // Outgoing webhooks (M6-6, design 18 §10.1), after Plugins; `g w` is free in 11 §6.
  webhooks: { section: "admin", show: (can: Can) => can("manage_webhooks"), shortcut: "g w" },
  // What AI assistants changed (M6-5, design 17 §11): every role with the admin sees its own.
  activity: { section: "admin", show: (can: Can) => can("read_admin") },
  // The WordPress import (M3-2 P32); `g i` is free in 11 §6.
  import: { section: "admin", show: (can: Can) => can("import"), shortcut: "g i" },
  settings: { section: "admin", show: (can: Can) => can("manage_settings"), shortcut: "g s" },
  system: { section: "admin", show: (can: Can) => can("manage_system"), shortcut: "g y" },
} as const satisfies Record<
  string,
  { section: ScreenSection; show: (can: Can) => boolean; shortcut?: string }
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

/**
 * The registered screens. Each screen task adds its entry to this literal when it adds
 * its route; nothing is registered at runtime (a lazy route module would register
 * too late for the sidebar and the palette).
 */
export const SCREEN_ENTRIES: readonly ScreenEntry[] = [
  { screen: "media", to: "/media", label: "nav.media", icon: ImageIcon },
  { screen: "comments", to: "/comments", label: "nav.comments", icon: MessageSquareIcon },
  { screen: "health", to: "/health", label: "nav.health", icon: HeartPulseIcon },
  { screen: "types", to: "/structure/types", label: "nav.contentTypes", icon: BlocksIcon },
  { screen: "taxonomies", to: "/structure/taxonomies", label: "nav.taxonomies", icon: TagsIcon },
  { screen: "menus", to: "/structure/menus", label: "nav.menus", icon: ListTreeIcon },
  {
    screen: "redirects",
    to: "/structure/redirects",
    label: "nav.redirects",
    icon: SignpostIcon,
  },
  { screen: "users", to: "/users", label: "nav.users", icon: UsersIcon },
  { screen: "appearance", to: "/appearance/themes", label: "nav.appearance", icon: PaletteIcon },
  { screen: "plugins", to: "/plugins", label: "nav.plugins", icon: PlugIcon },
  { screen: "webhooks", to: "/webhooks", label: "nav.webhooks", icon: WebhookIcon },
  { screen: "activity", to: "/ai-activity", label: "nav.aiActivity", icon: HistoryIcon },
  { screen: "import", to: "/import", label: "nav.import", icon: ImportIcon },
  { screen: "settings", to: "/settings", label: "nav.settings", icon: SettingsIcon },
  { screen: "system", to: "/system", label: "nav.system", icon: ServerCogIcon },
] as const;

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
  entries: readonly ScreenEntry[] = SCREEN_ENTRIES,
): NavSection[] {
  const can: Can = (cap) => me.capabilities.includes(cap);
  // Registered screens the viewer may see, in `SCREENS` order. Nothing without
  // `read_admin`: every admin API refuses that viewer.
  const screens = (section: ScreenSection) =>
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
  const admin = screens("admin");
  if (admin.length > 0) {
    sections.push({ id: "admin", label: t("nav.administration"), items: admin });
  }
  return sections;
}

/** The nav item whose screen `pathname` (an admin path, no `/admin` base) shows. */
export function navItemAt(sections: NavSection[], pathname: string): NavItem | undefined {
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  return sections.flatMap((s) => s.items).find((item) => item.to === (path || "/"));
}

/**
 * `sections` with a count badge on the items named in `badges` (by item id; the shell
 * passes the pending comments as `comments`). A zero or absent count shows none.
 */
export function withBadges(
  sections: NavSection[],
  badges: Partial<Record<string, number>>,
): NavSection[] {
  const any = Object.values(badges).some((n) => n !== undefined && n > 0);
  if (!any) return sections;
  return sections.map((section) => ({
    ...section,
    items: section.items.map((item) => {
      const n = badges[item.id];
      return n !== undefined && n > 0 ? { ...item, badge: n } : item;
    }),
  }));
}
