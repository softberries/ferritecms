import { expect, test } from "vitest";
import { pickFolder, readDrop } from "./pick-folder";
import { PackError } from "./ustar";

/** A file as `<input webkitdirectory>` gives it (`webkitRelativePath` set). */
function picked(path: string, body = "x"): File {
  const file = new File([body], path.slice(path.lastIndexOf("/") + 1));
  Object.defineProperty(file, "webkitRelativePath", { value: path });
  return file;
}

const decode = (b: Uint8Array) => new TextDecoder().decode(b);

test("input_selection_strips_the_top_folder", async () => {
  const folder = await pickFolder([
    picked("blog/theme.toml", "toml"),
    picked("blog/templates/index.html", "html"),
    picked("blog/.git/config"),
    picked("blog/assets/.DS_Store"),
  ]);
  expect(folder.root).toBe("blog");
  expect(folder.files.map((f) => f.path)).toEqual(["theme.toml", "templates/index.html"]);
  expect(decode(folder.files[0]?.bytes ?? new Uint8Array())).toBe("toml");
});

test("input_selection_checks_limits_before_reading", async () => {
  const many = Array.from({ length: 2001 }, (_, i) => picked(`t/assets/${i}.css`));
  await expect(pickFolder(many)).rejects.toMatchObject({ problem: "too-many-files" });
  const big = new File([new Uint8Array(1)], "big.bin");
  Object.defineProperty(big, "webkitRelativePath", { value: "t/assets/big.bin" });
  Object.defineProperty(big, "size", { value: 21 * 1024 * 1024 });
  await expect(pickFolder([big])).rejects.toBeInstanceOf(PackError);
});

// --- drops ----------------------------------------------------------------------------

interface FakeEntry {
  name: string;
  isFile: boolean;
  isDirectory: boolean;
  file?: (ok: (f: File) => void, fail: (e: unknown) => void) => void;
  createReader?: () => { readEntries: (ok: (e: FakeEntry[]) => void) => void };
}

function fileEntry(name: string, body = "x"): FakeEntry {
  return {
    name,
    isFile: true,
    isDirectory: false,
    file: (ok) => ok(new File([body], name)),
  };
}

/** A directory whose reader hands out its entries in batches of two, as Chrome does. */
function dirEntry(name: string, children: FakeEntry[]): FakeEntry {
  return {
    name,
    isFile: false,
    isDirectory: true,
    createReader: () => {
      let at = 0;
      return {
        readEntries: (ok) => {
          const batch = children.slice(at, at + 2);
          at += batch.length;
          ok(batch);
        },
      };
    },
  };
}

function drop(entries: (FakeEntry | File)[]): DataTransfer {
  const items = entries.map((e) => ({
    kind: "file",
    webkitGetAsEntry: () => (e instanceof File ? fileEntry(e.name) : e),
    getAsFile: () => (e instanceof File ? e : null),
  }));
  return { items, files: entries.filter((e) => e instanceof File) } as unknown as DataTransfer;
}

test("dropped_folder_is_walked", async () => {
  const tree = dirEntry("blog-min", [
    fileEntry("theme.toml", "t"),
    dirEntry("templates", [fileEntry("index.html"), fileEntry("post.html"), fileEntry("x.html")]),
    fileEntry(".hidden-note"),
    dirEntry(".git", [fileEntry("HEAD")]),
  ]);
  const got = await readDrop(drop([tree]));
  expect(got.kind).toBe("folder");
  if (got.kind !== "folder") return;
  expect(got.folder.root).toBe("blog-min");
  expect(got.folder.files.map((f) => f.path).sort()).toEqual([
    "templates/index.html",
    "templates/post.html",
    "templates/x.html",
    "theme.toml",
  ]);
});

test("dropped_tar_and_other_drops", async () => {
  const tar = new File(["t"], "theme.tar");
  expect(await readDrop(drop([tar]))).toEqual({ kind: "tar", file: tar });
  expect((await readDrop(drop([new File(["x"], "a.css")]))).kind).toBe("other");
  expect((await readDrop(drop([dirEntry("a", []), dirEntry("b", [])]))).kind).toBe("other");
});

test("dropped_folder_too_deep_stops", async () => {
  let deep: FakeEntry = fileEntry("x.css");
  for (let i = 20; i >= 0; i--) deep = dirEntry(`d${i}`, [deep]);
  await expect(readDrop(drop([deep]))).rejects.toMatchObject({ problem: "too-deep" });
});

test("dropped_folder_counts_entries_like_the_cli", async () => {
  // `package::read_dir` counts every listed entry (folders and hidden names too) against
  // the 2000 limit: 1998 files + a folder + a hidden file + the folder's file = 2001.
  const files = Array.from({ length: 1998 }, (_, i) => fileEntry(`f${i}.css`));
  const tree = dirEntry("t", [
    ...files,
    dirEntry("sub", [fileEntry("x.css")]),
    fileEntry(".hidden"),
  ]);
  await expect(readDrop(drop([tree]))).rejects.toMatchObject({ problem: "too-many-files" });
  // One fewer listed entry packs.
  const ok = dirEntry("t", [
    ...files.slice(1),
    dirEntry("sub", [fileEntry("x.css")]),
    fileEntry(".hidden"),
  ]);
  const got = await readDrop(drop([ok]));
  expect(got.kind).toBe("folder");
});
