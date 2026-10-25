// A trend line without axes. Decorative: `aria-hidden`, so whoever renders one also
// gives the value in text (MetricTile does).

import { useId } from "react";
import { Area, AreaChart } from "recharts";
import { cn } from "cn";

export interface SparklineProps {
  data: number[];
  className?: string;
}

export function Sparkline({ data, className }: SparklineProps) {
  const gradient = `spark-${useId().replace(/:/g, "")}`;
  const points = data.map((value, index) => ({ index, value }));
  return (
    <div aria-hidden="true" data-slot="sparkline" className={cn("h-10 w-full", className)}>
      <AreaChart
        responsive
        data={points}
        margin={{ top: 2, right: 0, bottom: 2, left: 0 }}
        accessibilityLayer={false}
        style={{ width: "100%", height: "100%" }}
      >
        <defs>
          <linearGradient id={gradient} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="var(--chart-1)" stopOpacity={0.28} />
            <stop offset="100%" stopColor="var(--chart-1)" stopOpacity={0} />
          </linearGradient>
        </defs>
        <Area
          type="monotone"
          dataKey="value"
          stroke="var(--chart-1)"
          strokeWidth={1.5}
          fill={`url(#${gradient})`}
          dot={false}
          activeDot={false}
          isAnimationActive={false}
        />
      </AreaChart>
    </div>
  );
}
