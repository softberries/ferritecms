// How the library prints a media item's facts: its size in B/kB/MB/GB (`Intl.NumberFormat`
// units, 1 kB = 1 000 bytes), its type as a short label (`PNG`, `PDF`), its dimensions.

import type { MediaDto } from "@/api/media";
import { uiLocale } from "@/lib/locale";

const UNITS = ["byte", "kilobyte", "megabyte", "gigabyte"] as const;

/** `222 bytes`, `123.5 kB`: the largest unit with a value of at least 1, one decimal at most. */
export function formatBytes(bytes: number, locale: string = uiLocale()): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1000 && unit < UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return new Intl.NumberFormat(locale, {
    style: "unit",
    unit: UNITS[unit],
    // "222 bytes" (the short form says "222 byte"), then "22.4 kB".
    unitDisplay: unit === 0 ? "long" : "short",
    maximumFractionDigits: unit === 0 ? 0 : 1,
  }).format(value);
}

const MIME_LABELS: Record<string, string> = {
  "image/jpeg": "JPEG",
  "image/png": "PNG",
  "image/gif": "GIF",
  "image/webp": "WebP",
  "image/svg+xml": "SVG",
  "application/pdf": "PDF",
  "audio/mpeg": "MP3",
  "audio/ogg": "Ogg",
  "audio/wav": "WAV",
  "audio/x-wav": "WAV",
  "video/mp4": "MP4",
  "video/webm": "WebM",
  "text/plain": "TXT",
  "text/csv": "CSV",
};

/** `PNG`, `PDF`; else the original name's extension; else the MIME type. */
export function typeLabel(media: Pick<MediaDto, "mime" | "original_name">): string {
  const known = MIME_LABELS[media.mime];
  if (known) return known;
  const dot = media.original_name.lastIndexOf(".");
  if (dot > 0 && dot < media.original_name.length - 1) {
    return media.original_name.slice(dot + 1, dot + 6).toUpperCase();
  }
  return media.mime;
}

/** `1200 × 800`, or `null` while unknown (set once the variants are made). */
export function dimensions(
  media: Pick<MediaDto, "width" | "height">,
  locale: string = uiLocale(),
): string | null {
  if (media.width == null || media.height == null) return null;
  const n = new Intl.NumberFormat(locale, { useGrouping: false });
  return `${n.format(media.width)} × ${n.format(media.height)}`;
}

/** Formats the variants pipeline decodes (03 §3): only these get variants. */
export function isRaster(mime: string): boolean {
  return ["image/jpeg", "image/png", "image/gif", "image/webp"].includes(mime);
}

/** A title to show: the item's, else its original file name. */
export function displayTitle(media: Pick<MediaDto, "title" | "original_name">): string {
  return media.title.trim() || media.original_name;
}
