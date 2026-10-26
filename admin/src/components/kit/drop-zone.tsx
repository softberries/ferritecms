// A file drop target (11 §7): the whole area takes dropped files, with an overlay while
// files are dragged over it, plus a "Choose files" button for the keyboard and touch
// screens (a hidden `<input type="file" multiple>`). Drags that carry no files (text,
// links, a sortable row) are left alone. `accept` is the input's attribute and also
// filters dropped files; the rest go to `onRejected`. The server decides the real type
// from the content (07 §1), so this filter is only a convenience.

import { cn } from "cn";
import { UploadIcon } from "lucide-react";
import { useRef, useState, type DragEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";

export interface DropZoneProps {
  onFiles: (files: File[]) => void;
  /** As the `accept` attribute: `image/*`, `.pdf`, `application/zip`, comma-separated. */
  accept?: string;
  /** Dropped files `accept` does not match. */
  onRejected?: (files: File[]) => void;
  children: ReactNode;
  /** Show the "Choose files" row (default); `false` when the screen has its own Upload
   * button (the media library). */
  chooser?: boolean;
  className?: string;
}

/** Whether `file` matches an `accept` list (by MIME type, `type/*`, or extension). */
function accepts(accept: string | undefined, file: File): boolean {
  const tokens = (accept ?? "")
    .split(",")
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);
  if (tokens.length === 0) return true;
  const name = file.name.toLowerCase();
  const type = file.type.toLowerCase();
  return tokens.some((tok) => {
    if (tok.startsWith(".")) return name.endsWith(tok);
    if (tok.endsWith("/*")) return type.startsWith(tok.slice(0, -1));
    return type === tok;
  });
}

const carriesFiles = (e: DragEvent) => Array.from(e.dataTransfer.types).includes("Files");

export function DropZone({
  onFiles,
  accept,
  onRejected,
  children,
  chooser = true,
  className,
}: DropZoneProps) {
  const { t } = useTranslation("kit");
  const input = useRef<HTMLInputElement>(null);
  // dragenter/dragleave fire for every child crossed; count them.
  const depth = useRef(0);
  const [over, setOver] = useState(false);

  const take = (files: File[]) => {
    const ok = files.filter((f) => accepts(accept, f));
    const refused = files.filter((f) => !accepts(accept, f));
    if (ok.length > 0) onFiles(ok);
    if (refused.length > 0) onRejected?.(refused);
  };

  return (
    <div
      data-slot="drop-zone"
      data-over={over || undefined}
      className={cn("relative flex flex-col gap-4", className)}
      onDragEnter={(e) => {
        if (!carriesFiles(e)) return;
        e.preventDefault();
        depth.current += 1;
        setOver(true);
      }}
      onDragOver={(e) => {
        if (!carriesFiles(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }}
      onDragLeave={(e) => {
        if (!carriesFiles(e)) return;
        depth.current = Math.max(0, depth.current - 1);
        if (depth.current === 0) setOver(false);
      }}
      onDrop={(e) => {
        if (!carriesFiles(e)) return;
        e.preventDefault();
        depth.current = 0;
        setOver(false);
        take(Array.from(e.dataTransfer.files));
      }}
    >
      {chooser ? (
        <div className="flex flex-wrap items-center gap-3">
          <Button variant="outline" onClick={() => input.current?.click()}>
            <UploadIcon data-icon="inline-start" />
            {t("dropZone.choose")}
          </Button>
          <span className="text-sm text-muted-foreground">{t("dropZone.hint")}</span>
          <input
            ref={input}
            type="file"
            multiple
            hidden
            tabIndex={-1}
            {...(accept ? { accept } : {})}
            onChange={(e) => {
              take(Array.from(e.currentTarget.files ?? []));
              // The same file chosen again is a new choice.
              e.currentTarget.value = "";
            }}
          />
        </div>
      ) : null}
      {children}
      {over ? (
        <div
          aria-hidden="true"
          className="pointer-events-none absolute inset-0 z-20 flex flex-col items-center justify-center gap-3 rounded-xl border-2 border-dashed border-primary bg-background/90 text-foreground animate-in fade-in-0 duration-(--dur-fast)"
        >
          <span className="flex size-11 items-center justify-center rounded-lg border bg-card text-primary shadow-1">
            <UploadIcon className="size-5" />
          </span>
          <span className="text-md font-medium">{t("dropZone.overlay")}</span>
        </div>
      ) : null}
    </div>
  );
}
