// The route change (11 §5): the new screen fades in and rises 4 px over `--dur-base`
// with `--ease-out`. The `m.div` is keyed by the matched leaf route, so a new route
// remounts it (and plays the enter animation) while a search-param change (a list
// filter) does not. Enter only, without `AnimatePresence` (ruling at Task 10 fix round
// 1): an exiting copy would keep rendering the same `<Outlet>`, so the next route would
// mount inside it, unmount with it and mount again. Under reduced motion only the
// opacity changes (no transform at any frame).

import { useMatches } from "@tanstack/react-router";
import { useReducedMotion } from "motion/react";
import * as m from "motion/react-m";
import type { ReactNode } from "react";
import { MOTION } from "@/design/tokens";

export function RouteTransition({ children }: { children: ReactNode }) {
  const leaf = useMatches({ select: (matches) => matches[matches.length - 1]?.routeId ?? "" });
  const reduce = useReducedMotion() ?? false;
  return (
    <m.div
      key={leaf}
      data-slot="route-transition"
      className="flex min-w-0 flex-1 flex-col"
      initial={reduce ? { opacity: 0 } : { opacity: 0, y: 4 }}
      animate={reduce ? { opacity: 1 } : { opacity: 1, y: 0 }}
      transition={{ duration: MOTION.durBase, ease: MOTION.easeOut }}
    >
      {children}
    </m.div>
  );
}
