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

const FADE = "2rem";

export function ScrollFade({ children, activeSelector, activeKey, className }: ScrollFadeProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ start: false, end: false });

  // Brings the active element into view; also on resize (a rotation, or a window that
  // narrows to phone width), since that can push it out of the row.
  const reveal = useCallback(() => {
    const el = ref.current;
    if (!el || !activeSelector) return;
    const active = el.querySelector<HTMLElement>(activeSelector);
    if (!active) return;
    // Only this row scrolls (never the page): `scrollIntoView` could move both.
    // The row is positioned, so `offsetLeft` is relative to it, whatever it scrolled.
    const left = active.offsetLeft;
    const right = left + active.offsetWidth;
    if (left < el.scrollLeft) el.scrollLeft = Math.max(0, left - 16);
    else if (right > el.scrollLeft + el.clientWidth) el.scrollLeft = right - el.clientWidth + 16;
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
