// Keyboard shortcuts (docs/design/11-admin-ui.md §6): one `keydown` listener on the
// window for every registered binding. `keys` is a combination (`mod+k`, `mod+shift+p`,
// `?`) or a sequence of steps separated by spaces (`g d`); a sequence times out after
// 1 000 ms between steps, measured between the key presses (`timeStamp`), not by a timer,
// so a busy page that handles the next key late still completes it. `mod` is ⌘ on Apple platforms and Ctrl elsewhere. Inside
// `input`, `textarea`, `select` and `[contenteditable]` only `allowInInput` bindings
// fire, so typing (and the editor's own `mod+b`) is never taken over.

import { useEffect, useLayoutEffect, useRef } from "react";

export interface Shortcut {
  /** Unique; registering the same id again replaces the earlier binding. */
  id: string;
  keys: string;
  /** What it does, shown on the `?` sheet (already translated). */
  label: string;
  /** The sheet's group heading (already translated). */
  group: string;
  run: () => void;
  /** The binding is live only while this returns true. */
  when?: () => boolean;
  allowInInput?: boolean;
}

/** How long a sequence (`g d`) waits for its next step. */
export const CHORD_TIMEOUT_MS = 1_000;

interface Step {
  key: string;
  mod: boolean;
  shift: boolean;
  alt: boolean;
}

const registry = new Map<string, Shortcut>();
/**
 * A sequence under way: the steps pressed so far. The bindings it can still complete are
 * looked up in the registry at each step, so one registered again between two steps (a
 * nav item whose label changed when its count loaded) still completes it.
 */
let pending: {
  pressed: KeyboardEvent[];
} | null = null;
let listening = false;

/** Whether this is an Apple platform (`mod` = ⌘). */
export function isApplePlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  const platform = nav.userAgentData?.platform ?? navigator.platform;
  return /mac|iphone|ipad|ipod/i.test(platform);
}

/** `mod+shift+p` → one step; a sequence is several. */
export function parseKeys(keys: string): Step[] {
  return keys
    .trim()
    .split(/\s+/)
    .map((step) => {
      const parts = step.toLowerCase().split("+");
      // `mod++` would be the plus key; nothing binds it, so the last part is the key.
      const key = parts[parts.length - 1] ?? "";
      const mods = new Set(parts.slice(0, -1));
      return { key, mod: mods.has("mod"), shift: mods.has("shift"), alt: mods.has("alt") };
    });
}

const NAMED: Record<string, string> = {
  escape: "esc",
  arrowup: "up",
  arrowdown: "down",
  arrowleft: "left",
  arrowright: "right",
  " ": "space",
};

function eventKey(e: KeyboardEvent): string {
  // With Option held, macOS composes another character (⌥R is "®"): a letter or digit
  // key is then named by the physical key, so `mod+alt+r` still matches. Only there:
  // elsewhere Alt (and AltGr, which arrives as Ctrl+Alt on Windows) types real letters
  // on many layouts (Polish ą, ę, ś…), which a shortcut must never swallow. A Mac has
  // no AltGr key, but a browser may report Option as AltGraph (R18): with ⌘ held the
  // chord is a command and types nothing, so the physical key applies then too; Option
  // alone with AltGraph reported is left to type.
  if (
    e.altKey &&
    isApplePlatform() &&
    (e.metaKey || !e.getModifierState("AltGraph")) &&
    /^(Key[A-Z]|Digit[0-9])$/.test(e.code)
  ) {
    return e.code.slice(-1).toLowerCase();
  }
  const key = e.key.toLowerCase();
  return NAMED[key] ?? key;
}

/** Punctuation (`?`, `/`, `#`) already carries Shift in `e.key`; its Shift is not compared. */
const isPunctuation = (key: string) => key.length === 1 && !/[a-z0-9]/.test(key);

