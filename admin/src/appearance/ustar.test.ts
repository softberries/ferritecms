// The browser's theme packer against the Rust one (P42): `blog-min.tar` is what
// `ferrite theme pack` wrote for `__fixtures__/blog-min/`, and `packTheme` must give the
// same bytes for the same folder.

import { expect, test } from "vitest";
import { PackError, isHidden, packTheme, readTarFile, themeNameOf, type PackFile } from "./ustar";

interface NodeFs {
  readFileSync(path: string): Uint8Array;
  readdirSync(
    path: string,
    options: { withFileTypes: true },
  ): { name: string; isDirectory(): boolean }[];
}

// A specifier TypeScript does not resolve, so the app's types stay free of Node's; the
// jsdom project still runs under Node.
const NODE_FS = "node:fs";
const fs = (await import(/* @vite-ignore */ NODE_FS)) as NodeFs;

// Relative to the admin folder, where Vitest runs (`import.meta.url` is not a file URL
// under jsdom).
const FIXTURES = "src/appearance/__fixtures__";
const read = (path: string) => new Uint8Array(fs.readFileSync(path));

/** Every file under `dir`, hidden ones included, with paths relative to it. */
function readFolder(dir: string, prefix = ""): PackFile[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry): PackFile[] => {
    const path = `${prefix}${entry.name}`;
    if (entry.isDirectory()) return readFolder(`${dir}/${entry.name}`, `${path}/`);
    return [{ path, bytes: read(`${dir}/${entry.name}`) }];
  });
}

const text = (s: string) => new TextEncoder().encode(s);
const ascii = (b: Uint8Array) => new TextDecoder().decode(b).replace(/\0+$/, "");

function packError(run: () => unknown): PackError {
  try {
    run();
  } catch (e) {
    if (e instanceof PackError) return e;
    throw e;
  }
  throw new Error("no PackError");
}

test("pack_matches_the_rust_fixture", () => {
  const files = readFolder(`${FIXTURES}/blog-min`);
  // The folder holds the hidden file the packer must leave out.
  expect(files.some((f) => f.path === ".hidden-note")).toBe(true);
  const expected = read(`${FIXTURES}/blog-min.tar`);
  const packed = packTheme(files);
  expect(packed.length).toBe(5120);
  expect(packed).toEqual(expected);
  // The input order does not matter.
  expect(packTheme([...files].reverse())).toEqual(packed);
});

test("pack_refuses_limits", () => {
  const one = (path: string, size = 1): PackFile => ({ path, bytes: new Uint8Array(size) });

  const many = Array.from({ length: 2001 }, (_, i) => one(`assets/f${i}.txt`));
  expect(packError(() => packTheme(many)).problem).toBe("too-many-files");
  // Exactly the limit packs.
  expect(() => packTheme(many.slice(0, 2000))).not.toThrow();

  const big = [one("assets/a.bin", 10 * 1024 * 1024), one("assets/b.bin", 10 * 1024 * 1024 + 1)];
  expect(packError(() => packTheme(big)).problem).toBe("too-large");

  const deep = `${Array.from({ length: 17 }, (_, i) => `d${i}`).join("/")}/x.css`;
  const tooDeep = packError(() => packTheme([one(deep)]));
  expect(tooDeep.problem).toBe("too-deep");
  expect(tooDeep.path).toBe(deep);
  // 16 levels are fine.
  const ok = `${Array.from({ length: 16 }, (_, i) => `d${i}`).join("/")}/x.css`;
  expect(() => packTheme([one(ok)])).not.toThrow();

  const long = `assets/${"a".repeat(120)}/${"b".repeat(120)}/abc.css`; // 256 bytes
  expect(text(long).length).toBe(256);
  const tooLong = packError(() => packTheme([one(long)]));
  expect(tooLong.problem).toBe("path-too-long");
  expect(tooLong.path).toBe(long);

  // A last segment over 100 bytes fits no ustar name field.
  const name = `assets/${"n".repeat(101)}`;
  expect(packError(() => packTheme([one(name)])).problem).toBe("does-not-fit");
  // A first folder over 155 bytes leaves no prefix to split at.
  const prefix = `${"p".repeat(160)}/x.css`;
  expect(packError(() => packTheme([one(prefix)])).problem).toBe("does-not-fit");

  // (`.` and `..` segments start with a dot: hidden, so skipped before these rules.)
  for (const bad of ["/abs.css", "a//b.css", "a\\b.css", "a/\u0007.css", "a/\u0085.css"]) {
    const e = packError(() => packTheme([one(bad)]));
    expect(e.problem, bad).toBe("bad-path");
    expect(e.path).toBe(bad);
  }
  expect(packError(() => packTheme([one("a.css"), one("a.css")])).problem).toBe("duplicate");
});

