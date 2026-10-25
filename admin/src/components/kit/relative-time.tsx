// "3 min ago", with the absolute time in `title` and `dateTime`; refreshes every 30 s.
// Within `JUST_NOW_MS` of now it says "just now" (never "0 seconds ago").

import { format, formatDistanceStrict } from "date-fns";
import { cn } from "cn";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

/** Closer to now than this reads "just now". */
export const JUST_NOW_MS = 45_000;

export interface RelativeTimeProps {
  /** An RFC 3339 timestamp as the API sends it. */
  at: string;
  className?: string;
}

export function RelativeTime({ at, className }: RelativeTimeProps) {
  const { t } = useTranslation();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return null;
  return (
    <time dateTime={at} title={format(date, "PPpp")} className={cn("tabular-nums", className)}>
      {Math.abs(now - date.getTime()) < JUST_NOW_MS
        ? t("kit.time.justNow")
        : formatDistanceStrict(date, now, { addSuffix: true })}
    </time>
  );
}
