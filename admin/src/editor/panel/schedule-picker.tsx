// The schedule picker (publish dialog, "Later"): a calendar and a 24-hour time field on
// the browser's clock, with the time zone named next to them and the same instant in
// UTC beneath, so it is never a guess what "09:00" means. The value is a wall clock;
// `wallClockToUtc` turns it into the RFC 3339 UTC time the API takes. Past days are
// disabled; a past time on today is refused by the dialog (and by the API).

import { GlobeIcon } from "lucide-react";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Calendar } from "@/components/ui/calendar";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  type WallClock,
  dateFromKey,
  dateKey,
  wallClockToUtc,
  zoneOffsetLabel,
} from "./publishing";

export interface SchedulePickerProps {
  value: WallClock;
  onChange: (value: WallClock) => void;
  /** The browser's IANA zone (`browserTimeZone()`). */
  timeZone: string;
  /** Why the chosen time cannot be used (in the past, refused by the server). */
  error?: string | null;
  disabled?: boolean;
}

export function SchedulePicker({
  value,
  onChange,
  timeZone,
  error,
  disabled,
}: SchedulePickerProps) {
  const { t } = useTranslation("editor");
  const timeId = useId();
  const zoneId = useId();
  const errorId = useId();
  const selected = dateFromKey(value.date);
  const utc = wallClockToUtc(value, timeZone);
  // Today's start, taken once: the calendar's earliest day (and the offset shown while
  // the value is not a valid time).
  const [today] = useState(() => {
    const d = new Date();
    d.setHours(0, 0, 0, 0);
    return d;
  });
  const at = utc ? Date.parse(utc) : today.getTime();
  return (
    <div data-slot="schedule-picker" className="flex flex-col gap-3 sm:flex-row sm:items-start">
      <Calendar
        mode="single"
        required
        selected={selected}
        onSelect={(day: Date | undefined) => {
          if (day) onChange({ ...value, date: dateKey(day) });
        }}
        disabled={disabled ? true : { before: today }}
        startMonth={today}
        {...(selected ? { defaultMonth: selected } : {})}
        className="rounded-lg border bg-card p-2 max-sm:self-center"
      />
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={timeId}>{t("schedule.time")}</Label>
          <Input
            id={timeId}
            type="time"
            step={60}
            required
            disabled={disabled}
            value={value.time}
            aria-describedby={`${zoneId}${error ? ` ${errorId}` : ""}`}
            aria-invalid={error ? true : undefined}
            onChange={(e) => onChange({ ...value, time: e.target.value })}
            className="w-32 font-mono tabular-nums"
          />
        </div>
        <div
          id={zoneId}
          data-slot="schedule-zone"
          className="flex flex-col gap-1 rounded-md bg-muted/60 px-3 py-2 text-xs text-muted-foreground"
        >
          <span className="flex items-center gap-1.5 text-foreground">
            <GlobeIcon aria-hidden="true" className="size-3.5 shrink-0" />
            {t("schedule.zone", {
              zone: timeZone.replace(/_/g, " "),
              offset: zoneOffsetLabel(timeZone, at),
            })}
          </span>
          {utc ? (
            <span className="font-mono tabular-nums">
              {t("schedule.utc", { at: utc.slice(0, 16).replace("T", " ") })}
            </span>
          ) : null}
        </div>
        {error ? (
          <p id={errorId} role="alert" className="text-sm text-destructive">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  );
}
