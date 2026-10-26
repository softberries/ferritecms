import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { qk } from "@/api/keys";
import { logout } from "@/auth/session";
import type { Draft } from "./autosave";
import {
  BACKUP_INTERVAL_MS,
  clearAllBackups,
  clearBackup,
  readBackup,
  writeBackup,
} from "./local-backup";

vi.mock("@/api/auth", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/auth")>()),
  logout: vi.fn(() => Promise.resolve()),
}));

const ITEM = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
const USER = "0190a1b2-0000-7000-8000-000000000001";
const OTHER = "0190a1b2-0000-7000-8000-000000000002";
const KEY = { item: ITEM, user: USER };

const draft = (title: string): Draft => ({
  base_version: 3,
  title,
  excerpt: null,
  body: { type: "doc", content: [] },
  fields: {},
  slug: null,
  parent_id: null,
  menu_order: null,
  terms: null,
  seo: null,
});

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-26T10:00:00Z"));
  window.localStorage.clear();
});

afterEach(() => {
  clearAllBackups(USER);
  clearAllBackups(OTHER);
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("local_backup_test", () => {
  test("writes_under_the_item_and_user_key", () => {
    writeBackup(KEY, draft("One"));
    const raw = window.localStorage.getItem(`ferrite:backup:${ITEM}:${USER}`);
    expect(raw).not.toBeNull();
    expect(readBackup(KEY)).toEqual({ draft: draft("One"), saved_at: "2026-10-26T10:00:00.000Z" });
    expect(readBackup({ item: ITEM, user: OTHER })).toBeNull();
  });

  test("throttle", () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    writeBackup(KEY, draft("One"));
    writeBackup(KEY, draft("Two"));
    writeBackup(KEY, draft("Three"));
    // At most once a second: the first at once, the newest at the end of the second.
    expect(setItem).toHaveBeenCalledTimes(1);
    expect(readBackup(KEY)?.draft.title).toBe("One");
    vi.advanceTimersByTime(BACKUP_INTERVAL_MS - 1);
    expect(setItem).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);
    expect(setItem).toHaveBeenCalledTimes(2);
    expect(readBackup(KEY)?.draft.title).toBe("Three");
    // Nothing waits any more; a write a second later goes at once.
    vi.advanceTimersByTime(BACKUP_INTERVAL_MS * 3);
    expect(setItem).toHaveBeenCalledTimes(2);
    writeBackup(KEY, draft("Four"));
    expect(setItem).toHaveBeenCalledTimes(3);
  });

  test("clear_cancels_a_waiting_write", () => {
    writeBackup(KEY, draft("One"));
    writeBackup(KEY, draft("Two"));
    clearBackup(KEY);
    vi.advanceTimersByTime(BACKUP_INTERVAL_MS * 2);
    expect(readBackup(KEY)).toBeNull();
  });

  test("storage_throwing", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const error = vi.spyOn(console, "error");
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("full", "QuotaExceededError");
    });
    expect(() => writeBackup(KEY, draft("One"))).not.toThrow();
    expect(warn).toHaveBeenCalledTimes(1);
    expect(error).not.toHaveBeenCalled();
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("denied", "SecurityError");
    });
    expect(readBackup(KEY)).toBeNull();
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {
      throw new DOMException("denied", "SecurityError");
    });
    expect(() => clearBackup(KEY)).not.toThrow();
    expect(() => clearAllBackups(USER)).not.toThrow();
  });

  test("garbage_reads_as_none", () => {
    window.localStorage.setItem(`ferrite:backup:${ITEM}:${USER}`, "{not json");
    expect(readBackup(KEY)).toBeNull();
    window.localStorage.setItem(`ferrite:backup:${ITEM}:${USER}`, JSON.stringify({ draft: 1 }));
    expect(readBackup(KEY)).toBeNull();
  });

  test("clear_on_logout", async () => {
    const other = { item: ITEM, user: OTHER };
    writeBackup(KEY, draft("Mine"));
    writeBackup({ item: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c", user: USER }, draft("Also"));
    writeBackup(other, draft("Theirs"));
    window.localStorage.setItem("ferrite:theme", "dark");
    const queryClient = new QueryClient();
    queryClient.setQueryData(qk.me, { id: USER });
    await logout({ queryClient, navigate: () => undefined });
    expect(readBackup(KEY)).toBeNull();
    expect(readBackup({ item: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c", user: USER })).toBeNull();
    // Another account's backup and other settings stay.
    expect(readBackup(other)?.draft.title).toBe("Theirs");
    expect(window.localStorage.getItem("ferrite:theme")).toBe("dark");
  });
});
