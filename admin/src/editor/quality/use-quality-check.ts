// The editor's checks (design 16 §11.1): `POST /content/{type}/check` 1.5 s after the
// last change and when the publish dialog opens; one request at a time (a change while
// one runs asks for one more when it ends); a result for an older document revision
// than the newest one sent is dropped.

import { useCallback, useEffect, useRef, useState } from "react";
import { checkContent, type CheckRequest, type CheckResponse } from "@/api/quality";

/** The quiet time before a check. */
export const CHECK_DEBOUNCE_MS = 1500;

export interface CheckerOptions {
  /** The draft to check now. */
  draft: () => CheckRequest;
  /** Sends one check (the API call; a fake in tests). */
  send: (draft: CheckRequest, signal: AbortSignal) => Promise<CheckResponse>;
  onResult: (r: CheckResponse) => void;
  onError?: (e: unknown) => void;
  debounceMs?: number;
}

/** See the module docs; framework-free, so the timing is testable. */
export class QualityChecker {
  private timer: ReturnType<typeof setTimeout> | null = null;
  private inFlight: Promise<void> | null = null;
  private again = false;
  /** Bumped on every change. */
  private revision = 0;
  private controller: AbortController | null = null;
  private disposed = false;

  constructor(private readonly options: CheckerOptions) {}

  /** The document changed: check after the quiet time. */
  changed(): void {
    this.revision += 1;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.run();
    }, this.options.debounceMs ?? CHECK_DEBOUNCE_MS);
  }

  /** Check now (the publish dialog opens); resolves when that check ended. */
  async now(): Promise<void> {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    await this.run();
  }

  private async run(): Promise<void> {
    if (this.disposed) return;
    if (this.inFlight) {
      this.again = true;
      return this.inFlight;
    }
    const revision = this.revision;
    const controller = new AbortController();
    this.controller = controller;
    this.inFlight = this.options
      .send(this.options.draft(), controller.signal)
      .then((r) => {
        // An older revision than the newest one sent since: dropped.
        if (!this.disposed && revision === this.revision) this.options.onResult(r);
      })
      .catch((e: unknown) => {
        if (!this.disposed && !controller.signal.aborted) this.options.onError?.(e);
      })
      .finally(() => {
        this.inFlight = null;
        if (this.again && !this.disposed) {
          this.again = false;
          void this.run();
        }
      });
    return this.inFlight;
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer) clearTimeout(this.timer);
    this.controller?.abort();
  }
}

export interface QualityState {
  report: CheckResponse | null;
  /** A check failed (the panel says the checks are unavailable). */
  failed: boolean;
  /** Check now (before the publish dialog). */
  checkNow: () => Promise<void>;
}

/**
 * Runs the checks for the editor of `type`: `subscribe` is called with the change
 * callback (the editor's `update` event) and returns its unsubscribe.
 */
export function useQualityCheck(options: {
  type: string;
  draft: () => CheckRequest;
  subscribe: (onChange: () => void) => () => void;
  enabled: boolean;
}): QualityState {
  const { type, enabled } = options;
  const [report, setReport] = useState<CheckResponse | null>(null);
  const [failed, setFailed] = useState(false);
  const draftRef = useRef(options.draft);
  const subscribeRef = useRef(options.subscribe);
  useEffect(() => {
    draftRef.current = options.draft;
    subscribeRef.current = options.subscribe;
  });
  const checkerRef = useRef<QualityChecker | null>(null);
  useEffect(() => {
    if (!enabled) return;
    const checker = new QualityChecker({
      draft: () => draftRef.current(),
      send: (d, signal) => checkContent(type, d, signal),
      onResult: (r) => {
        setFailed(false);
        setReport(r);
      },
      onError: () => setFailed(true),
    });
    checkerRef.current = checker;
    checker.changed();
    const off = subscribeRef.current(() => checker.changed());
    return () => {
      off();
      checker.dispose();
      if (checkerRef.current === checker) checkerRef.current = null;
    };
  }, [type, enabled]);
  const checkNow = useCallback(() => checkerRef.current?.now() ?? Promise.resolve(), []);
  return { report, failed, checkNow };
}
