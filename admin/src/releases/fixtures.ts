// Test data for the release screens.

import type { ReleaseDetail, ReleaseEntry } from "@/api/releases";

export const REL = "0190f1c0-0000-7000-8000-0000000000c1";
export const ITEM_A = "0190f1c0-0000-7000-8000-00000000000a";
export const ITEM_B = "0190f1c0-0000-7000-8000-00000000000b";
export const ME = "0190f1c0-0000-7000-8000-0000000000ee";

export function entry(over: Partial<ReleaseEntry> = {}): ReleaseEntry {
  return {
    item_id: ITEM_A,
    title: "About us",
    type: "page",
    locale: "en",
    status: "published",
    transition: "none",
    has_changes: true,
    conflict: false,
    base_version: 3,
    item_version: 3,
    entry_version: 1,
    position: 0,
    updated_at: "2026-11-03T10:00:00Z",
    review: null,
    ...over,
  };
}

export function release(over: Partial<ReleaseDetail> = {}): ReleaseDetail {
  return {
    id: REL,
    title: "Spring launch",
    description: "",
    status: "open",
    entry_count: 2,
    created_at: "2026-11-03T09:00:00Z",
    created_by: ME,
    updated_at: "2026-11-03T10:00:00Z",
    version: 4,
    entries: [
      entry(),
      entry({
        item_id: ITEM_B,
        title: "Spring sale",
        status: "draft",
        transition: "publish",
        has_changes: false,
        position: 1,
      }),
    ],
    ...over,
  };
}
