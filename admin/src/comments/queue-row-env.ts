// What the comments queue's rows share with the screen (`queue.tsx`, `queue-row.tsx`):
// the icon per action, which actions are destructive, and the row context.

import {
  CheckIcon,
  MessageSquareReplyIcon,
  RotateCcwIcon,
  ShieldAlertIcon,
  ShieldCheckIcon,
  Trash2Icon,
  UndoIcon,
  XCircleIcon,
  type LucideIcon,
} from "lucide-react";
import { createContext } from "react";
import type { CommentDto } from "@/api/comments";
import type { CommentAction } from "./moderate";

export const ICONS: Record<CommentAction, LucideIcon> = {
  approve: CheckIcon,
  unapprove: UndoIcon,
  notSpam: ShieldCheckIcon,
  restore: RotateCcwIcon,
  spam: ShieldAlertIcon,
  trash: Trash2Icon,
  reply: MessageSquareReplyIcon,
  delete: XCircleIcon,
};
export const DESTRUCTIVE_ACTIONS = new Set<CommentAction>(["trash", "delete"]);

/** What a row of the queue reads from the screen. */
export interface RowEnv {
  /** Comment id → author name (for "Reply to …"). */
  names: ReadonlyMap<string, string>;
  busy: boolean;
  /** The comment whose reply box is open. */
  replying: string | null;
  /** Rows offer "Only comments on this item" (not while the queue is narrowed). */
  filterable: boolean;
  act: (action: CommentAction, targets: readonly CommentDto[]) => void;
  filter: (item: string) => void;
  closeReply: (id: string) => void;
}

export const RowEnvContext = createContext<RowEnv | null>(null);
