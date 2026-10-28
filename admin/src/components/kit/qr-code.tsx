// A QR code drawn in React (the TOTP enrolment's `otpauth://` URL; 07 §2). `uqr`'s
// `encode` gives the module matrix only; the SVG is built here from `<rect>`s, so no
// markup string from the library reaches the DOM and the text is never in an attribute.
// Black on white in both themes (`--qr-fg` / `--qr-bg`), with the standard quiet zone of
// 4 modules, so phone cameras read it in dark mode too.

import { useMemo } from "react";
import { cn } from "cn";
import { QR_QUIET_ZONE, qrMatrix } from "./qr-matrix";

export interface QrCodeProps {
  text: string;
  /** The image's accessible name (what to do with it, never the text). */
  label: string;
  /** Drawn width and height in CSS pixels. */
  size?: number;
  className?: string;
}

export function QrCode({ text, label, size = 200, className }: QrCodeProps) {
  const matrix = useMemo(() => qrMatrix(text), [text]);
  const side = matrix.size + 2 * QR_QUIET_ZONE;
  const modules = matrix.data.flatMap((row, y) =>
    row.flatMap((dark, x) =>
      dark ? [{ key: y * matrix.size + x, x: x + QR_QUIET_ZONE, y: y + QR_QUIET_ZONE }] : [],
    ),
  );
  return (
    <svg
      role="img"
      aria-label={label}
      data-slot="qr-code"
      viewBox={`0 0 ${side} ${side}`}
      width={size}
      height={size}
      shapeRendering="crispEdges"
      className={cn("shrink-0 rounded-md fill-qr-fg ring-1 ring-border", className)}
    >
      <rect width={side} height={side} className="fill-qr-bg" />
      {modules.map((m) => (
        <rect key={m.key} data-module="" x={m.x} y={m.y} width={1} height={1} />
      ))}
    </svg>
  );
}
