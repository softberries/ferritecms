// The publish dialog (11 §1, after Ghost): publishing is a deliberate step. It sums up
// what goes out (title, address, terms, excerpt, unsaved changes that will be saved
// first), offers "Now" or "Later" (the schedule picker; only when `allowed.schedule`),
// and confirms with a button that says exactly what will happen. A failure of the
// transition stays in the dialog; a 412 goes to the editor's conflict handler.
// M6-4: the accessibility errors of the last check are listed, and when the type's gate
// is on (`errors`) the confirm button is disabled while they remain; a `422
// content-quality` from the server shows its own list (design 16 §11.1).

import { CalendarClockIcon, ZapIcon } from "lucide-react";
import { cn } from "cn";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ApiError, isProblem } from "@/api/problem";
import type { CheckResponse, FindingDto } from "@/api/quality";
import type { TaxonomyDto, TermDto } from "@/api/taxonomies";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { uiLocale } from "@/lib/locale";
import type { EditorDocument } from "../editor-state";
import { blockingFindings, findingId, messageFor } from "../quality/findings";
import {
  type WallClock,
  browserTimeZone,
  defaultScheduleWall,
  isPast,
  predictPermalink,
  wallClockToUtc,
} from "./publishing";
import { SchedulePicker } from "./schedule-picker";

export type PublishMode = "now" | "schedule";

export interface PublishDialogProps {
  doc: EditorDocument;
  /** `null` closes it. */
  mode: PublishMode | null;
  onClose: () => void;
  siteUrl: string | null;
  taxonomies: TaxonomyDto[];
  terms: Map<string, TermDto>;
  /** The last check (M6-4); `undefined`: no checks on this screen. */
  quality?: CheckResponse | null;
}

export function PublishDialog(props: PublishDialogProps) {
  // Mounted only while open, so every opening starts from a fresh state.
  return (
    <Dialog
      open={props.mode !== null}
      onOpenChange={(open) => {
        if (!open) props.onClose();
      }}
    >
      {props.mode !== null ? <PublishDialogBody {...props} mode={props.mode} /> : null}
    </Dialog>
  );
}

