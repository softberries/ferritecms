// Upload a theme (P42): pick or drop a folder, which is packed in the browser into the
// canonical bundle (`ustar.ts`), or pick a `.tar` bundle, which is sent as it is. Before
// the upload the dialog names the theme (read from `theme.toml`; for a `.tar` from the
// archive's `theme.toml` entry) and, when a theme of that name is installed, says the
// upload replaces it and, for the active theme, that the new files go live at once (H-T2).
// `POST /themes` runs over an XHR with progress; the server's `422` problems (one per
// template error) are listed as text.

import { cn } from "cn";
import { FileArchiveIcon, FolderOpenIcon, FolderUpIcon } from "lucide-react";
import { type DragEvent, type InputHTMLAttributes, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError } from "@/api/problem";
import type { ThemeDto } from "@/api/themes";
import { uploadTheme } from "@/api/upload";
import { Alert } from "@/auth/auth-layout";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { formatBytes } from "@/media/format";
import { problemText } from "@/structure/taxonomies/terms-model";
import { type PickedFolder, pickFolder, readDrop } from "./pick-folder";
import { PackError, type PackFile, packTheme, readTarFile, themeNameOf } from "./ustar";
import { ProgressBar } from "@/components/kit/progress-bar";

/** The largest `.tar` read for its `theme.toml` (the server's bundle limit). */
const MAX_TAR_READ = 23_020_544;

type Picked =
  | { kind: "folder"; folder: PickedFolder; name: string | null; bytes: number }
  | { kind: "tar"; file: File; name: string | null };

type Failure =
  | { kind: "pack"; error: PackError }
  | { kind: "load"; problems: string[] }
  | { kind: "other"; text: string };

// `webkitdirectory` is not in React's input attributes; React passes it through.
const DIRECTORY = { webkitdirectory: "" } as InputHTMLAttributes<HTMLInputElement>;

const decoder = new TextDecoder();

function nameOfFolder(files: readonly PackFile[]): string | null {
  const toml = files.find((f) => f.path === "theme.toml");
  return toml ? themeNameOf(decoder.decode(toml.bytes)) : null;
}

async function nameOfTar(file: File): Promise<string | null> {
  if (file.size > MAX_TAR_READ) return null;
  const toml = readTarFile(new Uint8Array(await file.arrayBuffer()), "theme.toml");
  return toml ? themeNameOf(decoder.decode(toml)) : null;
}

export interface UploadThemeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  themes: readonly ThemeDto[];
  onInstalled: (theme: ThemeDto) => void;
}

