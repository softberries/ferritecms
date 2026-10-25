// The post count tiles (every role): Published, Drafts, Pending review, Scheduled, from
// `GET /content/post/counts` (which already counts only what the actor may read). Each
// links to the filtered list. Published shows the all-time total as its value and, as
// text under the 30-day trend, how many were published in that window. There is no
// delta: the API has no history to compare against beyond the fetched window.

import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { MetricTile } from "@/components/kit/metric-tile";
import { Skeleton } from "@/components/ui/skeleton";
import { uiLocale } from "@/lib/locale";
import {
  ACTIVITY_DAYS,
  PUBLISHED_CAP,
  browserTimeZone,
  bucketByDay,
  isCapped,
  publishedQuery,
} from "./activity";
import { PanelError } from "./panel";
import { countsQuery } from "./queries";

const TILES = ["published", "draft", "pending", "scheduled"] as const;

export function Tiles() {
  const { t } = useTranslation("dashboard");
  const counts = useQuery(countsQuery("post"));
  const published = useQuery(publishedQuery("post"));
  const trend = useMemo(
    () =>
      published.data
        ? bucketByDay(published.data, ACTIVITY_DAYS, new Date(), browserTimeZone()).map(
            (b) => b.count,
          )
        : undefined,
    [published.data],
  );
  const recent = trend?.reduce((a, b) => a + b, 0);
  const number = new Intl.NumberFormat(uiLocale());
  /** The caption of a tile; Published names its 30-day count (capped: "1,000+"). */
  const caption = (status: (typeof TILES)[number]): string => {
    if (status !== "published") return t(`tiles.caption.${status}`);
    if (recent === undefined) return t("tiles.caption.publishedTotal");
    return published.data && isCapped(published.data)
      ? t("tiles.caption.publishedCapped", {
          formatted: number.format(PUBLISHED_CAP),
          days: ACTIVITY_DAYS,
        })
      : t("tiles.caption.publishedRecent", {
          count: recent,
          formatted: number.format(recent),
          days: ACTIVITY_DAYS,
        });
  };

  return (
    <section aria-labelledby="dashboard-tiles" className="flex flex-col gap-3">
      <h2 id="dashboard-tiles" className="sr-only">
        {t("tiles.title")}
      </h2>
      {counts.isError ? (
        <PanelError
          error={counts.error}
          retry={() => {
            void counts.refetch();
          }}
        />
      ) : (
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          {TILES.map((status) =>
            counts.data ? (
              <MetricTile
                key={status}
                label={t(`tiles.${status}`)}
                value={counts.data[status]}
                href={`/content/post?status=${status}`}
                caption={caption(status)}
                {...(status === "published" && trend ? { series: trend } : {})}
                className="min-h-28"
              />
            ) : (
              <TileSkeleton key={status} />
            ),
          )}
        </div>
      )}
    </section>
  );
}

function TileSkeleton() {
  return (
    <div
      aria-hidden="true"
      data-slot="tile-skeleton"
      className="flex min-h-28 flex-col gap-2 rounded-xl border bg-card p-4 shadow-1"
    >
      <Skeleton className="h-4 w-24" />
      <Skeleton className="h-7 w-12" />
    </div>
  );
}
