// Test data for the review screens.

import type { ReviewDto } from "@/api/reviews";
import type { MeDto } from "@/auth/session";

export const REVIEW = "0190f1c0-0000-7000-8000-0000000000d1";
export const ITEM = "0190f1c0-0000-7000-8000-00000000000a";
export const AUTHOR_ID = "0190f1c0-0000-7000-8000-0000000000a1";
export const EDITOR_ID = "0190f1c0-0000-7000-8000-0000000000e1";

export function review(over: Partial<ReviewDto> = {}): ReviewDto {
  return {
    id: REVIEW,
    item_id: ITEM,
    item_title: "Spring notes",
    item_type: "post",
    status: "in_review",
    step: 0,
    steps: [
      {
        key: "review",
        label: "Editor approval",
        approvers: { roles: ["editor", "admin"], users: [] },
        required: 1,
      },
    ],
    submitted_at: "2026-11-03T09:00:00Z",
    submitted_by: AUTHOR_ID,
    target_kind: "item",
    version: 2,
    note: "Please check the dates",
    decisions: [],
    assignees: [],
    ...over,
  };
}

const CAPS = {
  admin: [
    "read_admin",
    "edit_own",
    "edit_others",
    "publish_own",
    "publish_others",
    "manage_users",
    "manage_settings",
  ],
  editor: ["read_admin", "edit_own", "edit_others", "publish_own", "publish_others"],
  author: ["read_admin", "edit_own", "publish_own"],
  contributor: ["read_admin", "edit_own"],
} as const;

export function me(role: keyof typeof CAPS, id = role === "author" ? AUTHOR_ID : EDITOR_ID): MeDto {
  return {
    id,
    login: role,
    email: `${role}@example.test`,
    display_name: role,
    role,
    capabilities: [...CAPS[role]],
    scopes: [],
    must_reset: false,
    totp_enabled: false,
  };
}
