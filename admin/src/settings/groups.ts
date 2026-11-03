// The settings groups, in rail order: each is `/admin/settings/<id>`.

import {
  BookOpenIcon,
  HeartPulseIcon,
  ImageIcon,
  LanguagesIcon,
  Link2Icon,
  MailIcon,
  MessageSquareIcon,
  SlidersHorizontalIcon,
  SparklesIcon,
  type LucideIcon,
} from "lucide-react";

export const SETTINGS_GROUPS = [
  "general",
  "languages",
  "reading",
  "permalinks",
  "comments",
  "media-sizes",
  "mail",
  "ai-assistants",
  "health",
] as const;
export type SettingsGroup = (typeof SETTINGS_GROUPS)[number];

export const isSettingsGroup = (s: string): s is SettingsGroup =>
  (SETTINGS_GROUPS as readonly string[]).includes(s);

export const GROUP_ICONS: Record<SettingsGroup, LucideIcon> = {
  general: SlidersHorizontalIcon,
  languages: LanguagesIcon,
  reading: BookOpenIcon,
  permalinks: Link2Icon,
  comments: MessageSquareIcon,
  "media-sizes": ImageIcon,
  mail: MailIcon,
  "ai-assistants": SparklesIcon,
  health: HeartPulseIcon,
};

/** The group's key under `settings.groups` (i18n keys cannot hold `-`). */
export const GROUP_KEYS = {
  general: "general",
  languages: "languages",
  reading: "reading",
  permalinks: "permalinks",
  comments: "comments",
  "media-sizes": "mediaSizes",
  mail: "mail",
  "ai-assistants": "aiAssistants",
  health: "health",
} as const satisfies Record<SettingsGroup, string>;
