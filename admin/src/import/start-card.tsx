// The start card (P32): the WordPress export (required) and a zip of its uploads
// (optional), each a single-file `DropZone`; the options as switches, the category and
// tag bases behind "Advanced"; one primary "Start import". The upload is an XHR with
// progress (`uploadWordpressImport`) under one `Idempotency-Key`. The key is kept for a
// retry after a network error, an unreadable reply, a `5xx`, a cancel, or a `409` that
// says the same key's request is still being received (retried once on its own after
// `Retry-After`, at most 10 s, while Retry runs it at once): the server replays a finished
// start instead of queuing a second import. Changing a file or an option, a `4xx` refusal and a start make a new
// key. Focus moves to the progress while the files go up and back to the card's heading
// when the upload ends; progress is announced at 0, 25, 50, 75 and 100 %.

import { cn } from "cn";
import { ChevronDownIcon, FileArchiveIcon, FileCodeIcon, RotateCwIcon, XIcon } from "lucide-react";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { DEFAULT_IMPORT_OPTIONS, type ImportJobDto, type ImportOptionsDto } from "@/api/import";
import { ApiError } from "@/api/problem";
import { UploadNetworkError, UploadReplyError, uploadWordpressImport } from "@/api/upload";
import { Alert } from "@/auth/auth-layout";
import { DropZone } from "@/components/kit/drop-zone";
import { FormField, SwitchRow } from "@/components/kit/form-field";
import { ProgressBar } from "@/components/kit/progress-bar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { formatBytes } from "@/media/format";
import { MAX_BASE, baseIsValid } from "./import-model";

/** The server's limits (`MAX_WXR`, `MAX_UPLOADS`). */
const MAX_WXR_BYTES = 512 * 1024 * 1024;
const MAX_UPLOADS_BYTES = 4 * 1024 * 1024 * 1024;
/** The longest wait before the automatic retry of an in-progress `409`. */
const MAX_AUTO_RETRY_S = 10;

type Failure =
  | {
      kind:
        | "network"
        | "reply"
        | "server"
        | "busy"
        | "inProgress"
        | "inProgressWait"
        | "forbidden"
        | "unknown";
      detail?: string;
    }
  | { kind: "fields" };
const RETRYABLE: ReadonlySet<Failure["kind"]> = new Set([
  "network",
  "reply",
  "server",
  "inProgress",
  // While the one automatic retry waits: Retry runs it now (`start` clears the timer).
  "inProgressWait",
]);

type BooleanOption = "dry_run" | "keep_permalinks" | "site_options" | "fetch_media";
const SWITCHES = [
  { key: "keep_permalinks", label: "start.keepPermalinks", hint: "start.keepPermalinksHint" },
  { key: "site_options", label: "start.siteOptions", hint: "start.siteOptionsHint" },
  { key: "fetch_media", label: "start.fetchMedia", hint: "start.fetchMediaHint" },
  { key: "dry_run", label: "start.dryRun", hint: "start.dryRunHint" },
] as const satisfies readonly { key: BooleanOption; label: string; hint: string }[];

export interface StartCardProps {
  /** The queued job (`202`). */
  onStarted: (job: ImportJobDto) => void;
  /** `409`: another import is queued or running (the screen refreshes the runs). */
  onBusy?: () => void;
  /** Shown under the busy message: the way to what is running (the screen decides). */
  busyAction?: ReactNode;
  /** Move focus to the card's heading when it mounts ("New import"). */
  focusHeading?: boolean;
}

