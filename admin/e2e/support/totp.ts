// RFC 6238 TOTP (SHA-1, 6 digits, 30 s steps), the server's parameters (07 §2), for the
// E2E `totp` account. A step is accepted once (`users.totp_last_step`), so a test waits
// for a step newer than the last one used.

import { createHmac } from "node:crypto";

export const STEP_SECONDS = 30;

/** RFC 4648 base32 (no padding needed) to bytes. */
export function base32Decode(text: string): Buffer {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = 0;
  let value = 0;
  const out: number[] = [];
  for (const ch of text.replace(/=+$/, "").toUpperCase()) {
    const i = alphabet.indexOf(ch);
    if (i < 0) throw new Error(`not base32: ${ch}`);
    value = (value << 5) | i;
    bits += 5;
    if (bits >= 8) {
      out.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  return Buffer.from(out);
}

/** The step of `ms` (milliseconds since the epoch). */
export function stepAt(ms: number = Date.now()): number {
  return Math.floor(ms / 1000 / STEP_SECONDS);
}

/** The 6-digit code of `step` for a base32 secret. */
export function totpCode(secretBase32: string, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const mac = createHmac("sha1", base32Decode(secretBase32)).update(counter).digest();
  const offset = (mac[mac.length - 1] ?? 0) & 0x0f;
  const bin = mac.readUInt32BE(offset) & 0x7fffffff;
  return String(bin % 1_000_000).padStart(6, "0");
}

/** Waits until the current step is newer than `lastStep`; returns the current step. */
export async function nextFreshStep(lastStep: number): Promise<number> {
  for (;;) {
    const now = Date.now();
    const step = stepAt(now);
    if (step > lastStep) return step;
    await new Promise((r) => setTimeout(r, (step + 1) * STEP_SECONDS * 1000 - now + 50));
  }
}
