// The list's status tabs (All, Draft, Pending, Scheduled, Published, Private, Trash) with
// their counts from `GET /content/{type}/counts` (what the actor may read; M2b P14).
// They are links, not ARIA tabs: each one is a URL (`?status=`), and there is no tab
// panel to point at. The other filters are kept when the tab changes.

import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import type { SchemaContentCountsDto } from "@/api/schema";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { TAB_LIST, tabCountClass, tabLinkClass } from "@/components/kit/tab-styles";
import { uiLocale } from "@/lib/locale";
import {
  type ListSearch,
  STATUS_TABS,
  type StatusTab,
  parseListSearch,
  tabCount,
} from "./list-search";

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
        <ul className={TAB_LIST}>
          {STATUS_TABS.map((tab) => {
            const active = tab === current;
            const count = counts ? tabCount(counts, tab) : undefined;
            return (
              <li key={tab}>
                <Link
                  to="/content/$type"
                  params={{ type }}
                  search={(prev: Record<string, unknown>) => withStatus(parseListSearch(prev), tab)}
                  // The router marks a link current on a partial search match, which
                  // would make All current on every tab; only the exact URL counts, and
                  // `active` (from the parsed `status`) decides `aria-current`.
                  activeOptions={{ exact: true, includeSearch: true }}
                  aria-current={active ? "page" : undefined}
                  data-tab={tab}
                  className={tabLinkClass(active)}
                >
                  {t(`tabs.${tab}`)}
                  {count !== undefined ? (
                    <span data-slot="tab-count" className={tabCountClass(active)}>
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