export function StartCard({ onStarted, onBusy, busyAction, focusHeading = false }: StartCardProps) {
  const { t } = useTranslation("import");
  const [wxr, setWxr] = useState<File | null>(null);
  const [uploads, setUploads] = useState<File | null>(null);
  const [options, setOptions] = useState<ImportOptionsDto>(DEFAULT_IMPORT_OPTIONS);
  const [advanced, setAdvanced] = useState(false);
  const [fileErrors, setFileErrors] = useState<{ wxr?: string; uploads?: string }>({});
  const [baseErrors, setBaseErrors] = useState<{ category_base?: string; tag_base?: string }>({});
  const [failure, setFailure] = useState<Failure | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const key = useRef<string>(crypto.randomUUID());
  const abort = useRef<AbortController | null>(null);
  const retryTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const progressRegion = useRef<HTMLDivElement>(null);
  const advancedId = useId();
  const uploading = progress !== null;

  const clearRetry = () => {
    if (retryTimer.current !== null) clearTimeout(retryTimer.current);
    retryTimer.current = null;
  };
  useEffect(
    () => () => {
      abort.current?.abort();
      if (retryTimer.current !== null) clearTimeout(retryTimer.current);
    },
    [],
  );
  useEffect(() => {
    if (focusHeading) heading.current?.focus();
  }, [focusHeading]);
  // Focus follows the upload: to the progress when it starts, back to the heading when it
  // ends (the controls it came from were disabled or gone meanwhile).
  const wasUploading = useRef(false);
  useEffect(() => {
    if (uploading && !wasUploading.current) progressRegion.current?.focus();
    if (!uploading && wasUploading.current) heading.current?.focus();
    wasUploading.current = uploading;
  }, [uploading]);

  /** Anything the request carries changed: a new request, a new key. */
  const changed = () => {
    clearRetry();
    key.current = crypto.randomUUID();
    setFailure(null);
  };
  const setOption = <K extends keyof ImportOptionsDto>(k: K, v: ImportOptionsDto[K]) => {
    changed();
    setOptions((o) => ({ ...o, [k]: v }));
    if (k === "category_base" || k === "tag_base") {
      setBaseErrors((e) => ({ ...e, [k]: undefined }));
    }
  };

  const chooseWxr = (file: File) => {
    if (uploading) return;
    changed();
    if (file.size > MAX_WXR_BYTES) {
      setWxr(null);
      setFileErrors((e) => ({ ...e, wxr: t("start.wxr.tooLarge", { name: file.name }) }));
      return;
    }
    setFileErrors((e) => ({ ...e, wxr: undefined }));
    setWxr(file);
  };
  const chooseUploads = (file: File) => {
    if (uploading) return;
    changed();
    if (file.size > MAX_UPLOADS_BYTES) {
      setUploads(null);
      setFileErrors((e) => ({
        ...e,
        uploads: t("start.uploads.tooLarge", { name: file.name }),
      }));
      return;
    }
    setFileErrors((e) => ({ ...e, uploads: undefined }));
    setUploads(file);
  };

  /** `auto`: the one automatic retry of an in-progress `409`. */
  const start = async (auto = false) => {
    if (!wxr || uploading) return;
    clearRetry();
    const bad = {
      ...(baseIsValid(options.category_base)
        ? {}
        : { category_base: t("start.badBase", { max: MAX_BASE }) }),
      ...(baseIsValid(options.tag_base) ? {} : { tag_base: t("start.badBase", { max: MAX_BASE }) }),
    };
    if (Object.keys(bad).length > 0) {
      setBaseErrors(bad);
      setAdvanced(true);
      return;
    }
    setFailure(null);
    setProgress(0);
    const ctl = new AbortController();
    abort.current = ctl;
    try {
      const job = await uploadWordpressImport(wxr, uploads, options, {
        key: key.current,
        onProgress: (loaded, total) => setProgress(total > 0 ? loaded / total : 0),
        signal: ctl.signal,
      });
      key.current = crypto.randomUUID();
      setWxr(null);
      setUploads(null);
      setOptions(DEFAULT_IMPORT_OPTIONS);
      onStarted(job);
    } catch (error) {
      if (ctl.signal.aborted) return;
      setFailure(failureOf(error));
      if (error instanceof ApiError) {
        const inProgress = error.status === 409 && error.retryAfter !== null;
        if (inProgress && !auto) {
          // The same key's request is still being received: try again once, on its own.
          setFailure({ kind: "inProgressWait" });
          const wait = Math.min(error.retryAfter ?? 0, MAX_AUTO_RETRY_S) * 1000;
          retryTimer.current = setTimeout(() => {
            retryTimer.current = null;
            void start(true);
          }, wait);
        }
        // A `4xx` refusal: the next attempt is a new request. A `5xx` and an in-progress
        // `409` keep the key (the server may have the start, or be finishing it).
        if (!inProgress && error.status < 500) key.current = crypto.randomUUID();
        if (error.status === 409 && !inProgress) onBusy?.();
        const fields = error.fieldErrors();
        if (error.status === 413 && (fields.wxr || fields.uploads)) {
          setFileErrors({
            ...(fields.wxr ? { wxr: fields.wxr } : {}),
            ...(fields.uploads ? { uploads: fields.uploads } : {}),
          });
          setFailure({ kind: "fields" });
        }
        if (fields.category_base || fields.tag_base) {
          setBaseErrors({
            ...(fields.category_base ? { category_base: fields.category_base } : {}),
            ...(fields.tag_base ? { tag_base: fields.tag_base } : {}),
          });
          setAdvanced(true);
          setFailure({ kind: "fields" });
        }
      }
    } finally {
      if (abort.current === ctl) abort.current = null;
      setProgress(null);
    }
  };

  const retryable = failure !== null && RETRYABLE.has(failure.kind);
  const percent = Math.round((progress ?? 0) * 100);
  // What the live region says: only at the start, each quarter and the end.
  const milestone = progress === null ? null : Math.min(100, Math.floor(progress * 4) * 25);
  const announced =
    milestone === null
      ? ""
      : milestone === 0
        ? t("start.uploadStarted")
        : milestone === 100
          ? t("start.uploadFinished")
          : t("start.uploadMilestone", { percent: milestone });

  return (
    <section
      data-slot="import-start"
      aria-labelledby={`${advancedId}-title`}
      className="flex min-w-0 flex-col rounded-xl border bg-card shadow-1"
    >
      <div className="flex flex-col gap-1 border-b px-5 py-4">
        <h2
          ref={heading}
          id={`${advancedId}-title`}
          tabIndex={-1}
          className="rounded-sm text-md font-medium text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
        >
          {t("start.label")}
        </h2>
      </div>
      <fieldset disabled={uploading} className="flex min-w-0 flex-col gap-6 px-5 py-5">
        <div className="grid min-w-0 gap-4 lg:grid-cols-2">
          <FilePart
            slot="import-wxr"
            icon={FileCodeIcon}
            title={t("start.wxr.title")}
            hint={t("start.wxr.hint")}
            file={wxr}
            error={fileErrors.wxr}
            accept=".xml,application/xml,text/xml"
            labels={{
              choose: t("start.wxr.choose"),
              hint: t("start.wxr.dropHint"),
              overlay: t("start.wxr.overlay"),
            }}
            onFile={chooseWxr}
            onRejected={(f) => {
              changed();
              setFileErrors((e) => ({ ...e, wxr: t("start.wxr.notXml", { name: f.name }) }));
            }}
            onRemove={() => {
              changed();
              setWxr(null);
            }}
          />
          <FilePart
            slot="import-uploads"
            icon={FileArchiveIcon}
            title={t("start.uploads.title")}
            badge={t("start.uploads.optional")}
            hint={t("start.uploads.hint")}
            file={uploads}
            error={fileErrors.uploads}
            accept=".zip,application/zip,application/x-zip-compressed"
            labels={{
              choose: t("start.uploads.choose"),
              hint: t("start.uploads.dropHint"),
              overlay: t("start.uploads.overlay"),
            }}
            onFile={chooseUploads}
            onRejected={(f) => {
              changed();
              setFileErrors((e) => ({
                ...e,
                uploads: t("start.uploads.notZip", { name: f.name }),
              }));
            }}
            onRemove={() => {
              changed();
              setUploads(null);
            }}
          />
        </div>

        <div className="flex min-w-0 flex-col gap-4">
          <h3 className="text-sm font-medium text-foreground">{t("start.options")}</h3>
          <div className="grid min-w-0 gap-x-8 gap-y-5 md:grid-cols-2">
            {SWITCHES.map(({ key: k, label, hint }) => (
              <SwitchRow key={k} label={t(label)} hint={t(hint)}>
                {(control) => (
                  <Switch
                    {...control}
                    className="mt-0.5"
                    checked={options[k]}
                    onCheckedChange={(on) => setOption(k, on)}
                  />
                )}
              </SwitchRow>
            ))}
          </div>
          <div className="flex min-w-0 flex-col gap-4">
            <Button
              type="button"
              variant="ghost"
              aria-expanded={advanced}
              aria-controls={`${advancedId}-advanced`}
              onClick={() => setAdvanced((a) => !a)}
              className="-mx-2.5 self-start text-muted-foreground pointer-coarse:h-11"
            >
              <ChevronDownIcon
                data-icon="inline-start"
                className={cn(
                  "transition-transform duration-(--dur-fast) ease-out",
                  advanced ? "rotate-0" : "-rotate-90",
                )}
              />
              {t("start.advanced")}
            </Button>
            {advanced ? (
              <div
                id={`${advancedId}-advanced`}
                data-slot="import-advanced"
                className="grid min-w-0 gap-4 md:grid-cols-2"
              >
                <FormField
                  label={t("start.categoryBase")}
                  hint={t("start.categoryBaseHint")}
                  error={baseErrors.category_base}
                >
                  {(control) => (
                    <Input
                      {...control}
                      value={options.category_base}
                      spellCheck={false}
                      autoComplete="off"
                      className="font-mono"
                      onChange={(e) => setOption("category_base", e.currentTarget.value)}
                    />
                  )}
                </FormField>
                <FormField
                  label={t("start.tagBase")}
                  hint={t("start.tagBaseHint")}
                  error={baseErrors.tag_base}
                >
                  {(control) => (
                    <Input
                      {...control}
                      value={options.tag_base}
                      spellCheck={false}
                      autoComplete="off"
                      className="font-mono"
                      onChange={(e) => setOption("tag_base", e.currentTarget.value)}
                    />
                  )}
                </FormField>
              </div>
            ) : null}
          </div>
        </div>
      </fieldset>

      <div className="flex min-w-0 flex-col gap-3 border-t px-5 py-4">
        {/* Always in the page (empty while nothing uploads): text in a live region that
            appears with it is often not read, so "Upload started" must be a change. */}
        <span data-slot="import-progress-live" className="sr-only" aria-live="polite">
          {announced}
        </span>
        {failure && failure.kind !== "fields" ? (
          <Alert tone={failure.kind === "inProgressWait" ? "warning" : "error"}>
            {t(`start.errors.${failure.kind}`)}
            {failure.detail ? (
              <span className="block text-muted-foreground">{failure.detail}</span>
            ) : null}
          </Alert>
        ) : null}
        {failure?.kind === "busy" && busyAction ? (
          <div data-slot="import-busy-action" className="flex flex-wrap gap-2">
            {busyAction}
          </div>
        ) : null}
        {uploading ? (
          <div
            ref={progressRegion}
            tabIndex={-1}
            role="group"
            aria-label={t("start.uploadLabel")}
            data-slot="import-progress"
            className="flex min-w-0 flex-col gap-2 rounded-sm focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
          >
            <div className="flex items-center justify-between gap-3 text-sm">
              <span className="text-foreground tabular-nums">
                {t("start.uploading", { percent })}
              </span>
              <Button
                type="button"
                variant="ghost"
                className="pointer-coarse:h-11"
                onClick={() => abort.current?.abort()}
              >
                {t("start.cancel")}
              </Button>
            </div>
            <ProgressBar value={progress ?? 0} label={t("start.uploadLabel")} className="h-1.5" />
          </div>
        ) : (
          <div className="flex flex-wrap items-center justify-end gap-3">
            {!wxr ? (
              <span className="mr-auto text-sm text-muted-foreground">
                {t("start.needsExport")}
              </span>
            ) : null}
            {retryable ? (
              <Button type="button" className="pointer-coarse:h-11" onClick={() => void start()}>
                <RotateCwIcon data-icon="inline-start" />
                {t("start.retry")}
              </Button>
            ) : (
              <Button
                type="button"
                className="pointer-coarse:h-11"
                disabled={!wxr}
                onClick={() => void start()}
              >
                {options.dry_run ? t("start.submitDry") : t("start.submit")}
              </Button>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

function failureOf(error: unknown): Failure {
  if (error instanceof UploadNetworkError) return { kind: "network" };
  if (error instanceof UploadReplyError) return { kind: "reply" };
  if (error instanceof ApiError) {
    if (error.status === 409) return { kind: error.retryAfter !== null ? "inProgress" : "busy" };
    if (error.status === 403) return { kind: "forbidden" };
    const detail = error.problem.detail || error.problem.title;
    if (error.status >= 500) return { kind: "server", ...(detail ? { detail } : {}) };
    return { kind: "unknown", ...(detail ? { detail } : {}) };
  }
  return { kind: "unknown" };
}

function FilePart({
  slot,
  icon: Icon,
  title,
  badge,
  hint,
  file,
  error,
  accept,
  labels,
  onFile,
  onRejected,
  onRemove,
}: {
  slot: string;
  icon: typeof FileCodeIcon;
  title: string;
  badge?: string;
  hint: string;
  file: File | null;
  error: string | undefined;
  accept: string;
  labels: { choose: string; hint: string; overlay: string };
  onFile: (file: File) => void;
  onRejected: (file: File) => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation("import");
  return (
    <div data-slot={slot} className="flex min-w-0 flex-col gap-2">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-medium text-foreground">{title}</h3>
        {badge ? (
          <span className="rounded-full border px-2 text-xs text-muted-foreground">{badge}</span>
        ) : null}
      </div>
      <DropZone
        single
        accept={accept}
        labels={labels}
        chooser={!file}
        onFiles={(files) => {
          const f = files[0];
          if (f) onFile(f);
        }}
        onRejected={(files) => {
          const f = files[0];
          if (f) onRejected(f);
        }}
        className={cn(
          "min-h-28 min-w-0 justify-center rounded-lg border border-dashed p-4",
          error ? "border-destructive/60" : "border-border",
          file && "border-solid bg-muted/30",
        )}
      >
        {file ? (
          <FileChip
            icon={Icon}
            file={file}
            onRemove={onRemove}
            removeLabel={t("start.remove", { name: file.name })}
          />
        ) : null}
        <p className="text-xs text-muted-foreground">{hint}</p>
      </DropZone>
      {error ? (
        <p role="alert" className="text-sm break-words text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function FileChip({
  icon: Icon,
  file,
  onRemove,
  removeLabel,
}: {
  icon: typeof FileCodeIcon;
  file: File;
  onRemove: () => void;
  removeLabel: string;
}) {
  return (
    <div data-slot="import-file" className="flex min-w-0 items-center gap-3">
      <span className="flex size-9 shrink-0 items-center justify-center rounded-md border bg-card text-muted-foreground">
        <Icon aria-hidden="true" className="size-4" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-sm font-medium text-foreground" title={file.name}>
          {file.name}
        </span>
        <span className="text-xs text-muted-foreground tabular-nums">{formatBytes(file.size)}</span>
      </div>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        aria-label={removeLabel}
        title={removeLabel}
        className="shrink-0 pointer-coarse:size-11"
        onClick={onRemove}
      >
        <XIcon />
      </Button>
    </div>
  );
}
