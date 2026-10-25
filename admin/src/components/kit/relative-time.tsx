// "3 min ago", with the absolute time in `title` and `dateTime`; refreshes each minute.

import { format, formatDistanceToNowStrict } from "date-fns";
import { cn } from "cn";
import { useEffect, useState } from "react";

export interface RelativeTimeProps {
  /** An RFC 3339 timestamp as the API sends it. */
  at: string;
  className?: string;
}

export function RelativeTime({ at, className }: RelativeTimeProps) {
  const [, tick] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => tick((n) => n + 1), 60_000);
    return () => window.clearInterval(id);
  }, []);
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return null;
  return (
    <time dateTime={at} title={format(date, "PPpp")} className={cn("tabular-nums", className)}>
      {formatDistanceToNowStrict(date, { addSuffix: true })}
    </time>
  );
}
