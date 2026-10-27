// A theme folder picked in the browser (P42): from `<input type="file" webkitdirectory>`
// (each file's `webkitRelativePath`) or dropped (`DataTransferItem.webkitGetAsEntry()`,
// walked with the File and Directory Entries API). The folder's own name is stripped, so
// `blog/theme.toml` becomes `theme.toml`. Hidden entries (any segment starting with `.`)
// are skipped before anything is read, and the server's limits are checked from the file
// sizes before the bytes are read, so a huge folder fails fast instead of filling memory.

import { PACK_LIMITS, PackError, isHidden, type PackFile } from "./ustar";

export interface PickedFolder {
  /** The picked folder's own name (`blog`), `null` if it could not be told. */
  root: string | null;
  files: PackFile[];
}

/** What a drop held: one folder, one `.tar` file, or anything else. */
export type Drop =
  { kind: "folder"; folder: PickedFolder } | { kind: "tar"; file: File } | { kind: "other" };

interface Picked {
  path: string;
  file: File;
}

/** Reads the picked files once the limits allow it. */
async function read(root: string | null, picked: Picked[]): Promise<PickedFolder> {
  const files = picked.filter((p) => !isHidden(p.path));
  if (files.length > PACK_LIMITS.files) throw new PackError("too-many-files");
  const total = files.reduce((n, p) => n + p.file.size, 0);
  if (total > PACK_LIMITS.totalBytes) throw new PackError("too-large");
  return {
    root,
    files: await Promise.all(
      files.map(async (p) => ({ path: p.path, bytes: new Uint8Array(await p.file.arrayBuffer()) })),
    ),
  };
}

/** A `<input webkitdirectory>` selection, its top folder stripped from every path. */
export function pickFolder(selection: readonly File[]): Promise<PickedFolder> {
  let root: string | null = null;
  const picked = selection.flatMap((file): Picked[] => {
    const full = file.webkitRelativePath || file.name;
    const cut = full.indexOf("/");
    if (cut < 0) return [{ path: full, file }];
    root ??= full.slice(0, cut);
    return [{ path: full.slice(cut + 1), file }];
  });
  return read(root, picked);
}

// The File and Directory Entries API, as far as it is used here.
interface Entry {
  name: string;
  isFile: boolean;
  isDirectory: boolean;
}
interface FileEntry extends Entry {
  file(ok: (file: File) => void, fail: (error: unknown) => void): void;
}
interface DirectoryEntry extends Entry {
  createReader(): {
    readEntries(ok: (entries: Entry[]) => void, fail: (error: unknown) => void): void;
  };
}

/** All of a directory's entries (`readEntries` hands them out in batches until empty). */
async function children(dir: DirectoryEntry): Promise<Entry[]> {
  const reader = dir.createReader();
  const all: Entry[] = [];
  for (;;) {
    const batch = await new Promise<Entry[]>((ok, fail) => {
      reader.readEntries(ok, fail);
    });
    if (batch.length === 0) return all;
    all.push(...batch);
  }
}

/**
 * The files under `dir` with paths relative to it, hidden entries not walked; stops at the
 * depth limit and at one file past the count limit (the count is checked again in `read`).
 */
async function walk(dir: DirectoryEntry, prefix: string, depth: number, out: Picked[]) {
  for (const entry of await children(dir)) {
    if (entry.name.startsWith(".")) continue;
    const path = `${prefix}${entry.name}`;
    if (entry.isDirectory) {
      if (depth + 1 > PACK_LIMITS.depth) throw new PackError("too-deep", path);
      await walk(entry as DirectoryEntry, `${path}/`, depth + 1, out);
    } else if (entry.isFile) {
      const file = await new Promise<File>((ok, fail) => {
        (entry as FileEntry).file(ok, fail);
      });
      out.push({ path, file });
      if (out.length > PACK_LIMITS.files) throw new PackError("too-many-files");
    }
  }
}

/** What `dt` (a `drop` event's) holds: one folder, read; one `.tar` file; or other. */
export async function readDrop(dt: DataTransfer): Promise<Drop> {
  const items = Array.from(dt.items).filter((i) => i.kind === "file");
  if (items.length !== 1) return { kind: "other" };
  const item = items[0];
  const entry = item?.webkitGetAsEntry() as Entry | null | undefined;
  if (entry?.isDirectory) {
    const picked: Picked[] = [];
    await walk(entry as DirectoryEntry, "", 0, picked);
    return { kind: "folder", folder: await read(entry.name, picked) };
  }
  const file = item?.getAsFile();
  if (file && file.name.toLowerCase().endsWith(".tar")) return { kind: "tar", file };
  return { kind: "other" };
}
