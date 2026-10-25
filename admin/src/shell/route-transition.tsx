// The route change (11 §5): the new screen fades in and rises 4 px over `--dur-base`
// with `--ease-out`. Keyed by the matched leaf route, so a search-param change (a list
// filter) does not replay it. The outgoing screen leaves at once: during an exit the
// `<Outlet>` already renders the next route, so a timed exit would fade the new content
// out first. Under reduced motion only the opacity changes (no transform at any frame).

import { useMatches } from "@tanstack/react-router";
import { AnimatePresence, useReducedMotion } from "motion/react";
import * as m from "motion/react-m";
import type { ReactNode } from "react";
import { MOTION } from "@/design/tokens";

export function RouteTransition({ children }: { children: ReactNode }) {
  const leaf = useMatches({ select: (matches) => matches[matches.length - 1]?.routeId ?? "" });
  const reduce = useReducedMotion() ?? false;
  return (
    <AnimatePresence mode="wait" initial={false}>
      <m.div
        key={leaf}
        data-slot="route-transition"
        className="flex min-w-0 flex-1 flex-col"
        initial={reduce ? { opacity: 0 } : { opacity: 0, y: 4 }}
        animate={reduce ? { opacity: 1 } : { opacity: 1, y: 0 }}
        exit={{ opacity: 0, transition: { duration: 0 } }}
        transition={{ duration: MOTION.durBase, ease: MOTION.easeOut }}
      >
        {children}
      </m.div>
    </AnimatePresence>
  );
}
