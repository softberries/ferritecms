// Fixture files for the media specs, built in memory: a real PNG of any size (a two-colour
// diagonal gradient, so the variants job can decode it and screenshots show something),
// an SVG, a PDF header, and an executable-like binary the server refuses (`415`).

import { crc32, deflateSync } from "node:zlib";

export interface FixtureFile {
  name: string;
  mimeType: string;
  buffer: Buffer;
}

type Rgb = readonly [number, number, number];

function chunk(type: string, data: Buffer): Buffer {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body) >>> 0);
  return Buffer.concat([length, body, crc]);
}

/** A `width`×`height` RGB PNG fading diagonally from `from` to `to`. */
export function png(
  name: string,
  width: number,
  height: number,
  from: Rgb = [232, 93, 45],
  to: Rgb = [38, 70, 120],
): FixtureFile {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // bit depth
  header[9] = 2; // colour type: RGB
  const rowBytes = width * 3 + 1;
  const raw = Buffer.alloc(rowBytes * height);
  for (let y = 0; y < height; y++) {
    raw[y * rowBytes] = 0; // filter: none
    for (let x = 0; x < width; x++) {
      const t = (x / width + y / height) / 2;
      const at = y * rowBytes + 1 + x * 3;
      raw[at] = Math.round(from[0] + (to[0] - from[0]) * t);
      raw[at + 1] = Math.round(from[1] + (to[1] - from[1]) * t);
      raw[at + 2] = Math.round(from[2] + (to[2] - from[2]) * t);
    }
  }
  const buffer = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
  return { name, mimeType: "image/png", buffer };
}

/** An SVG with a script and an event handler (the sanitiser removes both). */
export function svg(name: string): FixtureFile {
  const text =
    '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80" viewBox="0 0 120 80">' +
    '<script>alert("svg")</script>' +
    '<rect width="120" height="80" fill="#2a6f97" onload="alert(1)"/>' +
    '<circle cx="60" cy="40" r="24" fill="#f4a259"/></svg>';
  return { name, mimeType: "image/svg+xml", buffer: Buffer.from(text, "utf8") };
}

/** A small PDF (its header is what the server's sniffing reads). */
export function pdf(name: string): FixtureFile {
  const text =
    "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n" +
    "2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n";
  return { name, mimeType: "application/pdf", buffer: Buffer.from(text, "latin1") };
}

/** A Windows executable's header (`MZ` … `PE`): refused as an unsupported type. */
export function exe(name: string): FixtureFile {
  const buffer = Buffer.alloc(1024);
  buffer.write("MZ", 0, "latin1");
  buffer.writeUInt32LE(0x80, 0x3c);
  buffer.write("PE\0\0", 0x80, "latin1");
  return { name, mimeType: "application/octet-stream", buffer };
}
