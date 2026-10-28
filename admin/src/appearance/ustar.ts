// The theme bundle packer (P42): a TypeScript port of `ferrite_theme::bundle::pack`
// (`crates/ferrite-theme/src/bundle.rs`, the `tar` crate's ustar writer), so a folder
// picked in the browser becomes the exact bytes `POST /themes` takes. Both packers are
// pinned byte for byte to `__fixtures__/blog-min.tar` (`ustar.test.ts`,
// `crates/ferrite/tests/theme_pack.rs`).
//
// The format: regular-file entries only, sorted by the paths' UTF-8 bytes; mode
// `0000644`, mtime, uid and gid 0, empty user and group names, magic `ustar\0` version
// `00`; a path over 100 bytes split at a `/` into prefix (≤ 155 bytes, the longest
// parent that fits) and name (≤ 100); data padded to 512 bytes; two zero blocks at the
// end. Hidden entries (any segment starting with `.`) are left out, and the server's
// folder limits apply (2000 files, 20 MiB, depth 16, paths ≤ 255 bytes).

/** One file of a theme folder, its path relative to the folder (`templates/index.html`). */
export interface PackFile {
  path: string;
  bytes: Uint8Array;
}

/** The server's folder limits (`ferrite_theme::package`, `bundle::MAX_BUNDLE_PATH`). */
export const PACK_LIMITS = {
  files: 2000,
  totalBytes: 20 * 1024 * 1024,
  depth: 16,
  pathBytes: 255,
} as const;

export type PackProblem =
  | "too-many-files"
  | "too-large"
  | "path-too-long"
  | "bad-path"
  | "too-deep"
  | "does-not-fit"
  | "duplicate";

/** Why a folder cannot be packed; `path` names the file, when one is to blame. */
export class PackError extends Error {
  constructor(
    readonly problem: PackProblem,
    readonly path: string | null = null,
  ) {
    super(path === null ? problem : `${path}: ${problem}`);
    this.name = "PackError";
  }
}

const BLOCK = 512;
const NAME_LEN = 100;
const PREFIX_LEN = 155;
const encoder = new TextEncoder();

/** Whether any segment of `path` starts with `.` (`package::is_hidden`). */
export function isHidden(path: string): boolean {
  return path.split("/").some((s) => s.startsWith("."));
}

/**
 * `package::valid_path`: relative, `/`-separated, no empty, `.` or `..` segments, no
 * backslash or control character (Unicode `Cc`, as Rust's `char::is_control`).
 */
function validPath(path: string): boolean {
  return (
    path !== "" &&
    !path.includes("\\") &&
    // eslint-disable-next-line no-control-regex
    !/[\u0000-\u001f\u007f-\u009f]/.test(path) &&
    path.split("/").every((s) => s !== "" && s !== "." && s !== "..")
  );
}

/** `bundle::path_problem`, in its order: length, shape, depth. */
function pathProblem(path: string, bytes: Uint8Array): PackProblem | null {
  if (bytes.length > PACK_LIMITS.pathBytes) return "path-too-long";
  if (!validPath(path)) return "bad-path";
  if (path.split("/").length - 1 > PACK_LIMITS.depth) return "too-deep";
  return null;
}

function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    const d = (a[i] ?? 0) - (b[i] ?? 0);
    if (d !== 0) return d;
  }
  return a.length - b.length;
}

/**
 * The ustar name and prefix of a path (`tar` `UstarHeader::set_path`): the whole path
 * when it fits the name field, else the longest parent that fits the prefix field and
 * the rest as the name. `null` when no split fits.
 */
function splitPath(bytes: Uint8Array): { name: Uint8Array; prefix: Uint8Array } | null {
  if (bytes.length <= NAME_LEN) return { name: bytes, prefix: new Uint8Array() };
  for (let i = bytes.length - 1; i > 0; i--) {
    if (bytes[i] !== 0x2f || i > PREFIX_LEN) continue;
    const name = bytes.subarray(i + 1);
    return name.length <= NAME_LEN ? { name, prefix: bytes.subarray(0, i) } : null;
  }
  return null;
}

/** `tar`'s `octal_into`: zero-padded octal digits, then a NUL in the field's last byte. */
function octal(header: Uint8Array, offset: number, length: number, value: number): void {
  const digits = value.toString(8).padStart(length - 1, "0");
  for (let i = 0; i < length - 1; i++) header[offset + i] = digits.charCodeAt(i);
  header[offset + length - 1] = 0;
}

