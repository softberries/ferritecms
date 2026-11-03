// The live preview's per-viewer choices (design 15 §3.3), kept in `localStorage` like the
// other display preferences (`design/preferences.ts`): the device width and, in the split
// view, the writing side's share of the width (35–65 %). Storage can be missing or throw,
// so every access is guarded and falls back to the default.

export const DEVICE_KEY = "ferrite.admin.previewDevice";
export const SPLIT_KEY = "ferrite.admin.previewSplit";

export const DEVICES = ["desktop", "tablet", "phone"] as const;
export type Device = (typeof DEVICES)[number];

/** The frame's width per device (Desktop fills the pane). */
export const DEVICE_WIDTH: Record<Device, string> = {
  desktop: "100%",
  tablet: "768px",
  phone: "390px",
};

export const SPLIT_MIN = 35;
export const SPLIT_MAX = 65;
export const SPLIT_DEFAULT = 50;
/** One arrow key moves the divider this far. */
export const SPLIT_STEP = 5;

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Not persisted; the choice still holds for this page.
  }
}

export function storedDevice(): Device {
  const v = read(DEVICE_KEY);
  return DEVICES.find((d) => d === v) ?? "desktop";
}

export function storeDevice(device: Device): void {
  write(DEVICE_KEY, device);
}

export const clampSplit = (n: number) => Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, n));

export function storedSplit(): number {
  const n = Number(read(SPLIT_KEY));
  return Number.isFinite(n) && n > 0 ? clampSplit(n) : SPLIT_DEFAULT;
}

export function storeSplit(n: number): void {
  write(SPLIT_KEY, String(clampSplit(n)));
}
