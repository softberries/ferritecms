// The autosave scheduler (07 §3, Review Focus 3): one write 10 s after a change, never
// two at once, a newer draft after the running one, offline retries, and no write once
// an explicit save has stored the draft (nor while it runs).

import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { NetworkError } from "@/api/client";
import { ApiError } from "@/api/problem";
import {
  AUTOSAVE_INTERVAL_MS,
  type AutosaveState,
  type Draft,
  createAutosaver,
  dropPendingDraft,
  peekPendingDraft,
  pendingDrafts,
} from "./autosave";

const draft = (title: string, base_version = 3): Draft => ({
  base_version,
  title,
  excerpt: null,
  body: { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: title }] }] },
  fields: {},
  slug: null,
  parent_id: null,
  menu_order: null,
  terms: null,
});

/** A promise the test settles by hand. */
function deferred() {
  let resolve!: () => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

class FakeDocument extends EventTarget {
  visibilityState: DocumentVisibilityState = "visible";
}

function setup(save: (d: Draft) => Promise<void>, onError?: (e: unknown) => void) {
  const win = new EventTarget();
  const doc = new FakeDocument();
  const states: AutosaveState[] = [];
  const a = createAutosaver({
    save,
    intervalMs: AUTOSAVE_INTERVAL_MS,
    window: win,
    document: doc,
    ...(onError ? { onError } : {}),
  });
  a.state.subscribe((s) => states.push(s));
  return { a, win, doc, states };
}

/** Lets the promise chains of a finished write run. */
const settle = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  pendingDrafts.clear();
});