function PublishDialogBody({
  doc,
  mode: initialMode,
  onClose,
  siteUrl,
  taxonomies,
  terms,
  quality,
}: PublishDialogProps & { mode: PublishMode }) {
  const { t } = useTranslation("editor");
  const { t: tq } = useTranslation("quality");
  const [refused, setRefused] = useState<FindingDto[] | null>(null);
  const blocking = quality ? blockingFindings(quality.findings) : [];
  const gated = quality?.gate.mode === "errors" && quality.gate.blocked;
  const item = doc.item;
  const canSchedule = item?.allowed.schedule === true;
  const canNow = item?.allowed.publish === true;
  const [mode, setMode] = useState<PublishMode>(
    initialMode === "schedule" && canSchedule ? "schedule" : canNow ? "now" : "schedule",
  );
  const [zone] = useState(browserTimeZone);
  const [wall, setWall] = useState<WallClock>(() => defaultScheduleWall(zone));
  const [busy, setBusy] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);

  const at = wallClockToUtc(wall, zone);
  const past = mode === "schedule" && (at === null || isPast(at));
  const scheduleError =
    mode !== "schedule"
      ? null
      : at === null
        ? t("schedule.invalid")
        : past
          ? t("schedule.past")
          : null;

  const whenLabel = at
    ? new Intl.DateTimeFormat(uiLocale(), {
        weekday: "short",
        day: "numeric",
        month: "short",
        hour: "2-digit",
        minute: "2-digit",
        // 24 h, as the time field shows it.
        hourCycle: "h23",
      }).format(new Date(at))
    : "";

  const title = doc.title.trim() || t("title.untitled");
  const permalink = item
    ? (predictPermalink(item.permalink, doc.savedMeta.slug, doc.meta.slug) ?? item.permalink)
    : "";
  const host = siteUrl ? siteUrl.replace(/^https?:\/\//, "").replace(/\/$/, "") : "";
  const chosen = doc.meta.terms.map((id) => terms.get(id)).filter((x): x is TermDto => !!x);

  const confirm = async () => {
    if (!item || (mode === "schedule" && (past || at === null))) return;
    setBusy(true);
    setServerError(null);
    const onError = (e: unknown) => {
      if (isProblem(e, "content-quality")) {
        const listed = (e.problem as { findings?: FindingDto[] }).findings ?? [];
        setRefused(listed);
        setServerError(tq("publish.refused"));
        return;
      }
      const detail =
        e instanceof ApiError
          ? e.problem.errors?.[0]?.detail || e.problem.detail || e.problem.title
          : "";
      setServerError(detail ? t("publish.failedWith", { detail }) : t("publish.failed"));
    };
    const next =
      mode === "schedule" && at
        ? await doc.transition("schedule", { at, onError })
        : await doc.transition("publish", { onError });
    setBusy(false);
    if (!next) return;
    onClose();
    const url = siteUrl ? new URL(next.permalink, siteUrl).toString() : null;
    if (mode === "schedule") {
      toast.success(t("publish.toast.scheduled", { when: whenLabel }), { id: "editor-transition" });
    } else {
      toast.success(t("publish.toast.published"), {
        id: "editor-transition",
        ...(url
          ? {
              description: url,
              action: {
                label: t("publish.toast.view"),
                onClick: () => window.open(url, "_blank", "noopener"),
              },
            }
          : {}),
      });
    }
  };

  const confirmLabel = busy
    ? mode === "schedule"
      ? t("publish.scheduling")
      : t("publish.publishing")
    : mode === "schedule"
      ? t("publish.confirmSchedule", { when: whenLabel })
      : t("publish.confirmNow");

  return (
    <DialogContent
      data-slot="publish-dialog"
      className="max-h-[calc(100dvh-1rem)] gap-0 overflow-y-auto p-0 sm:max-w-xl"
    >
      <DialogHeader className="px-6 pt-6 pb-4">
        <DialogTitle className="text-lg">
          {item?.status === "scheduled" ? t("publish.titleScheduled") : t("publish.title")}
        </DialogTitle>
        <DialogDescription>{t("publish.description")}</DialogDescription>
      </DialogHeader>

      <div className="flex flex-col gap-5 px-6 pb-6">
        <dl
          data-slot="publish-summary"
          className="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-3 gap-y-2.5 rounded-xl border bg-muted/30 p-4 text-sm"
        >
          <dt className="text-muted-foreground">{t("publish.summary.title")}</dt>
          <dd className="font-medium break-words text-foreground">{title}</dd>
          <dt className="text-muted-foreground">{t("publish.summary.address")}</dt>
          <dd className="font-mono text-xs leading-5 break-all text-foreground">
            <span className="text-muted-foreground">{host}</span>
            {permalink}
          </dd>
          {taxonomies.map((tax) => {
            const mine = chosen.filter((term) => term.taxonomy === tax.name);
            return (
              <div key={tax.name} className="contents">
                <dt className="text-muted-foreground">{tax.label}</dt>
                <dd className="flex flex-wrap gap-1">
                  {mine.length > 0 ? (
                    mine.map((term) => (
                      <span
                        key={term.id}
                        className="inline-flex h-6 items-center rounded-md border bg-background px-2 text-xs"
                      >
                        {term.name}
                      </span>
                    ))
                  ) : (
                    <span className="text-muted-foreground">{t("publish.summary.none")}</span>
                  )}
                </dd>
              </div>
            );
          })}
          <dt className="text-muted-foreground">{t("publish.summary.excerpt")}</dt>
          <dd className="text-foreground">
            {doc.meta.excerpt.trim() === "" ? (
              <span className="text-muted-foreground">{t("publish.summary.excerptDerived")}</span>
            ) : (
              <span className="line-clamp-2">{doc.meta.excerpt}</span>
            )}
          </dd>
        </dl>
        {doc.dirty ? (
          <p data-slot="publish-unsaved" className="-mt-2 text-xs text-muted-foreground">
            {t("publish.unsaved")}
          </p>
        ) : null}
        {quality !== undefined && (blocking.length > 0 || refused) ? (
          <div
            data-slot="publish-quality"
            className="flex flex-col gap-1 rounded-lg border border-destructive/40 p-3 text-sm"
          >
            <p className="font-medium">
              {tq("publish.errors", { count: (refused ?? blocking).length })}
            </p>
            {gated ? <p className="text-muted-foreground">{tq("publish.blocked")}</p> : null}
            <ul className="list-disc pl-5">
              {(refused ?? blocking).map((f) => (
                <li key={findingId(f)}>{messageFor(tq, f)}</li>
              ))}
            </ul>
          </div>
        ) : null}

        {canSchedule && canNow ? (
          <fieldset className="flex flex-col gap-2">
            <legend className="mb-2 text-sm font-medium">{t("publish.when")}</legend>
            <div className="grid grid-cols-2 gap-2">
              <ModeOption
                value="now"
                checked={mode === "now"}
                onSelect={() => setMode("now")}
                icon={<ZapIcon aria-hidden="true" className="size-4" />}
                label={t("publish.now")}
                hint={t("publish.nowHint")}
                disabled={busy}
              />
              <ModeOption
                value="schedule"
                checked={mode === "schedule"}
                onSelect={() => setMode("schedule")}
                icon={<CalendarClockIcon aria-hidden="true" className="size-4" />}
                label={t("publish.later")}
                hint={t("publish.laterHint")}
                disabled={busy}
              />
            </div>
          </fieldset>
        ) : null}

        {mode === "schedule" ? (
          <SchedulePicker
            value={wall}
            onChange={(v) => {
              setWall(v);
              setServerError(null);
            }}
            timeZone={zone}
            error={scheduleError ?? serverError}
            disabled={busy}
          />
        ) : serverError ? (
          <p role="alert" className="text-sm text-destructive">
            {serverError}
          </p>
        ) : null}
      </div>

      <DialogFooter className="m-0 rounded-b-xl border-t bg-muted/40 px-6 py-4">
        <Button type="button" variant="outline" onClick={onClose} disabled={busy}>
          {t("publish.cancel")}
        </Button>
        <Button
          type="button"
          data-slot="publish-confirm"
          disabled={busy || gated || (mode === "schedule" && scheduleError !== null)}
          {...(gated ? { title: tq("publish.blocked") } : {})}
          onClick={() => {
            void confirm();
          }}
        >
          {confirmLabel}
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

function ModeOption({
  value,
  checked,
  onSelect,
  icon,
  label,
  hint,
  disabled,
}: {
  value: PublishMode;
  checked: boolean;
  onSelect: () => void;
  icon: ReactNode;
  label: string;
  hint: string;
  disabled: boolean;
}) {
  return (
    <label
      className={cn(
        "relative flex cursor-pointer flex-col gap-1 rounded-lg border p-3 transition-colors duration-(--dur-fast) has-focus-visible:ring-2 has-focus-visible:ring-ring has-focus-visible:ring-offset-2 has-focus-visible:ring-offset-background",
        checked ? "border-primary bg-primary/6" : "hover:bg-muted/50",
        disabled && "cursor-not-allowed opacity-60",
      )}
    >
      <input
        type="radio"
        name="publish-mode"
        value={value}
        checked={checked}
        disabled={disabled}
        onChange={onSelect}
        className="sr-only"
      />
      <span
        className={cn("flex items-center gap-2 text-sm font-medium", checked && "text-primary")}
      >
        {icon}
        {label}
      </span>
      <span className="text-xs text-muted-foreground">{hint}</span>
    </label>
  );
}