export function UploadThemeDialog({
  open,
  onOpenChange,
  themes,
  onInstalled,
}: UploadThemeDialogProps) {
  const { t } = useTranslation("admin");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-slot="upload-theme"
        className="flex max-h-[calc(100svh-2rem)] flex-col sm:max-w-lg"
      >
        <DialogHeader>
          <DialogTitle>{t("themes.uploadDialog.title")}</DialogTitle>
          <DialogDescription>{t("themes.uploadDialog.description")}</DialogDescription>
        </DialogHeader>
        {/* Mounted only while open: a closed dialog forgets the pick and aborts. */}
        {open ? (
          <UploadBody
            themes={themes}
            onCancel={() => onOpenChange(false)}
            onInstalled={onInstalled}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function UploadBody({
  themes,
  onCancel,
  onInstalled,
}: {
  themes: readonly ThemeDto[];
  onCancel: () => void;
  onInstalled: (theme: ThemeDto) => void;
}) {
  const { t } = useTranslation("admin");
  const folderInput = useRef<HTMLInputElement>(null);
  const tarInput = useRef<HTMLInputElement>(null);
  const abort = useRef<AbortController | null>(null);
  const depth = useRef(0);
  const [over, setOver] = useState(false);
  const [picked, setPicked] = useState<Picked | null>(null);
  const [reading, setReading] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const uploading = progress !== null;

  // Leaving the dialog ends an upload in flight.
  useEffect(() => () => abort.current?.abort(), []);

  const take = async (read: () => Promise<Picked | null>) => {
    setFailure(null);
    setReading(true);
    try {
      setPicked(await read());
    } catch (error) {
      setPicked(null);
      setFailure(
        error instanceof PackError
          ? { kind: "pack", error }
          : { kind: "other", text: problemText(error) },
      );
    } finally {
      setReading(false);
    }
  };

  const takeFolder = (files: File[]) =>
    take(async () => {
      const folder = await pickFolder(files);
      const bytes = folder.files.reduce((n, f) => n + f.bytes.length, 0);
      return { kind: "folder", folder, name: nameOfFolder(folder.files), bytes };
    });
  const takeTar = (file: File) =>
    take(async () => ({ kind: "tar", file, name: await nameOfTar(file) }));

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    depth.current = 0;
    setOver(false);
    if (uploading) return;
    // The entries must be taken during the event; `readDrop` does that before it awaits.
    const drop = readDrop(e.dataTransfer);
    void take(async () => {
      const got = await drop;
      if (got.kind === "folder") {
        const bytes = got.folder.files.reduce((n, f) => n + f.bytes.length, 0);
        return { kind: "folder", folder: got.folder, name: nameOfFolder(got.folder.files), bytes };
      }
      if (got.kind === "tar")
        return { kind: "tar", file: got.file, name: await nameOfTar(got.file) };
      throw new Error(t("themes.uploadDialog.dropOther"));
    });
  };

  const submit = async () => {
    if (!picked || uploading) return;
    setFailure(null);
    let body: Uint8Array | Blob;
    try {
      body = picked.kind === "folder" ? packTheme(picked.folder.files) : picked.file;
    } catch (error) {
      setFailure(
        error instanceof PackError
          ? { kind: "pack", error }
          : { kind: "other", text: problemText(error) },
      );
      return;
    }
    const controller = new AbortController();
    abort.current = controller;
    setProgress(0);
    try {
      const theme = await uploadTheme(body, {
        signal: controller.signal,
        onProgress: (loaded, total) => {
          setProgress(total > 0 ? loaded / total : 0);
        },
      });
      onInstalled(theme);
    } catch (error) {
      if (error instanceof DOMException && error.name === "AbortError") return;
      if (error instanceof ApiError && error.status === 422) {
        const problems = (error.problem.errors ?? []).map((e) => e.detail);
        setFailure({ kind: "load", problems: problems.length > 0 ? problems : [error.message] });
      } else {
        setFailure({ kind: "other", text: problemText(error) });
      }
    } finally {
      abort.current = null;
      setProgress(null);
    }
  };

  const installed = picked?.name ? themes.find((th) => th.name === picked.name) : undefined;

  return (
    <div className="flex min-h-0 flex-col gap-4">
      <div className="-mx-1 flex min-h-0 flex-col gap-4 overflow-y-auto px-1 py-1">
        {picked ? (
          <PickedSummary picked={picked} disabled={uploading} onAgain={() => setPicked(null)} />
        ) : (
          <div
            data-slot="theme-drop"
            data-over={over || undefined}
            className={cn(
              "flex flex-col items-center gap-4 rounded-xl border-2 border-dashed px-4 py-8 text-center transition-colors duration-(--dur-fast)",
              over ? "border-primary bg-primary/5" : "border-border",
            )}
            onDragEnter={(e) => {
              e.preventDefault();
              depth.current += 1;
              setOver(true);
            }}
            onDragOver={(e) => {
              e.preventDefault();
              e.dataTransfer.dropEffect = "copy";
            }}
            onDragLeave={() => {
              depth.current = Math.max(0, depth.current - 1);
              if (depth.current === 0) setOver(false);
            }}
            onDrop={onDrop}
          >
            <span className="flex size-11 items-center justify-center rounded-lg border bg-card text-muted-foreground shadow-1">
              <FolderUpIcon aria-hidden="true" className="size-5" />
            </span>
            <div className="flex flex-col gap-1">
              <p className="text-md text-foreground">
                {over ? t("themes.uploadDialog.overlay") : t("themes.uploadDialog.drop")}
              </p>
              <p className="text-sm text-muted-foreground">{t("themes.uploadDialog.dropHint")}</p>
            </div>
            <div className="flex flex-wrap justify-center gap-2">
              <Button
                variant="outline"
                className="pointer-coarse:min-h-11"
                disabled={reading}
                onClick={() => folderInput.current?.click()}
              >
                <FolderOpenIcon data-icon="inline-start" />
                {t("themes.uploadDialog.chooseFolder")}
              </Button>
              <Button
                variant="outline"
                className="pointer-coarse:min-h-11"
                disabled={reading}
                onClick={() => tarInput.current?.click()}
              >
                <FileArchiveIcon data-icon="inline-start" />
                {t("themes.uploadDialog.chooseTar")}
              </Button>
            </div>
            {reading ? (
              <p role="status" className="text-sm text-muted-foreground">
                {t("themes.uploadDialog.reading")}
              </p>
            ) : null}
            <input
              ref={folderInput}
              data-slot="theme-folder-input"
              type="file"
              multiple
              hidden
              tabIndex={-1}
              {...DIRECTORY}
              onChange={(e) => {
                const files = Array.from(e.currentTarget.files ?? []);
                e.currentTarget.value = "";
                if (files.length > 0) void takeFolder(files);
              }}
            />
            <input
              ref={tarInput}
              data-slot="theme-tar-input"
              type="file"
              hidden
              tabIndex={-1}
              accept=".tar,application/x-tar"
              onChange={(e) => {
                const file = e.currentTarget.files?.[0];
                e.currentTarget.value = "";
                if (file) void takeTar(file);
              }}
            />
          </div>
        )}
        {installed && picked ? (
          <div data-slot="theme-replace">
            <Alert tone="warning">
              {installed.active
                ? t("themes.uploadDialog.replaceActive", {
                    name: installed.name,
                    version: installed.version,
                  })
                : t("themes.uploadDialog.replace", {
                    name: installed.name,
                    version: installed.version,
                  })}
            </Alert>
          </div>
        ) : null}
        {failure ? <FailureText failure={failure} /> : null}
        {progress !== null ? (
          <ProgressBar value={progress} label={t("themes.uploadDialog.progress")} />
        ) : null}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={onCancel}>
          {t("themes.uploadDialog.cancel")}
        </Button>
        <Button
          onClick={() => void submit()}
          disabled={!picked || uploading || reading}
          aria-busy={uploading || undefined}
        >
          {uploading
            ? t("themes.uploadDialog.uploading")
            : installed
              ? t("themes.uploadDialog.submitReplace")
              : t("themes.uploadDialog.submit")}
        </Button>
      </DialogFooter>
    </div>
  );
}

function PickedSummary({
  picked,
  disabled,
  onAgain,
}: {
  picked: Picked;
  disabled: boolean;
  onAgain: () => void;
}) {
  const { t } = useTranslation("admin");
  const Icon = picked.kind === "folder" ? FolderOpenIcon : FileArchiveIcon;
  const summary =
    picked.kind === "folder"
      ? t("themes.uploadDialog.folderSummary", {
          count: picked.folder.files.length,
          size: formatBytes(picked.bytes),
        })
      : t("themes.uploadDialog.tarSummary", {
          file: picked.file.name,
          size: formatBytes(picked.file.size),
        });
  return (
    <div
      data-slot="theme-picked"
      className="flex items-center gap-3 rounded-xl border bg-muted/40 p-3"
    >
      <span className="flex size-10 shrink-0 items-center justify-center rounded-lg border bg-card text-muted-foreground">
        <Icon aria-hidden="true" className="size-5" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p
          data-slot="theme-picked-name"
          title={picked.name ?? undefined}
          className={cn(
            "truncate font-medium",
            picked.name ? "text-foreground" : "text-muted-foreground",
          )}
        >
          {picked.name ?? t("themes.uploadDialog.unknownName")}
        </p>
        <p className="truncate text-xs text-muted-foreground tabular-nums" title={summary}>
          {summary}
        </p>
      </div>
      <Button
        variant="ghost"
        className="shrink-0 pointer-coarse:min-h-11"
        disabled={disabled}
        onClick={onAgain}
      >
        {t("themes.uploadDialog.chooseAgain")}
      </Button>
    </div>
  );
}

function FailureText({ failure }: { failure: Failure }) {
  const { t } = useTranslation("admin");
  if (failure.kind === "pack") {
    return (
      <Alert tone="error">
        {t(`themes.uploadDialog.pack.${failure.error.problem}`, { path: failure.error.path ?? "" })}
      </Alert>
    );
  }
  if (failure.kind === "other") return <Alert tone="error">{failure.text}</Alert>;
  return <LoadProblems lead={t("themes.uploadDialog.doesNotLoad")} problems={failure.problems} />;
}

/** Why a theme does not load: the server's problems (`templates/<file>:<line>: …`) as text. */
export function LoadProblems({ lead, problems }: { lead: string; problems: readonly string[] }) {
  return (
    <div
      role="alert"
      data-slot="theme-problems"
      className="flex flex-col gap-2 rounded-lg border border-destructive/40 bg-destructive/6 px-3 py-2.5 text-sm"
    >
      <p className="font-medium text-foreground">{lead}</p>
      <ul className="flex list-disc flex-col gap-1 pl-5 font-mono text-xs break-words text-foreground">
        {problems.map((p, i) => (
          <li key={i}>{p}</li>
        ))}
      </ul>
    </div>
  );
}
