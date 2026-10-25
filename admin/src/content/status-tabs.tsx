// The list's status tabs (All, Draft, Pending, Scheduled, Published, Private, Trash) with
// their counts from `GET /content/{type}/counts` (what the actor may read; M2b P14).
// They are links, not ARIA tabs: each one is a URL (`?status=`), and there is no tab
// panel to point at. The other filters are kept when the tab changes.

import { Link } from "@tanstack/react-router";
import { cn } from "cn";
import { useTranslation } from "react-i18next";
import type { SchemaContentCountsDto } from "@/api/schema";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { uiLocale } from "@/lib/locale";
import { type ListSearch, STATUS_TABS, type StatusTab, tabCount } from "./list-search";

export interface StatusTabsProps {
  type: string;
  search: ListSearch;
  counts: SchemaContentCountsDto | undefined;
}

export function StatusTabs({ type, search, counts }: StatusTabsProps) {
  const { t } = useTranslation("content");
  const current: StatusTab = search.status ?? "all";
  const number = new Intl.NumberFormat(uiLocale());
  return (
    <nav aria-label={t("tabs.label")}>
      <ScrollFade activeSelector={`[data-tab="${current}"]`} activeKey={current}>
        {/* The rule under the tabs is an inset shadow, so the active underline covers it
          inside the scrolling row. */}
        <ul className="flex w-max min-w-full gap-1 shadow-[inset_0_-1px_0_var(--border)]">
          {STATUS_TABS.map((tab) => {
            const active = tab === current;
            const count = counts ? tabCount(counts, tab) : undefined;
            return (
              <li key={tab}>
                <Link
                  to="/content/$type"
                  params={{ type }}
                  search={(prev: ListSearch) => withStatus(prev, tab)}
                  // The router marks a link current on a partial search match, which
                  // would make All current on every tab; only the exact URL counts, and
                  // `active` (from the parsed `status`) decides `aria-current`.
                  activeOptions={{ exact: true, includeSearch: true }}
                  aria-current={active ? "page" : undefined}
                  data-tab={tab}
                  className={cn(
                    "relative flex h-control items-center gap-2 rounded-t-md px-2.5 text-sm font-medium whitespace-nowrap -outline-offset-2 transition-colors",
                    "after:absolute after:inset-x-1.5 after:bottom-0 after:h-0.5 after:rounded-full after:transition-colors",
                    active
                      ? "text-foreground after:bg-primary"
                      : "text-muted-foreground after:bg-transparent hover:text-foreground hover:after:bg-border",
                  )}
                >
                  {t(`tabs.${tab}`)}
                  {count !== undefined ? (
                    <span
                      data-slot="tab-count"
                      className={cn(
                        "min-w-5 rounded-full px-1.5 text-center text-xs leading-5 tabular-nums",
                        active ? "bg-primary/10 text-foreground" : "bg-muted text-muted-foreground",
                      )}
                    >
                      {number.format(count)}
                    </span>
                  ) : null}
                </Link>
              </li>
            );
          })}
        </ul>
      </ScrollFade>
    </nav>
  );
}

/** `search` on another tab: the other filters stay; "all" is the absent default. */
function withStatus(search: ListSearch, tab: StatusTab): ListSearch {
  const next = { ...search };
  delete next.status;
  return tab === "all" ? next : { ...next, status: tab };
}
