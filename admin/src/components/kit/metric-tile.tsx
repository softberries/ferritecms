// A number with its label, an optional change on the previous period and a trend.
// The value and the change are always text; the sparkline only illustrates them.

import { Link } from "@tanstack/react-router";
import { ArrowDownRightIcon, ArrowRightIcon, ArrowUpRightIcon } from "lucide-react";
import { cn } from "cn";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Sparkline } from "./sparkline";

export interface MetricTileProps {
  label: string;
  value: number;
  /** Change on the previous period, in the value's own unit. */
  delta?: number;
  series?: number[];
  /** An admin path (without `/admin`); the whole tile links there. */
  href?: string;
  className?: string;
}

const number = new Intl.NumberFormat("en");
const signed = new Intl.NumberFormat("en", { signDisplay: "exceptZero" });

export function MetricTile({ label, value, delta, series, href, className }: MetricTileProps) {
  const { t } = useTranslation();
  const body: ReactNode = (
    <>
      <span className="text-sm font-medium text-muted-foreground">{label}</span>
      <span className="flex items-baseline gap-2">
        <span className="text-2xl font-semibold text-foreground tabular-nums">
          {number.format(value)}
        </span>
        {delta !== undefined ? <Delta delta={delta} /> : null}
      </span>
      {delta !== undefined ? (
        <span className="sr-only">
          {delta > 0
            ? t("kit.metric.up", { value: number.format(delta) })
            : delta < 0
              ? t("kit.metric.down", { value: number.format(-delta) })
              : t("kit.metric.flat")}
        </span>
      ) : null}
      {series && series.length > 1 ? <Sparkline data={series} className="mt-1" /> : null}
    </>
  );
  const classes = cn(
    "flex min-w-0 flex-col gap-1 rounded-xl border bg-card p-4 text-card-foreground shadow-1",
    href && "transition-colors hover:border-input hover:bg-muted/40",
    className,
  );
  return href ? (
    <Link to={href} data-slot="metric-tile" className={classes}>
      {body}
    </Link>
  ) : (
    <div data-slot="metric-tile" className={classes}>
      {body}
    </div>
  );
}

function Delta({ delta }: { delta: number }) {
  const Icon = delta > 0 ? ArrowUpRightIcon : delta < 0 ? ArrowDownRightIcon : ArrowRightIcon;
  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-flex items-center gap-0.5 text-sm font-medium tabular-nums",
        delta > 0 ? "text-success" : delta < 0 ? "text-destructive" : "text-muted-foreground",
      )}
    >
      <Icon className="size-3.5" />
      {signed.format(delta)}
    </span>
  );
}