function header(name: Uint8Array, prefix: Uint8Array, size: number): Uint8Array {
  const h = new Uint8Array(BLOCK);
  h.set(name, 0);
  octal(h, 100, 8, 0o644); // mode
  octal(h, 108, 8, 0); // uid
  octal(h, 116, 8, 0); // gid
  octal(h, 124, 12, size);
  octal(h, 136, 12, 0); // mtime
  h[156] = 0x30; // typeflag `0`: a regular file
  h.set(encoder.encode("ustar\0"), 257);
  h.set(encoder.encode("00"), 263);
  // uname, gname, devmajor and devminor stay zero.
  h.set(prefix, 345);
  // The checksum: every byte summed with the checksum field read as eight spaces.
  h.fill(0x20, 148, 156);
  const sum = h.reduce((a, b) => a + b, 0);
  octal(h, 148, 8, sum);
  return h;
}

/**
 * Packs `files` into the canonical theme bundle (the module comment), or throws a
 * `PackError` for the first problem: the count and the total size first, then each path
 * in order.
 */
export function packTheme(files: readonly PackFile[]): Uint8Array {
  const entries = files
    .filter((f) => !isHidden(f.path))
    .map((f) => ({ ...f, key: encoder.encode(f.path) }))
    .sort((a, b) => compareBytes(a.key, b.key));
  if (entries.length > PACK_LIMITS.files) throw new PackError("too-many-files");
  const total = entries.reduce((n, f) => n + f.bytes.length, 0);
  if (total > PACK_LIMITS.totalBytes) throw new PackError("too-large");

  // As `pack`: every path's rules first, then the headers (where a split can fail).
  for (const [i, f] of entries.entries()) {
    const problem = pathProblem(f.path, f.key);
    if (problem) throw new PackError(problem, f.path);
    const previous = entries[i - 1];
    if (previous && compareBytes(previous.key, f.key) === 0) {
      throw new PackError("duplicate", f.path);
    }
  }
  const headers: Uint8Array[] = [];
  let size = 2 * BLOCK;
  for (const f of entries) {
    const split = splitPath(f.key);
    if (!split) throw new PackError("does-not-fit", f.path);
    headers.push(header(split.name, split.prefix, f.bytes.length));
    size += BLOCK + Math.ceil(f.bytes.length / BLOCK) * BLOCK;
  }

  const out = new Uint8Array(size);
  let at = 0;
  for (const [i, f] of entries.entries()) {
    out.set(headers[i] ?? new Uint8Array(BLOCK), at);
    at += BLOCK;
    out.set(f.bytes, at);
    at += Math.ceil(f.bytes.length / BLOCK) * BLOCK;
  }
  // The rest of `out` is already the zero padding and the two end blocks.
  return out;
}

function field(tar: Uint8Array, at: number, length: number): string {
  const bytes = tar.subarray(at, at + length);
  const end = bytes.indexOf(0);
  return new TextDecoder().decode(end < 0 ? bytes : bytes.subarray(0, end));
}

/**
 * The bytes of the entry `path` in a ustar archive, or `null` (not there, or not a
 * readable archive). Used only to read a picked `.tar`'s `theme.toml` for the replace
 * warning; the server parses the upload with its own rules.
 */
export function readTarFile(tar: Uint8Array, path: string): Uint8Array | null {
  let at = 0;
  while (at + BLOCK <= tar.length) {
    const h = tar.subarray(at, at + BLOCK);
    if (h.every((b) => b === 0)) return null;
    const sizeText = field(tar, at + 124, 12).trim();
    if (!/^[0-7]+$/.test(sizeText)) return null;
    const size = parseInt(sizeText, 8);
    const prefix = field(tar, at + 345, PREFIX_LEN);
    const name = field(tar, at + 0, NAME_LEN);
    const full = prefix ? `${prefix}/${name}` : name;
    const start = at + BLOCK;
    if (start + size > tar.length) return null;
    if (full === path) return tar.slice(start, start + size);
    at = start + Math.ceil(size / BLOCK) * BLOCK;
  }
  return null;
}

/**
 * The `name` of `theme.toml`'s `[theme]` table, as a plain `name = "…"` (or `'…'`) line,
 * else `null`. Theme names are 1–64 of `[a-z0-9-]`, so no escapes; anything fancier is
 * left to the server, and the screen then cannot say which theme an upload replaces.
 */
export function themeNameOf(toml: string): string | null {
  let inTheme = false;
  for (const raw of toml.split(/\r?\n/)) {
    const line = raw.trim();
    if (line.startsWith("[")) {
      inTheme = /^\[\s*theme\s*\]\s*(#.*)?$/.test(line);
      continue;
    }
    if (!inTheme) continue;
    const m = /^name\s*=\s*(?:"([^"\\]*)"|'([^']*)')\s*(#.*)?$/.exec(line);
    if (m) return m[1] ?? m[2] ?? null;
  }
  return null;
}
