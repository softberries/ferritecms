import { render, renderHook } from "@testing-library/react";
import { createElement, useLayoutEffect } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import {
  CHORD_TIMEOUT_MS,
  listShortcuts,
  registerShortcut,
  useShortcut,
  type Shortcut,
} from "./shortcuts";

const cleanups: (() => void)[] = [];

function bind(keys: string, extra: Partial<Shortcut> = {}) {
  const run = vi.fn();
  cleanups.push(registerShortcut({ id: keys, keys, label: keys, group: "Test", run, ...extra }));
  return run;
}

function press(key: string, init: KeyboardEventInit = {}, target: EventTarget = document.body) {
  const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(e);
  return e;
}

function platform(name: string) {
  vi.spyOn(navigator, "platform", "get").mockReturnValue(name);
}

beforeEach(() => {
  vi.useFakeTimers();
  platform("Linux x86_64");
});

afterEach(() => {
  cleanups.splice(0).forEach((c) => c());
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

test("single_key_and_combination", () => {
  const help = bind("?");
  const palette = bind("mod+k");
  expect(press("?", { shiftKey: true }).defaultPrevented).toBe(true);
  expect(help).toHaveBeenCalledOnce();
  press("k", { ctrlKey: true });
  expect(palette).toHaveBeenCalledOnce();
  // Without the modifier, or with an extra one, nothing.
  press("k");
  press("k", { ctrlKey: true, shiftKey: true });
  press("k", { ctrlKey: true, altKey: true });
  expect(palette).toHaveBeenCalledOnce();
});

test("chords", () => {
  const dash = bind("g d");
  const pages = bind("g g");
  press("g");
  expect(dash).not.toHaveBeenCalled();
  press("d");
  expect(dash).toHaveBeenCalledOnce();
  press("g");
  press("g");
  expect(pages).toHaveBeenCalledOnce();
  // A wrong second step cancels the sequence; the next `d` alone does nothing.
  press("g");
  press("x");
  press("d");
  expect(dash).toHaveBeenCalledOnce();
});

test("chord_completes_with_a_binding_registered_between_steps", () => {
  bind("g d");
  const first = vi.fn();
  const offFirst = registerShortcut({ id: "nav", keys: "g s", label: "S", group: "T", run: first });
  press("g");
  // Registered again (its label changed) before the second step.
  offFirst();
  const second = vi.fn();
  cleanups.push(registerShortcut({ id: "nav", keys: "g s", label: "S2", group: "T", run: second }));
  press("s");
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledOnce();
});

test("chord_timeout", () => {
  const dash = bind("g d");
  press("g");
  vi.advanceTimersByTime(CHORD_TIMEOUT_MS - 1);
  press("d");
  expect(dash).toHaveBeenCalledOnce();
  press("g");
  vi.advanceTimersByTime(CHORD_TIMEOUT_MS);
  press("d");
  expect(dash).toHaveBeenCalledOnce();
});

// The gap is measured between the key presses, so a page too busy to handle the next key
// within the timeout (its timer would have fired first) still completes the sequence.
test("chord_survives_a_late_handled_second_step", () => {
  const plugins = bind("g l");
  const g = new KeyboardEvent("keydown", { key: "g", bubbles: true, cancelable: true });
  const l = new KeyboardEvent("keydown", { key: "l", bubbles: true, cancelable: true });
  document.body.dispatchEvent(g);
  // Both keys were pressed within the timeout; the second is handled long after it.
  vi.advanceTimersByTime(CHORD_TIMEOUT_MS * 3);
  document.body.dispatchEvent(l);
  expect(plugins).toHaveBeenCalledOnce();
});

// WebKit stamps a key when it handles it: a key handled late, but before the timer has
// fired (the page was busy and the timer is queued behind it), still completes the sequence.
test("chord_completes_before_the_timer_fires_whatever_the_stamps", () => {
  const plugins = bind("g l");
  press("g");
  vi.setSystemTime(Date.now() + CHORD_TIMEOUT_MS * 3); // the clock moves, no timer runs
  press("l");
  expect(plugins).toHaveBeenCalledOnce();
});

test("ignored_in_inputs", () => {
  const help = bind("?");
  const dash = bind("g d");
  const palette = bind("mod+k", { allowInInput: true });
  const input = document.createElement("input");
  const area = document.createElement("textarea");
  const editor = document.createElement("div");
  editor.setAttribute("contenteditable", "true");
  const inner = document.createElement("p");
  editor.append(inner);
  document.body.append(input, area, editor);

  for (const target of [input, area, inner]) {
    press("?", { shiftKey: true }, target);
    press("g", {}, target);
    press("d", {}, target);
    press("k", { ctrlKey: true }, target);
  }
  expect(help).not.toHaveBeenCalled();
  expect(dash).not.toHaveBeenCalled();
  expect(palette).toHaveBeenCalledTimes(3);

  // `contenteditable="false"` is not a text field.
  const off = document.createElement("div");
  off.setAttribute("contenteditable", "false");
  document.body.append(off);
  press("?", { shiftKey: true }, off);
  expect(help).toHaveBeenCalledOnce();
});

test("mod_is_meta_on_macos_and_control_elsewhere", () => {
  const palette = bind("mod+k");
  press("k", { metaKey: true });
  expect(palette).not.toHaveBeenCalled();
  press("k", { ctrlKey: true });
  expect(palette).toHaveBeenCalledOnce();

  platform("MacIntel");
  press("k", { ctrlKey: true });
  expect(palette).toHaveBeenCalledOnce();
  press("k", { metaKey: true });
  expect(palette).toHaveBeenCalledTimes(2);
});

test("when_and_default_prevented", () => {
  let live = false;
  const help = bind("?", { when: () => live });
  press("?", { shiftKey: true });
  expect(help).not.toHaveBeenCalled();
  live = true;
  // A key a component already handled is not taken again.
  const button = document.createElement("button");
  button.addEventListener("keydown", (e) => e.preventDefault());
  document.body.append(button);
  press("?", { shiftKey: true }, button);
  expect(help).not.toHaveBeenCalled();
  press("?", { shiftKey: true });
  expect(help).toHaveBeenCalledOnce();
});

test("unregister", () => {
  const run = vi.fn();
  const off = registerShortcut({ id: "x", keys: "x", label: "X", group: "Test", run });
  expect(listShortcuts().map((s) => s.id)).toEqual(["x"]);
  off();
  expect(listShortcuts()).toEqual([]);
  press("x");
  expect(run).not.toHaveBeenCalled();

  // Re-registering an id replaces it; the old handle no longer removes the new one.
  const first = vi.fn();
  const second = vi.fn();
  const offFirst = registerShortcut({ id: "y", keys: "y", label: "Y", group: "T", run: first });
  cleanups.push(registerShortcut({ id: "y", keys: "y", label: "Y", group: "T", run: second }));
  offFirst();
  press("y");
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledOnce();
});

test("use_shortcut_registers_while_mounted_and_reads_the_latest_run", () => {
  const a = vi.fn();
  const b = vi.fn();
  const { rerender, unmount } = renderHook(
    ({ run }: { run: () => void }) => {
      useShortcut({ id: "h", keys: "h", label: "H", group: "T", run });
    },
    { initialProps: { run: a } },
  );
  press("h");
  expect(a).toHaveBeenCalledOnce();
  rerender({ run: b });
  press("h");
  expect(b).toHaveBeenCalledOnce();
  expect(a).toHaveBeenCalledOnce();
  unmount();
  expect(listShortcuts()).toEqual([]);
});

// The `keydown` listener is outside React, which runs passive effects after paint: a key
// pressed once the control shows must find its binding, so it is bound in the commit.
test("use_shortcut_binds_in_the_commit_that_renders_it", () => {
  let seen: string[] | undefined;
  function Bound() {
    useShortcut({ id: "c", keys: "c", label: "C", group: "T", run: () => {} });
    return null;
  }
  // A later sibling's layout effect runs in the same commit, before any passive effect.
  function Probe() {
    useLayoutEffect(() => {
      seen = listShortcuts().map((s) => s.id);
    }, []);
    return null;
  }
  const { unmount } = render(
    createElement("div", null, createElement(Bound), createElement(Probe)),
  );
  expect(seen).toEqual(["c"]);
  unmount();
});

test("alt_combinations_match_the_physical_key", () => {
  platform("MacIntel");
  const revisions = bind("mod+alt+r");
  // macOS: ⌘⌥R arrives as "®"; the code names the key.
  press("®", { metaKey: true, altKey: true, code: "KeyR" });
  expect(revisions).toHaveBeenCalledOnce();
  press("r", { metaKey: true, altKey: true, code: "KeyR" });
  expect(revisions).toHaveBeenCalledTimes(2);
  // Without Option, or on another key, nothing.
  press("r", { metaKey: true, code: "KeyR" });
  press("®", { metaKey: true, altKey: true, code: "KeyT" });
  expect(revisions).toHaveBeenCalledTimes(2);
});

test("physical_key_only_on_apple_and_never_with_altgraph", () => {
  // Windows/Linux: Ctrl+Alt (or AltGr) + R typing "®" on US-International stays typing.
  const revisions = bind("mod+alt+r");
  press("®", { ctrlKey: true, altKey: true, code: "KeyR" });
  expect(revisions).not.toHaveBeenCalled();
  // Where Alt leaves the letter as it is, the key itself still matches.
  press("r", { ctrlKey: true, altKey: true, code: "KeyR" });
  expect(revisions).toHaveBeenCalledOnce();
});

test("mac_option_reported_as_altgraph_still_runs_cmd_option_r", () => {
  // R18: a macOS browser that reports Option as AltGraph. ⌘ held: a command chord,
  // which types nothing, so the physical key still matches.
  platform("MacIntel");
  const revisions = bind("mod+alt+r");
  const event = new KeyboardEvent("keydown", {
    key: "®",
    code: "KeyR",
    metaKey: true,
    altKey: true,
    modifierAltGraph: true,
    bubbles: true,
    cancelable: true,
  });
  expect(event.getModifierState("AltGraph")).toBe(true);
  expect(event.altKey).toBe(true);
  document.body.dispatchEvent(event);
  expect(revisions).toHaveBeenCalledOnce();
  expect(event.defaultPrevented).toBe(true);
  // Option alone with AltGraph reported types its character: an Option-only binding
  // does not take it.
  const option = bind("alt+r");
  press("®", { altKey: true, code: "KeyR", modifierAltGraph: true });
  expect(option).not.toHaveBeenCalled();
});
