// The media picker's one open request, outside React: `openMediaPicker` (from any code,
// such as an editor command) asks for it and gets a promise; the `MediaPickerHost` mounted
// in the shell shows the dialog for it and settles it with the chosen item, or `null`
// when cancelled. A second request replaces the first (which settles with `null`); with
// no host mounted the request settles with `null` at once.

import { useSyncExternalStore } from "react";
import type { MediaDto, MediaKind } from "@/api/media";

export interface PickerOptions {
  /** Only this kind (images for the featured image and inline images); any when absent. */
  kind?: MediaKind;
  /** The dialog's title ("Set featured image"). */
  title: string;
}

export interface PickerRequest extends PickerOptions {
  id: number;
}

let current: (PickerRequest & { resolve: (media: MediaDto | null) => void }) | null = null;
let seq = 0;
let hosts = 0;
const listeners = new Set<() => void>();

const emit = () => {
  for (const fn of listeners) fn();
};

/** Opens the picker; resolves to the chosen item, or `null` when cancelled. */
export function requestMediaPicker(opts: PickerOptions): Promise<MediaDto | null> {
  if (hosts === 0) return Promise.resolve(null);
  current?.resolve(null);
  return new Promise((resolve) => {
    seq += 1;
    current = { ...opts, id: seq, resolve };
    emit();
  });
}

/** Settles request `id` (unless another one replaced it meanwhile). */
export function settlePicker(id: number, media: MediaDto | null): void {
  if (current?.id !== id) return;
  const { resolve } = current;
  current = null;
  emit();
  resolve(media);
}

/** Registers a host while mounted (requests without one settle at once). */
export function registerPickerHost(): () => void {
  hosts += 1;
  return () => {
    hosts -= 1;
    if (hosts === 0 && current) settlePicker(current.id, null);
  };
}

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
};

let snapshot: PickerRequest | null = null;
const read = (): PickerRequest | null => {
  if (current === null) snapshot = null;
  else if (snapshot?.id !== current.id) {
    snapshot = {
      id: current.id,
      title: current.title,
      ...(current.kind ? { kind: current.kind } : {}),
    };
  }
  return snapshot;
};

/** The open request, for the host. */
export function usePickerRequest(): PickerRequest | null {
  return useSyncExternalStore(subscribe, read, read);
}
