// The live preview beside the editor (design 15 §3, §11.1). The pane shows the item as the
// site renders it with the working copy laid over it: a sandboxed iframe (no
// `allow-same-origin`, ruling P7 D16) on a signed preview link (`use-preview-frame.ts`),
// loaded again after each completed autosave and save. A device switch narrows the frame
// (Desktop, Tablet 768 px, Phone 390 px, centred with a 1 px border; remembered per
// viewer); a polite status line says when it was last updated. A new item has nothing to
// preview until its first save: the pane says so, with Save.
//
// `SplitDivider` is the split view's handle (≥ 1280 px): a vertical separator that moves
// with the pointer and the keyboard (arrows 5 %, Home/End to the limits, 35–65 %).

import { MonitorIcon, SmartphoneIcon, TabletIcon } from "lucide-react";
import { cn } from "cn";
import { type KeyboardEvent, type PointerEvent, type RefObject, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { uiLocale } from "@/lib/locale";
import {
  DEVICES,
  DEVICE_WIDTH,
  type Device,
  SPLIT_MAX,
  SPLIT_MIN,
  SPLIT_STEP,
  clampSplit,
  storeDevice,
  storedDevice,
} from "./preview-prefs";
import { usePreviewFrame } from "./use-preview-frame";

export interface PreviewPaneProps {
  type: string;
  /** `null`: a new item (nothing to preview until it is saved). */
  itemId: string | null;
  /** The item's title as typed (the frame's accessible name). */
  title: string;
  /** Bumped after each completed autosave or save: the frame loads again. */
  refreshKey: number;
  /** When the working copy last reached the slot (and so the preview), else `null`. */
  updatedAt: Date | null;
  /** Change mode: the release whose staged change is previewed. */
  releaseId?: string | null;
  /** A new item's Save (its first save makes the preview possible). */
  onSave: () => void;
  className?: string;
}

const DEVICE_ICONS = { desktop: MonitorIcon, tablet: TabletIcon, phone: SmartphoneIcon } as const;

export function PreviewPane({
  type,
  itemId,
  title,
  refreshKey,
  updatedAt,
  releaseId,
  onSave,
  className,
}: PreviewPaneProps) {
  const { t } = useTranslation("editor");
  const [device, setDevice] = useState<Device>(storedDevice);
  const frame = usePreviewFrame({
    type,
    itemId,
    releaseId: releaseId ?? null,
    refreshKey,
    enabled: itemId !== null,
  });
  const name = title.trim() || t("title.untitled");
  const time = updatedAt
    ? new Intl.DateTimeFormat(uiLocale(), {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hourCycle: "h23",
      }).format(updatedAt)
    : null;

  return (
    <section
      aria-label={t("preview.label")}
      data-slot="preview-pane"
      className={cn("flex min-h-0 min-w-0 flex-col bg-muted/30", className)}
    >
      <div className="flex h-11 shrink-0 items-center justify-between gap-2 border-b bg-background px-3">
        <h2 className="text-sm font-semibold">{t("preview.heading")}</h2>
        <ToggleGroup
          type="single"
          spacing={0}
          value={device}
          aria-label={t("preview.device")}
          onValueChange={(v) => {
            const next = DEVICES.find((d) => d === v);
            if (!next) return;
            setDevice(next);
            storeDevice(next);
          }}
          className="rounded-lg bg-muted p-0.5"
        >
          {DEVICES.map((d) => {
            const Icon = DEVICE_ICONS[d];
            return (
              <ToggleGroupItem
                key={d}
                value={d}
                aria-label={t(`preview.devices.${d}`)}
                title={t(`preview.devices.${d}`)}
                className="h-7 rounded-md! border-0 px-2 text-muted-foreground pointer-coarse:h-11 pointer-coarse:min-w-11 data-[state=on]:bg-background data-[state=on]:text-foreground data-[state=on]:shadow-(--shadow-1)"
              >
                <Icon aria-hidden="true" className="size-4" />
              </ToggleGroupItem>
            );
          })}
        </ToggleGroup>
      </div>
      <div className="flex min-h-0 flex-1 justify-center overflow-auto p-0">
        {itemId === null ? (
          <div
            data-slot="preview-unsaved"
            className="m-auto flex max-w-xs flex-col items-center gap-3 p-6 text-center"
          >
            <p className="text-sm text-muted-foreground">{t("preview.saveFirst")}</p>
            <Button type="button" variant="outline" onClick={onSave}>
              {t("save.button")}
            </Button>
          </div>
        ) : frame.failed ? (
          <div className="m-auto flex max-w-xs flex-col items-center gap-3 p-6 text-center">
            <p role="alert" className="text-sm text-muted-foreground">
              {t("preview.failed")}
            </p>
            <Button type="button" variant="outline" onClick={frame.retry}>
              {t("preview.retry")}
            </Button>
          </div>
        ) : (
          <div
            data-slot="preview-frame-box"
            style={{ width: DEVICE_WIDTH[device] }}
            className={cn(
              "h-full max-w-full shrink-0 bg-background",
              device !== "desktop" && "my-3 border",
            )}
          >
            <iframe
              title={t("preview.frameTitle", { title: name })}
              data-slot="preview-frame"
              // P7 D16: an opaque origin; the theme's and plugins' scripts run, but cannot
              // reach the admin.
              sandbox="allow-scripts allow-popups allow-forms"
              referrerPolicy="no-referrer"
              {...(frame.src ? { src: frame.src } : {})}
              className="block size-full border-0"
            />
          </div>
        )}
      </div>
      <p
        role="status"
        aria-live="polite"
        data-slot="preview-status"
        className="flex h-8 shrink-0 items-center border-t bg-background px-3 text-xs text-muted-foreground tabular-nums"
      >
        {time ? t("preview.updated", { time }) : t("preview.saved")}
      </p>
    </section>
  );
}

export interface SplitDividerProps {
  /** The writing side's share of the width, in percent (35–65). */
  value: number;
  onChange: (value: number) => void;
  /** The element whose width the percentage is of (pointer drags). */
  container?: RefObject<HTMLElement | null>;
}

export function SplitDivider({ value, onChange, container }: SplitDividerProps) {
  const { t } = useTranslation("editor");
  const dragging = useRef(false);
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    let next: number | null = null;
    if (e.key === "ArrowLeft") next = value - SPLIT_STEP;
    else if (e.key === "ArrowRight") next = value + SPLIT_STEP;
    else if (e.key === "Home") next = SPLIT_MIN;
    else if (e.key === "End") next = SPLIT_MAX;
    if (next === null) return;
    e.preventDefault();
    onChange(clampSplit(next));
  };
  const onMove = (e: PointerEvent<HTMLDivElement>) => {
    const el = container?.current;
    if (!dragging.current || !el) return;
    const box = el.getBoundingClientRect();
    if (box.width <= 0) return;
    onChange(clampSplit(Math.round(((e.clientX - box.left) / box.width) * 100)));
  };
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={t("preview.resize")}
      aria-valuenow={value}
      aria-valuemin={SPLIT_MIN}
      aria-valuemax={SPLIT_MAX}
      tabIndex={0}
      data-slot="preview-divider"
      onKeyDown={onKey}
      onPointerDown={(e) => {
        dragging.current = true;
        e.currentTarget.setPointerCapture?.(e.pointerId);
      }}
      onPointerMove={onMove}
      onPointerUp={(e) => {
        dragging.current = false;
        e.currentTarget.releasePointerCapture?.(e.pointerId);
      }}
      className="group relative z-10 w-2 shrink-0 cursor-col-resize self-stretch touch-none bg-transparent outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <span
        aria-hidden="true"
        className="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border transition-colors duration-(--dur-fast) group-hover:bg-primary group-focus-visible:bg-primary"
      />
    </div>
  );
}
