// The upload tray: a bottom-right panel listing the queue (`upload-queue.ts`), one row per
// file with its name (as text), a progress bar with the percent, the error's detail and
// Retry / Cancel. It collapses to its header; completions and failures are announced
// politely. While anything uploads, leaving the screen asks first (router navigation
// through `useLeaveGuard`, closing or reloading the tab through `beforeunload`), and
// leaving anyway cancels what has not finished.

import { cn } from "cn";
import { CheckIcon, ChevronDownIcon, CircleAlertIcon, RotateCwIcon, XIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { MediaDto } from "@/api/media";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { useLeaveGuard } from "@/editor/leave-guard";
import { uiLocale } from "@/lib/locale";
import type { UploadEntry, UploadQueue } from "./upload-queue";
import { uploadErrorText } from "./use-upload-queue";

export interface UploadTrayProps {
  queue: UploadQueue;
  entries: readonly UploadEntry[];
  /** Lift the tray above the floating selection bar (phones). */
  raised?: boolean;
  /** Opens an uploaded item's details. */
  onOpen: (media: MediaDto) => void;
}

export function UploadTray({ queue, entries, raised = false, onOpen }: UploadTrayProps) {
  const { t } = useTranslation("media");
  const [collapsed, setCollapsed] = useState(false);
  const active = entries.some((e) => e.state === "queued" || e.state === "uploading");
  const guard = useLeaveGuard(active);
  const announcement = useAnnouncement(entries);
  const panel = useRef<HTMLElement>(null);
  const shown = entries.length > 0;
  // Lift the toasts above the tray while it is shown (`--toast-lift`, read by the shell's
  // Toaster), so an Undo toast never covers the uploads or the other way round.
  useEffect(() => {
    const el = panel.current;
    const root = document.documentElement;
    if (!shown || !el) return;
    const measure = () => root.style.setProperty("--toast-lift", `${el.offsetHeight + 12}px`);
    measure();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(el);
    return () => {
      ro?.disconnect();
      root.style.removeProperty("--toast-lift");
    };
  }, [shown]);

  const done = entries.filter((e) => e.state === "done").length;
  const failed = entries.filter((e) => e.state === "failed").length;
  const counted = entries.filter((e) => e.state !== "cancelled").length;
  const heading = active
    ? t("tray.uploading", { done, total: counted })
    : failed > 0
      ? t("tray.someFailed", { count: failed })
      : t("tray.done", { count: done });

  const { loaded, total } = entries
    .filter((e) => e.state !== "cancelled" && e.state !== "failed")
    .reduce((acc, e) => ({ loaded: acc.loaded + e.loaded, total: acc.total + e.total }), {
      loaded: 0,
      total: 0,
    });

  return (
    <>
      {entries.length > 0 ? (
        <section
          ref={panel}
          aria-labelledby="upload-tray-heading"
          data-slot="upload-tray"
          className={cn(
            "fixed right-3 z-40 flex w-[min(24rem,calc(100vw-1.5rem))] flex-col overflow-hidden rounded-xl border bg-popover text-popover-foreground shadow-3 sm:right-6",
            raised ? "bottom-20 sm:bottom-24" : "bottom-3 sm:bottom-6",
          )}
        >
          <header className="flex h-12 shrink-0 items-center gap-1 pr-1.5 pl-4">
            <h2
              id="upload-tray-heading"
              className="min-w-0 flex-1 truncate text-sm font-medium tabular-nums"
              title={heading}
            >
              {heading}
            </h2>
            <Button
              variant="ghost"
              size="icon-sm"
              className="pointer-coarse:size-11"
              aria-expanded={!collapsed}
              aria-controls="upload-tray-list"
              aria-label={collapsed ? t("tray.expand") : t("tray.collapse")}
              onClick={() => setCollapsed((c) => !c)}
            >
              <ChevronDownIcon
                className={cn(
                  "transition-transform duration-(--dur-fast)",
                  collapsed && "rotate-180",
                )}
              />
            </Button>
            {active ? null : (
              <Button
                variant="ghost"
                size="icon-sm"
                className="pointer-coarse:size-11"
                aria-label={t("tray.close")}
                onClick={() => {
                  for (const e of entries) if (e.state === "failed") queue.cancel(e.id);
                  queue.clearDone();
                }}
              >
                <XIcon />
              </Button>
            )}
          </header>
          {active ? <Bar value={total > 0 ? loaded / total : 0} label={t("tray.overall")} /> : null}
          <ul
            id="upload-tray-list"
            hidden={collapsed}
            className="max-h-[min(18rem,40svh)] overflow-y-auto border-t"
          >
            {entries.map((entry) => (
              <Row key={entry.id} entry={entry} queue={queue} onOpen={onOpen} />
            ))}
          </ul>
        </section>
      ) : null}
      <p className="sr-only" aria-live="polite">
        {announcement}
      </p>
      <AlertDialog open={guard.blocked}>
        <AlertDialogContent data-slot="upload-leave-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("leave.title")}</AlertDialogTitle>
            <AlertDialogDescription>{t("leave.body")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel onClick={guard.stay}>{t("leave.stay")}</AlertDialogCancel>
            <Button
              variant="destructive"
              className="bg-destructive text-background hover:bg-destructive/90 dark:bg-destructive dark:hover:bg-destructive/90"
              onClick={() => {
                queue.dispose();
                guard.leave();
              }}
            >
              {t("leave.leave")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function Row({
  entry,
  queue,
  onOpen,
}: {
  entry: UploadEntry;
  queue: UploadQueue;
  onOpen: (media: MediaDto) => void;
}) {
  const { t } = useTranslation("media");
  const name = entry.file.name;
  const fraction = entry.total > 0 ? Math.min(1, entry.loaded / entry.total) : 0;
  const percent = new Intl.NumberFormat(uiLocale(), { style: "percent" }).format(fraction);
  const running = entry.state === "uploading" || entry.state === "queued";
  const status =
    entry.state === "uploading"
      ? percent
      : entry.state === "queued"
        ? entry.waiting
          ? t("tray.state.retrying")
          : t("tray.state.queued")
        : t(`tray.state.${entry.state}`);
  return (
    <li
      data-slot="upload-row"
      data-state={entry.state}
      className="flex flex-col gap-1.5 border-b px-4 py-2.5 last:border-b-0"
    >
      <div className="flex min-h-8 items-center gap-2">
        <StateIcon entry={entry} />
        <span className="min-w-0 flex-1 truncate text-sm" title={name}>
          {name}
        </span>
        <span
          className={cn(
            "shrink-0 text-xs tabular-nums",
            entry.state === "failed" ? "text-destructive" : "text-muted-foreground",
          )}
        >
          {status}
        </span>
        {running ? (
          <Button
            variant="ghost"
            size="icon-sm"
            className="pointer-coarse:size-11"
            aria-label={t("tray.cancel", { name })}
            onClick={() => queue.cancel(entry.id)}
          >
            <XIcon />
          </Button>
        ) : entry.state === "failed" ? (
          <Button
            variant="ghost"
            size="sm"
            className="pointer-coarse:h-11"
            aria-label={t("tray.retryName", { name })}
            onClick={() => queue.retry(entry.id)}
          >
            <RotateCwIcon data-icon="inline-start" />
            {t("tray.retry")}
          </Button>
        ) : entry.state === "done" && entry.media ? (
          <Button
            variant="ghost"
            size="sm"
            className="pointer-coarse:h-11"
            aria-label={t("tray.openName", { name })}
            onClick={() => {
              if (entry.media) onOpen(entry.media);
            }}
          >
            {t("tray.open")}
          </Button>
        ) : null}
      </div>
      {running ? (
        <Bar
          value={entry.state === "uploading" ? fraction : 0}
          label={t("tray.progress", { name })}
        />
      ) : null}
      {entry.state === "failed" || (entry.waiting && entry.error) ? (
        <p data-slot="upload-error" className="text-sm break-words text-destructive">
          {uploadErrorText(entry.error, t)}
        </p>
      ) : null}
    </li>
  );
}

function StateIcon({ entry }: { entry: UploadEntry }) {
  if (entry.state === "done") {
    return <CheckIcon aria-hidden="true" className="size-4 shrink-0 text-success" />;
  }
  if (entry.state === "failed") {
    return <CircleAlertIcon aria-hidden="true" className="size-4 shrink-0 text-destructive" />;
  }
  return null;
}

/** A thin progress bar; `value` 0..1. */
function Bar({ value, label }: { value: number; label: string }) {
  const percent = Math.round(value * 100);
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      className="h-1 w-full overflow-hidden rounded-full bg-muted"
    >
      <div
        className="h-full origin-left rounded-full bg-primary transition-transform duration-(--dur-base) ease-out motion-reduce:transition-none"
        style={{ transform: `scaleX(${value})` }}
      />
    </div>
  );
}

/** The last completion or failure, for the polite live region. */
function useAnnouncement(entries: readonly UploadEntry[]): string {
  const { t } = useTranslation("media");
  const [seen, setSeen] = useState(entries);
  const [message, setMessage] = useState("");
  if (seen !== entries) {
    const before = new Map(seen.map((e) => [e.id, e.state]));
    let next = "";
    for (const e of entries) {
      if (before.get(e.id) === e.state) continue;
      if (e.state === "done") next = t("tray.announce.done", { name: e.file.name });
      if (e.state === "failed") {
        next = t("tray.announce.failed", {
          name: e.file.name,
          reason: uploadErrorText(e.error, t),
        });
      }
    }
    setSeen(entries);
    if (next) setMessage(next);
  }
  return message;
}