describe("autosave", () => {
  test("dirty_puts_once_at_10s", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a, states } = setup(save);
    expect(AUTOSAVE_INTERVAL_MS).toBe(10_000);
    a.markDirty(draft("one"));
    a.markDirty(draft("two"));
    expect(a.state.get()).toBe("dirty");
    await vi.advanceTimersByTimeAsync(9_999);
    expect(save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenLastCalledWith(draft("two"));
    await settle();
    expect(a.state.get()).toBe("saved");
    expect(states).toEqual(["dirty", "saving", "saved"]);
    // Nothing changed since: no further write.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(save).toHaveBeenCalledTimes(1);
    a.dispose();
  });

  test("edits_during_a_running_put_make_exactly_one_follow_up", async () => {
    const first = deferred();
    const save = vi
      .fn<(d: Draft) => Promise<void>>()
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue(undefined);
    const { a } = setup(save);
    a.markDirty(draft("a"));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(save).toHaveBeenCalledTimes(1);
    // Three edits while the first write runs; a flush meanwhile does not start a second.
    a.markDirty(draft("b"));
    a.markDirty(draft("c"));
    const flushed = a.flush();
    a.markDirty(draft("d"));
    await vi.advanceTimersByTimeAsync(30_000);
    expect(save).toHaveBeenCalledTimes(1);
    first.resolve();
    await flushed;
    await vi.advanceTimersByTimeAsync(30_000);
    // Exactly one follow-up, with the latest draft.
    expect(save).toHaveBeenCalledTimes(2);
    expect(save).toHaveBeenLastCalledWith(draft("d"));
    expect(a.state.get()).toBe("saved");
    a.dispose();
  });

  test("offline_then_retry_on_online", async () => {
    const save = vi
      .fn<(d: Draft) => Promise<void>>()
      .mockRejectedValueOnce(new NetworkError(new TypeError("Failed to fetch")))
      .mockResolvedValue(undefined);
    const { a, win } = setup(save);
    a.markDirty(draft("offline"));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(a.state.get()).toBe("offline");
    // Back online before the next tick: retried at once, with the same draft.
    win.dispatchEvent(new Event("online"));
    await settle();
    expect(save).toHaveBeenCalledTimes(2);
    expect(save).toHaveBeenLastCalledWith(draft("offline"));
    expect(a.state.get()).toBe("saved");
    a.dispose();
  });

  test("offline_retries_on_the_next_tick", async () => {
    const save = vi
      .fn<(d: Draft) => Promise<void>>()
      .mockRejectedValueOnce(new NetworkError(new TypeError("Failed to fetch")))
      .mockResolvedValue(undefined);
    const { a } = setup(save);
    a.markDirty(draft("x"));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(a.state.get()).toBe("offline");
    // An edit while offline keeps saying offline (not "unsaved") until it is stored.
    a.markDirty(draft("y"));
    expect(a.state.get()).toBe("offline");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(save).toHaveBeenCalledTimes(2);
    expect(save).toHaveBeenLastCalledWith(draft("y"));
    expect(a.state.get()).toBe("saved");
    a.dispose();
  });

  test("a_network_failure_after_mark_clean_does_not_come_back", async () => {
    // e1cd3a5 re-review: the draft went back to the item (`markClean`) while its PUT ran;
    // that PUT then failed offline and re-queued the old draft, which was written later.
    const running = deferred();
    const save = vi.fn<(d: Draft) => Promise<void>>(() => running.promise);
    const { a, states } = setup(save);
    a.markDirty(draft("Half-way"));
    await vi.advanceTimersByTimeAsync(AUTOSAVE_INTERVAL_MS);
    expect(save).toHaveBeenCalledOnce();
    a.markClean();
    running.reject(new NetworkError("offline"));
    await settle();
    expect(a.state.get()).toBe("saved");
    expect(states).not.toContain("offline");
    await vi.advanceTimersByTimeAsync(AUTOSAVE_INTERVAL_MS * 3);
    expect(save).toHaveBeenCalledOnce();
  });

  test("a_newer_draft_survives_a_network_failure_after_mark_clean", async () => {
    // An edit after the `markClean` is still written once the connection is back.
    const running = deferred();
    const save = vi.fn<(d: Draft) => Promise<void>>(() => running.promise);
    const { a } = setup(save);
    a.markDirty(draft("Half-way"));
    await vi.advanceTimersByTimeAsync(AUTOSAVE_INTERVAL_MS);
    a.markClean();
    a.markDirty(draft("Newer"));
    running.reject(new NetworkError("offline"));
    await settle();
    save.mockImplementation(() => Promise.resolve());
    await vi.advanceTimersByTimeAsync(AUTOSAVE_INTERVAL_MS);
    expect(save.mock.calls.map(([d]) => d.title)).toEqual(["Half-way", "Newer"]);
  });

  test("explicit_save_marks_clean_and_puts_nothing", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a } = setup(save);
    a.markDirty(draft("typed"));
    await vi.advanceTimersByTimeAsync(4_000);
    // The PATCH: held around it, then clean (the server deleted the slot).
    const release = await a.hold();
    a.markClean();
    release();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(save).not.toHaveBeenCalled();
    expect(a.state.get()).toBe("saved");
    a.dispose();
  });

  test("hold_waits_for_the_running_put_and_blocks_new_ones", async () => {
    const running = deferred();
    const save = vi
      .fn<(d: Draft) => Promise<void>>()
      .mockReturnValueOnce(running.promise)
      .mockResolvedValue(undefined);
    const { a } = setup(save);
    a.markDirty(draft("one"));
    await vi.advanceTimersByTimeAsync(10_000);
    let held = false;
    const holding = a.hold().then((release) => {
      held = true;
      return release;
    });
    await settle();
    // The PATCH must not start while a PUT can still land after it.
    expect(held).toBe(false);
    running.resolve();
    const release = await holding;
    expect(held).toBe(true);
    // While held, a change waits.
    a.markDirty(draft("two"));
    await vi.advanceTimersByTimeAsync(30_000);
    expect(save).toHaveBeenCalledTimes(1);
    release();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(save).toHaveBeenCalledTimes(2);
    a.dispose();
  });

  test("a_lazy_draft_is_read_when_the_write_starts", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a } = setup(save);
    let title = "early";
    a.markDirty(() => draft(title));
    title = "late";
    await vi.advanceTimersByTimeAsync(10_000);
    expect(save).toHaveBeenCalledWith(draft("late"));
    a.dispose();
  });

  test("hidden_page_flushes_now", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a, doc } = setup(save);
    a.markDirty(draft("leaving"));
    doc.visibilityState = "hidden";
    doc.dispatchEvent(new Event("visibilitychange"));
    await settle();
    expect(save).toHaveBeenCalledTimes(1);
    a.dispose();
  });

  test("a_refusal_reports_and_does_not_retry_by_itself", async () => {
    const refused = new ApiError({
      type: "https://ferrite.dev/problems/unauthenticated",
      title: "Unauthenticated",
      status: 401,
      detail: "sign in",
    });
    const save = vi.fn<(d: Draft) => Promise<void>>().mockRejectedValue(refused);
    const onError = vi.fn();
    const { a } = setup(save, onError);
    a.markDirty(draft("x"));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(a.state.get()).toBe("error");
    expect(onError).toHaveBeenCalledWith(refused);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(save).toHaveBeenCalledTimes(1);
    a.dispose();
  });

  test("dispose_stops_everything", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a, win } = setup(save);
    a.markDirty(draft("x"));
    a.dispose();
    win.dispatchEvent(new Event("online"));
    await vi.advanceTimersByTimeAsync(60_000);
    await a.flush();
    expect(save).not.toHaveBeenCalled();
  });

  // The final flush on leaving the editor can outlast a test's jsdom, whose closed
  // window and document have lost their methods by the time `dispose` runs.
  test("dispose_after_the_window_is_closed_does_not_throw", () => {
    const { a, win, doc } = setup(() => Promise.resolve());
    Object.defineProperty(win, "removeEventListener", { value: undefined });
    Object.defineProperty(doc, "removeEventListener", { value: undefined });
    expect(() => a.dispose()).not.toThrow();
    expect(a.disposed()).toBe(true);
  });

  // The live preview (M6-3, design 15 §3.2): while the pane is open the slot follows a
  // 2 s idle debounce instead of the 10 s interval, and each completed write is reported.
  test("debounce_puts_once_after_2s_idle", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a } = setup(save);
    a.setDebounce(2_000);
    // Typing every 500 ms for 1.5 s: nothing yet.
    for (const word of ["a", "ab", "abc", "abcd"]) {
      a.markDirty(draft(word));
      await vi.advanceTimersByTimeAsync(500);
    }
    expect(save).not.toHaveBeenCalled();
    // 2 s after the last keystroke: one write of the newest draft.
    await vi.advanceTimersByTimeAsync(1_500);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ title: "abcd" }));
    // Nothing changed since: no further write.
    await vi.advanceTimersByTimeAsync(20_000);
    expect(save).toHaveBeenCalledTimes(1);
    a.dispose();
  });

  test("debounce_keeps_one_put_per_2s_while_typing", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a } = setup(save);
    a.setDebounce(2_000);
    // Typing without a pause for 10 s (a key every 300 ms): the writes are at least
    // 2 s apart and none waits longer than the regular interval.
    const at: number[] = [];
    save.mockImplementation(() => {
      at.push(Date.now());
      return Promise.resolve();
    });
    const start = Date.now();
    for (let i = 0; i < 34; i++) {
      a.markDirty(draft(`t${i}`));
      await vi.advanceTimersByTimeAsync(300);
    }
    expect(at.length).toBeGreaterThanOrEqual(1);
    expect(at[0]! - start).toBeLessThanOrEqual(AUTOSAVE_INTERVAL_MS);
    for (let i = 1; i < at.length; i++) expect(at[i]! - at[i - 1]!).toBeGreaterThanOrEqual(2_000);
    a.dispose();
  });

  test("debounce_off_goes_back_to_the_interval", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a } = setup(save);
    a.setDebounce(2_000);
    a.setDebounce(null);
    a.markDirty(draft("x"));
    await vi.advanceTimersByTimeAsync(2_500);
    expect(save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(7_500);
    expect(save).toHaveBeenCalledTimes(1);
    a.dispose();
  });

  test("debounce_rearms_a_waiting_draft", async () => {
    const save = vi.fn(() => Promise.resolve());
    const { a } = setup(save);
    a.markDirty(draft("x"));
    await vi.advanceTimersByTimeAsync(1_000);
    // The pane opens: the waiting draft goes 2 s from now, not at 10 s.
    a.setDebounce(2_000);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(save).toHaveBeenCalledTimes(1);
    a.dispose();
  });

  test("on_saved_reports_each_completed_write", async () => {
    const onSaved = vi.fn();
    let fail = true;
    const save = vi.fn(() =>
      fail ? Promise.reject(new NetworkError("offline")) : Promise.resolve(),
    );
    const win = new EventTarget();
    const a = createAutosaver({
      save,
      intervalMs: AUTOSAVE_INTERVAL_MS,
      window: win,
      document: new FakeDocument(),
      onSaved,
    });
    a.markDirty(draft("x"));
    await vi.advanceTimersByTimeAsync(10_000);
    // A failed write is not reported.
    expect(onSaved).not.toHaveBeenCalled();
    fail = false;
    win.dispatchEvent(new Event("online"));
    await settle();
    expect(onSaved).toHaveBeenCalledTimes(1);
    expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ title: "x" }));
    a.dispose();
  });

  test("pending_drafts_belong_to_their_account", () => {
    const entry = { userId: "u1", draft: draft("kept"), at: "2026-10-26T10:00:00Z" };
    pendingDrafts.set("item-1", entry);
    expect(peekPendingDraft("item-1", "u2")).toBeNull();
    expect(peekPendingDraft("item-1", undefined)).toBeNull();
    expect(peekPendingDraft("item-1", "u1")).toBe(entry);
    dropPendingDraft("item-1", entry);
    expect(pendingDrafts.has("item-1")).toBe(false);
  });
});
