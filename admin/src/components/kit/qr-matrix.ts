// The module matrix behind `QrCode`: `uqr`'s `encode` (its string renderers are not used).

import { encode } from "uqr";

/** Light modules around the code on every side (ISO/IEC 18004 asks for 4). */
export const QR_QUIET_ZONE = 4;

export interface QrMatrix {
  version: number;
  /** Modules a side, without the quiet zone. */
  size: number;
  /** `true` is a dark module; rows top to bottom. */
  data: boolean[][];
}

/** The module matrix of `text`, error correction M (15 %: a screen photo's glare). */
export function qrMatrix(text: string): QrMatrix {
  const { version, size, data } = encode(text, { ecc: "M", border: 0 });
  return { version, size, data };
}
