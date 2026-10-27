// /admin/system/<tab> (`manage_system`; 07 §3 System): one page with link tabs for
// Status, Nodes, Jobs and the Audit log. Each tab is a URL, its filters in the search.
// The tabs' counts come from `GET /system/status` (live nodes, dead jobs).

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { systemStatusQuery } from "@/api/system";
import { PageHeader } from "@/components/kit/page-header";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { TAB_LIST, tabCountClass, tabLinkClass } from "@/components/kit/tab-styles";
import { uiLocale } from "@/lib/locale";
import { AuditTab } from "./audit-tab";
import { JobsTab } from "./jobs-tab";
import { NodesTab } from "./nodes-tab";
import { StatusTab } from "./status-tab";
import {
  SYSTEM_TABS,
  type SystemSearch,
  type SystemSearchPatch,
  type SystemTab,
} from "./system-model";

export interface SystemScreenProps {
  tab: SystemTab;
  search: SystemSearch;
  setSearch: (patch: SystemSearchPatch) => void;
}

export function SystemScreen({ tab, search, setSearch }: SystemScreenProps) {
  const { t } = useTranslation("admin");
  return (
    <div data-slot="system-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={t("system.title")}
        description={t("system.description")}
        tabs={<Tabs current={tab} />}
      />
      <section data-slot="system-tab" data-tab={tab} aria-label={t(`system.tabs.${tab}`)}>
        {tab === "status" ? <StatusTab /> : null}
        {tab === "nodes" ? <NodesTab /> : null}
        {tab === "jobs" ? <JobsTab search={search} setSearch={setSearch} /> : null}
        {tab === "audit" ? <AuditTab search={search} setSearch={setSearch} /> : null}
      </section>
    </div>
  );
}

function Tabs({ current }: { current: SystemTab }) {
  const { t } = useTranslation("admin");
  const status = useQuery({ ...systemStatusQuery, meta: INLINE_ERRORS });
  const number = new Intl.NumberFormat(uiLocale());
  const count = (tab: SystemTab): number | undefined => {
    if (!status.data) return undefined;
    if (tab === "nodes") return status.data.nodes.live;
    if (tab === "jobs" && status.data.jobs.dead > 0) return status.data.jobs.dead;
    return undefined;
  };
  return (
    <nav aria-label={t("system.tabs.label")}>
      <ScrollFade activeSelector={`[data-tab-link="${current}"]`} activeKey={current}>
        <ul className={TAB_LIST}>
          {SYSTEM_TABS.map((tab) => {
            const active = tab === current;
            const n = count(tab);
            return (
              <li key={tab}>
                <Link
                  to="/system/$tab"
                  params={{ tab }}
                  aria-current={active ? "page" : undefined}
                  activeOptions={{ exact: true, includeSearch: false }}
                  data-tab-link={tab}
                  className={tabLinkClass(active)}
                >
                  {t(`system.tabs.${tab}`)}
                  {n !== undefined ? (
                    <span
                      data-slot="tab-count"
                      className={tabCountClass(active)}
                      title={tab === "jobs" ? t("system.tabs.deadCount") : undefined}
                    >
                      {number.format(n)}
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
