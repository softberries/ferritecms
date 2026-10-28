// A row that scrolls sideways when it does not fit (phone widths), without a scrollbar:
// the edge that hides more content fades out, which is the cue that it scrolls. The
// element matching `activeSelector` is kept in view whenever `activeKey` changes (the
// current status tab) or the row resizes.

import { cn } from "cn";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

export interface ScrollFadeProps {
  children: ReactNode;
  /** A selector inside the row to keep scrolled into view. */
  activeSelector?: string;
  /** Changing it scrolls the active element into view again. */
  activeKey?: string;
  className?: string;
}

/** The fade's width, in rem (the mask below and `reveal`'s margins). */
const FADE_REM = 2;
const FADE = `${FADE_REM}rem`;

/** `FADE` in pixels, from the root font size (a user's larger default text widens it). */
function fadePx(): number {
  const root = Number.parseFloat(getComputedStyle(document.documentElement).fontSize);
  return FADE_REM * (Number.isFinite(root) && root > 0 ? root : 16);
}

/** Sets the room past the row's end (see `reveal`). */
function setRoom(el: HTMLElement, room: { current: number }, px: number): void {
  room.current = px;
  el.style.setProperty("--scroll-fade-room", `${px}px`);
}

export function ScrollFade({ children, activeSelector, activeKey, className }: ScrollFadeProps) {
  const ref = useRef<HTMLDivElement>(null);
  const room = useRef(0);
  const [edges, setEdges] = useState({ start: false, end: false });

  // Brings the active element into view; also on resize (a rotation, or a window that
  // narrows to phone width), since that can push it out of the row. The row then starts
  // at an item's edge (or its very start), so the item cut by the start fade is never
  // reduced to its tail (a tab's bare count). The last items may need the row to scroll
  // a little past its end for that: the room is `--scroll-fade-room`, which a row opts
  // into as its end padding (`TAB_LIST`).
  const reveal = useCallback(() => {
    const el = ref.current;
    if (!el || !activeSelector) return;
    const active = el.querySelector<HTMLElement>(activeSelector);
    if (!active) return;
    // Only this row scrolls (never the page): `scrollIntoView` could move both.
    // The row is positioned, so `offsetLeft` is relative to it, whatever it scrolled.
    const left = active.offsetLeft;
    const right = left + active.offsetWidth;
    const width = el.clientWidth;
    const max = Math.max(0, el.scrollWidth - room.current - width);
    if (max === 0) {
      // Everything fits: no room, no scroll.
      setRoom(el, room, 0);
      el.scrollLeft = 0;
      return;
    }
    const fade = fadePx();
    const fits = (at: number) =>
      (at === 0 || left >= at + fade) && right <= at + width - (at >= max ? 0 : fade);
    const current = el.scrollLeft;
    const items = Array.from(el.firstElementChild?.children ?? [], (c) =>
      c instanceof HTMLElement ? c.offsetLeft : 0,
    );
    const target = [0, ...items]
      .filter(fits)
      .sort((a, b) => Math.abs(a - current) - Math.abs(b - current))[0];
    if (target === undefined) {
      // Wider than the row: its start in view.
      setRoom(el, room, 0);
      el.scrollLeft = left;
      return;
    }
    const clean = current < 1 || items.some((x) => Math.abs(x - current) < 1);
    if (fits(current) && clean) {
      // Only the room this position needs (none once the row fits again).
      setRoom(el, room, Math.max(0, current - max));
      return;
    }
    setRoom(el, room, Math.max(0, target - max));
    el.scrollLeft = target;
  }, [activeSelector]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const start = el.scrollLeft > 1;
      const end = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
      setEdges((prev) => (prev.start === start && prev.end === end ? prev : { start, end }));
    };
    measure();
    el.addEventListener("scroll", measure, { passive: true });
    const ro =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            reveal();
            measure();
          });
    ro?.observe(el);
    return () => {
      el.removeEventListener("scroll", measure);
      ro?.disconnect();
    };
  }, [reveal]);

  useEffect(() => {
    reveal();
  }, [reveal, activeKey]);

  const mask = [
    edges.start ? `transparent, black ${FADE}` : "black",
    edges.end ? `black calc(100% - ${FADE}), transparent` : "black",
  ].join(", ");
  return (
    <div
      ref={ref}
      data-slot="scroll-fade"
      data-fade-start={edges.start || undefined}
      data-fade-end={edges.end || undefined}
      className={cn("no-scrollbar relative overflow-x-auto", className)}
      style={
        edges.start || edges.end ? { maskImage: `linear-gradient(to right, ${mask})` } : undefined
      }
    >
      {children}
    </div>
  );
}