function matches(step: Step, e: KeyboardEvent): boolean {
  if (eventKey(e) !== step.key) return false;
  const apple = isApplePlatform();
  const mod = apple ? e.metaKey : e.ctrlKey;
  // The other platform's key (Ctrl on a Mac, the Windows key elsewhere) is never `mod`.
  const other = apple ? e.ctrlKey : e.metaKey;
  if (mod !== step.mod || other) return false;
  if (e.altKey !== step.alt) return false;
  if (!isPunctuation(step.key) && e.shiftKey !== step.shift) return false;
  return true;
}

function inTextField(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  if (target.closest("input, textarea, select")) return true;
  if (target instanceof HTMLElement && target.isContentEditable) return true;
  // jsdom does not implement `isContentEditable`; the attribute is the source of truth.
  const editable = target.closest("[contenteditable]");
  return editable !== null && editable.getAttribute("contenteditable") !== "false";
}

function clearPending(): void {
  pending = null;
}

/** Whether `e` came too long after the last step pressed so far. */
function timedOut(e: KeyboardEvent): boolean {
  const last = pending?.pressed[pending.pressed.length - 1];
  return last === undefined || e.timeStamp - last.timeStamp >= CHORD_TIMEOUT_MS;
}

function eligible(s: Shortcut, inInput: boolean): boolean {
  if (inInput && !s.allowInInput) return false;
  return s.when ? s.when() : true;
}

function fire(s: Shortcut, e: KeyboardEvent): void {
  e.preventDefault();
  clearPending();
  s.run();
}

function onKeyDown(e: KeyboardEvent): void {
  if (e.defaultPrevented || e.isComposing || e.repeat) return;
  if (["shift", "control", "meta", "alt"].includes(e.key.toLowerCase())) return;
  const inInput = inTextField(e.target);

  if (pending && timedOut(e)) clearPending();
  if (pending) {
    const pressed = [...pending.pressed, e];
    const next = [...registry.values()].filter((s) => {
      const steps = parseKeys(s.keys);
      return (
        steps.length >= pressed.length &&
        eligible(s, inInput) &&
        pressed.every((ev, i) => matches(steps[i] as Step, ev))
      );
    });
    if (next.length > 0) {
      const done = next.find((s) => parseKeys(s.keys).length === pressed.length);
      if (done) {
        fire(done, e);
        return;
      }
      e.preventDefault();
      startPending(pressed);
      return;
    }
    clearPending();
  }

  const first = [...registry.values()].filter((s) => {
    const step = parseKeys(s.keys)[0];
    return step !== undefined && eligible(s, inInput) && matches(step, e);
  });
  if (first.length === 0) return;
  const single = first.find((s) => parseKeys(s.keys).length === 1);
  if (single) {
    fire(single, e);
    return;
  }
  e.preventDefault();
  startPending([e]);
}

function startPending(pressed: KeyboardEvent[]): void {
  pending = { pressed };
}

/** Registers a binding; the returned function removes it. */
export function registerShortcut(s: Shortcut): () => void {
  registry.set(s.id, s);
  if (!listening) {
    window.addEventListener("keydown", onKeyDown);
    listening = true;
  }
  return () => {
    if (registry.get(s.id) !== s) return;
    registry.delete(s.id);
    if (registry.size === 0) {
      clearPending();
      window.removeEventListener("keydown", onKeyDown);
      listening = false;
    }
  };
}

/** Every registered binding, in registration order. */
export function listShortcuts(): Shortcut[] {
  return [...registry.values()];
}

/**
 * Registers `s` while the component is mounted. `run` and `when` are read from the
 * latest render, so they may close over fresh state without re-registering.
 */
export function useShortcut(s: Shortcut): void {
  const latest = useRef(s);
  useEffect(() => {
    latest.current = s;
  });
  const { id, keys, label, group, allowInInput } = s;
  // A layout effect: bound in the commit that shows the control, not after paint.
  useLayoutEffect(
    () =>
      registerShortcut({
        id,
        keys,
        label,
        group,
        ...(allowInInput === undefined ? {} : { allowInInput }),
        run: () => latest.current.run(),
        when: () => latest.current.when?.() ?? true,
      }),
    [id, keys, label, group, allowInInput],
  );
}
