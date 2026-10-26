// A media item's thumbnail (11 §7) in a square box: for raster images the smallest
// variant whose short side fills the box at the screen's pixel ratio, else the
// original; SVGs only ever through `<img src>` (never inlined); other kinds a type icon
// with the file's extension. The image's `alt` is the item's alt text; without one the
// image is decorative (`alt=""`) and the title is shown as text beside or under it.

import { cn } from "cn";
import {
  FileArchiveIcon,
  FileAudioIcon,
  FileIcon,
  FileSpreadsheetIcon,
  FileTextIcon,
  FileVideoIcon,
} from "lucide-react";
import type { MediaDto } from "@/api/media";

export type MediaThumbSize = "sm" | "md" | "lg";

/** The box's side in CSS pixels, and its classes. */
const BOX: Record<MediaThumbSize, { px: number; box: string; figure: string }> = {
  sm: { px: 40, box: "size-10", figure: "flex-row items-center" },
  md: { px: 96, box: "size-24", figure: "w-24 flex-col" },
  lg: { px: 160, box: "size-40", figure: "w-40 flex-col" },
};

export interface MediaThumbProps {
  media: MediaDto;
  size: MediaThumbSize;
  className?: string;
}

/** The URL to show in a `px`-wide square box, or `null` for a kind with no picture. */
function thumbUrl(media: MediaDto, px: number): string | null {
  if (!media.mime.startsWith("image/")) return null;
  if (media.mime === "image/svg+xml") return media.url;
  const need = px * (typeof window === "undefined" ? 1 : window.devicePixelRatio || 1);
  const fits = media.variants
    .filter((v) => Math.min(v.width, v.height) >= need)
    .sort((a, b) => a.width * a.height - b.width * b.height);
  return fits[0]?.url ?? media.url;
}

/** A type icon by MIME family. */
function KindIcon({ mime, className }: { mime: string; className: string }) {
  const props = { "aria-hidden": true, className };
  if (mime.startsWith("video/")) return <FileVideoIcon {...props} />;
  if (mime.startsWith("audio/")) return <FileAudioIcon {...props} />;
  if (mime === "application/pdf" || mime.startsWith("text/")) return <FileTextIcon {...props} />;
  if (/zip|tar|gzip|7z|rar/.test(mime)) return <FileArchiveIcon {...props} />;
  if (/spreadsheet|excel|csv/.test(mime)) return <FileSpreadsheetIcon {...props} />;
  return <FileIcon {...props} />;
}

/** `PDF` from `report.pdf`, else the MIME subtype (`application/zip` → `ZIP`). */
function extension(media: MediaDto): string {
  const dot = media.original_name.lastIndexOf(".");
  const ext =
    dot > 0 ? media.original_name.slice(dot + 1) : (media.mime.split("/")[1] ?? "").split("+")[0];
  return (ext ?? "").slice(0, 5).toUpperCase();
}

export function MediaThumb({ media, size, className }: MediaThumbProps) {
  const { px, box, figure } = BOX[size];
  const src = thumbUrl(media, px);
  const alt = media.alt.trim();
  const title = media.title.trim() || media.original_name;
  return (
    <figure data-slot="media-thumb" className={cn("m-0 flex min-w-0 gap-2", figure, className)}>
      <div
        className={cn(
          "relative flex shrink-0 flex-col items-center justify-center gap-1 overflow-hidden rounded-md border bg-muted text-muted-foreground",
          box,
        )}
        {...(src === null && alt ? { role: "img", "aria-label": alt } : {})}
      >
        {src !== null ? (
          <img
            src={src}
            alt={alt}
            width={px}
            height={px}
            loading="lazy"
            decoding="async"
            className="size-full object-cover"
          />
        ) : (
          <>
            <KindIcon mime={media.mime} className={size === "sm" ? "size-4" : "size-6"} />
            {size === "sm" ? null : (
              <span className="font-mono text-xs font-medium tabular-nums">{extension(media)}</span>
            )}
          </>
        )}
      </div>
      {alt ? null : (
        <figcaption className="min-w-0 truncate text-sm" title={title}>
          {title}
        </figcaption>
      )}
    </figure>
  );
}
