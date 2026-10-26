// The icon of each field kind, shared by the builder's rows and the kind menu.

import {
  CalendarClockIcon,
  HashIcon,
  ImageIcon,
  LinkIcon,
  ListChecksIcon,
  PilcrowIcon,
  Repeat2Icon,
  ToggleLeftIcon,
  TypeIcon,
  type LucideIcon,
} from "lucide-react";
import type { FieldKind } from "@/editor/fields/fields-model";

export const KIND_ICONS: Record<FieldKind, LucideIcon> = {
  text: TypeIcon,
  rich_text: PilcrowIcon,
  number: HashIcon,
  date: CalendarClockIcon,
  bool: ToggleLeftIcon,
  select: ListChecksIcon,
  relation: LinkIcon,
  media: ImageIcon,
  repeater: Repeat2Icon,
};