test("long_path_splits", () => {
  // 50 + 1 + 60 + 1 + 9 = 121 bytes: too long for the name field alone.
  const dirA = "a".repeat(50);
  const dirB = "b".repeat(60);
  const path = `${dirA}/${dirB}/style.css`;
  const tar = packTheme([{ path, bytes: text("x") }]);
  const header = tar.subarray(0, 512);
  // The longest parent that fits the 155-byte prefix, the rest in the name.
  expect(ascii(header.subarray(345, 500))).toBe(`${dirA}/${dirB}`);
  expect(ascii(header.subarray(0, 100))).toBe("style.css");
  expect([...(readTarFile(tar, path) ?? [])]).toEqual([...text("x")]);

  // A name of exactly 100 bytes fills the field with no NUL.
  const hundred = "c".repeat(100);
  const exact = packTheme([{ path: `assets/${hundred}`, bytes: new Uint8Array() }]);
  expect(new TextDecoder().decode(exact.subarray(0, 100))).toBe(hundred);
  expect(ascii(exact.subarray(345, 500))).toBe("assets");

  // A multi-byte path is measured in bytes: 34 × 3-byte characters = 102 bytes.
  const wide = `assets/${"€".repeat(34)}`;
  expect(packError(() => packTheme([{ path: wide, bytes: new Uint8Array() }])).problem).toBe(
    "does-not-fit",
  );
});

test("hidden_files_skipped", () => {
  expect(isHidden(".git/config")).toBe(true);
  expect(isHidden("assets/.DS_Store")).toBe(true);
  expect(isHidden("assets/a.css")).toBe(false);
  const files: PackFile[] = [
    { path: "theme.toml", bytes: text('[theme]\nname = "x"\nversion = "1.0.0"\n') },
    { path: ".hidden", bytes: text("no") },
    { path: "assets/.cache/a.css", bytes: text("no") },
    { path: "assets/.DS_Store", bytes: text("no") },
  ];
  const tar = packTheme(files);
  // One header, one data block and the two end blocks.
  expect(tar.length).toBe(4 * 512);
  expect([...(readTarFile(tar, "theme.toml") ?? [])]).toEqual([...(files[0]?.bytes ?? [])]);
  expect(readTarFile(tar, ".hidden")).toBeNull();
  // Hidden files do not count against the limits.
  const hidden = Array.from({ length: 2500 }, (_, i) => ({
    path: `.git/objects/${i}`,
    bytes: new Uint8Array(),
  }));
  expect(() => packTheme([...files, ...hidden])).not.toThrow();
});

test("sorted_by_bytes_not_utf16", () => {
  // U+FF5E sorts after U+1F600 in UTF-16 code units but before it in UTF-8 bytes (as the
  // Rust packer's BTreeMap of strings orders them).
  const tar = packTheme([
    { path: "assets/😀.css", bytes: new Uint8Array() },
    { path: "assets/～.css", bytes: new Uint8Array() },
  ]);
  expect(ascii(tar.subarray(0, 100))).toBe("assets/～.css");
  expect(ascii(tar.subarray(512, 612))).toBe("assets/😀.css");
});

test("theme_name_is_read_from_theme_toml", () => {
  expect(themeNameOf('[theme]\nname        = "blog-min"\nversion = "0.1.0"\n')).toBe("blog-min");
  expect(themeNameOf("# c\n[theme] # t\nname = 'lit' # n\n")).toBe("lit");
  expect(themeNameOf('[menus]\nname = "no"\n[theme]\nversion = "1"\n')).toBeNull();
  expect(themeNameOf("not toml")).toBeNull();
  const toml = readTarFile(read(`${FIXTURES}/blog-min.tar`), "theme.toml");
  expect(toml && themeNameOf(new TextDecoder().decode(toml))).toBe("blog-min");
  expect(readTarFile(new Uint8Array(100), "theme.toml")).toBeNull();
});
