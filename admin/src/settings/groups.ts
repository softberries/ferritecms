// The settings groups, in rail order: each is `/admin/settings/<id>`.

import {
  BookOpenIcon,
  ImageIcon,
  Link2Icon,
  MailIcon,
  MessageSquareIcon,
  SlidersHorizontalIcon,
  type LucideIcon,
} from "lucide-react";

export const SETTINGS_GROUPS = [
  "general",
  "reading",
  "permalinks",
  "comments",
  "media-sizes",
  "mail",
] as const;
export type SettingsGroup = (typeof SETTINGS_GROUPS)[number];

export const isSettingsGroup = (s: string): s is SettingsGroup =>
  (SETTINGS_GROUPS as readonly string[]).includes(s);

export const GROUP_ICONS: Record<SettingsGroup, LucideIcon> = {
  general: SlidersHorizontalIcon,
  reading: BookOpenIcon,
  permalinks: Link2Icon,
  comments: MessageSquareIcon,
  "media-sizes": ImageIcon,
  mail: MailIcon,
};

/** The group's key under `settings.groups` (i18n keys cannot hold `-`). */
export const GROUP_KEYS = {
  general: "general",
  reading: "reading",
  permalinks: "permalinks",
  comments: "comments",
  "media-sizes": "mediaSizes",
  mail: "mail",
} as const satisfies Record<SettingsGroup, string>;
