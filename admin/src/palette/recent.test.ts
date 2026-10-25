import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { RECENT_MAX, listRecent, pushRecent, recentStorageKey, setRecentViewer } from "./recent";

beforeEach(() => {
  localStorage.clear();
  setRecentViewer("viewer-a");
});

afterEach(() => {
  setRecentViewer(null);
  vi.restoreAllMocks();
});

const item = (id: string, title = `Item ${id}`) =>
  ({ kind: "item", type: "post", id, title, status: "draft" }) as const;

test("newest_first_without_duplicates", () => {
  pushRecent({ kind: "screen", id: "dashboard" });
  pushRecent(item("1"));
  pushRecent({ kind: "screen", id: "content-page" });
  // Opening an entry again moves it to the front (with its latest title).
  pushRecent(item("1", "Renamed"));
  expect(listRecent()).toEqual([
    item("1", "Renamed"),
    { kind: "screen", id: "content-page" },
    { kind: "screen", id: "dashboard" },
  ]);
});

test("keeps_at_most_eight", () => {
  for (let i = 0; i < 12; i++) pushRecent(item(String(i)));
  expect(RECENT_MAX).toBe(8);
  expect(listRecent().map((e) => e.id)).toEqual(["11", "10", "9", "8", "7", "6", "5", "4"]);
});

test("per_viewer", () => {
  pushRecent(item("a"));
  setRecentViewer("viewer-b");
  expect(listRecent()).toEqual([]);
  pushRecent(item("b"));
  setRecentViewer("viewer-a");
  expect(listRecent().map((e) => e.id)).toEqual(["a"]);
  expect(localStorage.getItem(recentStorageKey("viewer-b"))).toContain('"b"');
  // Without a viewer nothing is read or written.
  setRecentViewer(null);
  pushRecent(item("c"));
  expect(listRecent()).toEqual([]);
});

test("broken_or_blocked_storage_is_empty", () => {
  localStorage.setItem(recentStorageKey("viewer-a"), "{not json");
  expect(listRecent()).toEqual([]);
  // Entries of an unknown shape are dropped, the rest kept.
  localStorage.setItem(
    recentStorageKey("viewer-a"),
    JSON.stringify([{ kind: "screen", id: "dashboard" }, { kind: "nope" }, 7, item("1")]),
  );
  expect(listRecent()).toEqual([{ kind: "screen", id: "dashboard" }, item("1")]);

  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
    throw new DOMException("blocked", "SecurityError");
  });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new DOMException("blocked", "SecurityError");
  });
  expect(listRecent()).toEqual([]);
  expect(() => pushRecent(item("2"))).not.toThrow();
});
