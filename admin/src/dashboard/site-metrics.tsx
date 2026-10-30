// Site traffic (`manage_system`, P36): the Traffic and Performance cards over one time
// range, every node's figures summed (`GET /system/metrics/series`, hourly). The range
// control sits once above both cards, so their numbers always cover the same hours;
// both read the same query, so a range costs one request. The choice is not kept: the
// dashboard opens on 24 hours.

import { cn } from "cn";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { METRICS_RANGES, type MetricsRange } from "@/api/system";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { PerformanceCard } from "./performance-card";
import { TrafficCard } from "./traffic-card";

function isRange(value: string): value is MetricsRange {
  return (METRICS_RANGES as readonly string[]).includes(value);
}

export function SiteMetrics({ className }: { className?: string }) {
  const { t } = useTranslation("dashboard");
  const [range, setRange] = useState<MetricsRange>("24h");
  const heading = useId();
  return (
    <section aria-labelledby={heading} className={cn("flex min-w-0 flex-col gap-4", className)}>
      <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-3">
        <div className="flex min-w-0 flex-col gap-0.5">
          <h2 id={heading} className="text-md font-semibold text-foreground">
            {t("metrics.title")}
          </h2>
          <p className="text-sm text-muted-foreground">{t("metrics.description")}</p>
        </div>
        <ToggleGroup
          type="single"
          variant="outline"
          spacing={0}
          value={range}
          aria-label={t("metrics.range")}
          onValueChange={(v) => {
            // Radix sends "" when the pressed item is pressed again: keep the range.
            if (isRange(v)) setRange(v);
          }}
        >
          {METRICS_RANGES.map((r) => (
            <ToggleGroupItem
              key={r}
              value={r}
              className="px-3 pointer-coarse:h-11"
              // The items are radios (`role="radio"`): an arrow key that moves focus also
              // selects, as the ARIA radio group pattern has it (Radix only moves focus).
              onFocus={() => setRange(r)}
            >
              {t(`metrics.ranges.${r}`)}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </div>
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-12">
        <TrafficCard range={range} className="lg:col-span-8" />
        <PerformanceCard range={range} className="lg:col-span-4" />
      </div>
    </section>
  );
}
