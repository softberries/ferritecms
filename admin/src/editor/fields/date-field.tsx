// A `date` field: a button showing the instant on the browser's clock (with its zone),
// opening a calendar and a 24-hour time as in the schedule picker; the value is stored as
// RFC 3339 UTC (`2026-11-03T08:30:00Z`). Clear stores `null`. A stored value that is not
// a date shows as it is, so nothing is silently replaced.

import { CalendarIcon, GlobeIcon } from "lucide-react";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Calendar } from "@/components/ui/calendar";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  type WallClock,
  browserTimeZone,
  dateFromKey,
  dateKey,
  formatWhen,
  utcToWallClock,
  wallClockToUtc,
  zoneOffsetLabel,
} from "../panel/publishing";
import type { ControlIds } from "./fields-context";
import { isRfc3339 } from "./fields-model";

export function DateField({
  ids,
  value,
  onChange,
  disabled,
}: {
  ids: ControlIds;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation("editor");
  const [timeZone] = useState(browserTimeZone);
  const [now] = useState(() => Date.now());
  const [open, setOpen] = useState(false);
  const timeId = useId();
  const stored = typeof value === "string" ? value : null;
  const valid = stored !== null && isRfc3339(stored);
  const [wall, setWall] = useState<WallClock>(
    () => (valid ? utcToWallClock(stored, timeZone) : null) ?? { date: "", time: "09:00" },
  );
  const commit = (next: WallClock) => {
    setWall(next);
    const utc = wallClockToUtc(next, timeZone);
    if (utc) onChange(utc);
  };
  const selected = dateFromKey(wall.date);
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Popover
        open={open}
        onOpenChange={(next) => {
          if (next) {
            setWall(
              (valid ? utcToWallClock(stored, timeZone) : null) ?? { date: "", time: "09:00" },
            );
          }
          setOpen(next);
        }}
      >
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="outline"
            id={ids.id}
            aria-describedby={ids.describedBy}
            {...(ids.invalid ? { "aria-invalid": true } : {})}
            disabled={disabled}
            className="h-9 min-w-0 justify-start gap-2 font-normal tabular-nums"
          >
            <CalendarIcon data-icon="inline-start" />
            <span className="truncate">
              {stored === null ? t("fields.datePlaceholder") : valid ? formatWhen(stored) : stored}
            </span>
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="flex w-auto flex-col gap-3">
          <Calendar
            mode="single"
            selected={selected}
            onSelect={(day: Date | undefined) => {
              if (day) commit({ ...wall, date: dateKey(day) });
            }}
            {...(selected ? { defaultMonth: selected } : {})}
            className="rounded-lg border bg-card p-2"
          />
          <div className="flex items-end gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor={timeId}>{t("schedule.time")}</Label>
              <Input
                id={timeId}
                type="time"
                step={60}
                value={wall.time}
                onChange={(e) => {
                  const next = { ...wall, time: e.target.value };
                  if (next.date) commit(next);
                  else setWall(next);
                }}
                className="w-32 font-mono tabular-nums"
              />
            </div>
            <p className="flex items-center gap-1.5 pb-2 text-xs text-muted-foreground">
              <GlobeIcon aria-hidden="true" className="size-3.5 shrink-0" />
              {t("schedule.zone", {
                zone: timeZone.replace(/_/g, " "),
                offset: zoneOffsetLabel(timeZone, valid ? Date.parse(stored) : now),
              })}
            </p>
          </div>
          <div className="flex justify-end gap-2">
            {stored !== null ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => {
                  onChange(null);
                  setOpen(false);
                }}
              >
                {t("fields.clear")}
              </Button>
            ) : null}
            <Button type="button" size="sm" onClick={() => setOpen(false)}>
              {t("fields.done")}
            </Button>
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}
