// Install a plugin (07 §1 H-P1, Review Focus 2) in three steps:
//   1. choose its `ferrite-plugin.toml` and its `.wasm` component (two inputs, or both
//      dropped at once);
//   2. `POST /plugins/inspect` sends both and shows the permission report;
//   3. "Approve and install" sends the same two files with `approve` = the report's
//      fingerprint. If the files changed in between, the server refuses with `422`
//      (field `approve`) and its new report, which replaces the shown one and must be
//      approved again. A new plugin arrives disabled; the last step offers Enable.
// Both requests run over an XHR with a progress bar (the component may be 32 MiB).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { cn } from "cn";
import { CheckIcon, FileCodeIcon, FileCogIcon, PackagePlusIcon } from "lucide-react";
import { type DragEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  type PermissionReportDto,
  type PluginDto,
  enablePlugin,
  pluginsQuery,
} from "@/api/plugins";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { inspectPluginUpload, installPluginUpload } from "@/api/upload";
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
import { PermissionReport } from "./permission-report";

export interface InstallPluginDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** After the install (the plugin as stored: new ones disabled). */
  onInstalled: (plugin: PluginDto) => void;
  /** After Enable on the last step. */
  onEnabled?: (plugin: PluginDto) => void;
}

export function InstallPluginDialog({
  open,
  onOpenChange,
  onInstalled,
  onEnabled,
}: InstallPluginDialogProps) {
  const { t } = useTranslation("admin");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-slot="install-plugin"
        className="flex max-h-[calc(100svh-2rem)] flex-col sm:max-w-2xl"
      >
        <DialogHeader>
          <DialogTitle>{t("plugins.install.title")}</DialogTitle>
          <DialogDescription>{t("plugins.install.description")}</DialogDescription>
        </DialogHeader>
        {/* Mounted only while open: a closed dialog forgets the files and aborts. */}
        {open ? (
          <InstallBody
            onClose={() => onOpenChange(false)}
            onInstalled={onInstalled}
            onEnabled={(p) => {
              onEnabled?.(p);
              onOpenChange(false);
            }}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

type Step =
  | { kind: "pick" }
  | { kind: "report"; report: PermissionReportDto; changed: boolean }
  | { kind: "done"; plugin: PluginDto };

type Failure = { lead: string | null; lines: string[] };

const STEPS = ["pick", "report", "done"] as const;

/** A 422's field problems as "field: detail" lines, else the problem's text. */
function failureOf(error: unknown): Failure {
  if (error instanceof ApiError && error.status === 422) {
    const lines = (error.problem.errors ?? []).map((e) =>
      e.field ? `${e.field}: ${e.detail}` : e.detail,
    );
    if (lines.length > 0) return { lead: error.problem.detail || null, lines };
  }
  return { lead: problemText(error), lines: [] };
}

/** The problem's `report` when the install was refused for a changed upload. */
function changedReport(error: unknown): PermissionReportDto | null {
  if (!(error instanceof ApiError) || error.status !== 422) return null;
  if (!(error.problem.errors ?? []).some((e) => e.field === "approve")) return null;
  const report = error.problem.report as Partial<PermissionReportDto> | null | undefined;
  return report && typeof report.fingerprint === "string" && Array.isArray(report.items)
    ? (report as PermissionReportDto)
    : null;
}

const isAbort = (e: unknown) => e instanceof DOMException && e.name === "AbortError";

function InstallBody({
  onClose,
  onInstalled,
  onEnabled,
}: {
  onClose: () => void;
  onInstalled: (plugin: PluginDto) => void;
  onEnabled: (plugin: PluginDto) => void;
}) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const installed = useQuery(pluginsQuery);
  const [manifest, setManifest] = useState<File | null>(null);
  const [component, setComponent] = useState<File | null>(null);
  const [step, setStep] = useState<Step>({ kind: "pick" });
  const [progress, setProgress] = useState<number | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [enabling, setEnabling] = useState(false);
  const abort = useRef<AbortController | null>(null);
  const busy = progress !== null || enabling;

  // Leaving the dialog ends a request in flight. The server may still finish an
  // install it had received, so the list (and retained data it may take back) is read
  // again.
  useEffect(
    () => () => {
      if (!abort.current) return;
      abort.current.abort();
      void queryClient.invalidateQueries({ queryKey: qk.plugins, exact: true });
      void queryClient.invalidateQueries({ queryKey: qk.pluginsRetained });
    },
    [queryClient],
  );

  const run = async <T,>(call: (signal: AbortSignal) => Promise<T>): Promise<T | undefined> => {
    const controller = new AbortController();
    abort.current = controller;
    setFailure(null);
    setProgress(0);
    try {
      return await call(controller.signal);
    } finally {
      abort.current = null;
      setProgress(null);
    }
  };
  const onProgress = (loaded: number, total: number) => {
    setProgress(total > 0 ? loaded / total : 0);
  };

  const inspect = async () => {
    if (!manifest || !component || busy) return;
    try {
      const report = await run((signal) =>
        inspectPluginUpload({ manifest, component }, { onProgress, signal }),
      );
      if (report) setStep({ kind: "report", report, changed: false });
    } catch (error) {
      if (!isAbort(error)) setFailure(failureOf(error));
    }
  };

  const install = async (report: PermissionReportDto) => {
    if (!manifest || !component || busy) return;
    try {
      const plugin = await run((signal) =>
        installPluginUpload({ manifest, component }, report.fingerprint, { onProgress, signal }),
      );
      if (plugin) {
        setStep({ kind: "done", plugin });
        onInstalled(plugin);
      }
    } catch (error) {
      if (isAbort(error)) return;
      const fresh = changedReport(error);
      if (fresh) setStep({ kind: "report", report: fresh, changed: true });
      else setFailure(failureOf(error));
    }
  };

  const enable = async (plugin: PluginDto) => {
    if (busy) return;
    setEnabling(true);
    setFailure(null);
    try {
      onEnabled(await enablePlugin(plugin.name));
    } catch (error) {
      reportApiError(error);
      setFailure(failureOf(error));
    } finally {
      setEnabling(false);
    }
  };

  const take = (files: readonly File[]) => {
    setFailure(null);
    for (const f of files) {
      const name = f.name.toLowerCase();
      if (name.endsWith(".toml")) setManifest(f);
      else if (name.endsWith(".wasm")) setComponent(f);
    }
  };

  let body: ReactNode;
  let footer: ReactNode;
  if (step.kind === "pick") {
    body = (
      <PickFiles
        manifest={manifest}
        component={component}
        disabled={busy}
        onManifest={(f) => {
          setFailure(null);
          setManifest(f);
        }}
        onComponent={(f) => {
          setFailure(null);
          setComponent(f);
        }}
        onDropFiles={take}
      />
    );
    footer = (
      <>
        <Button variant="outline" onClick={onClose}>
          {t("plugins.install.cancel")}
        </Button>
        <Button
          onClick={() => void inspect()}
          disabled={!manifest || !component || busy}
          aria-busy={busy || undefined}
        >
          {busy ? t("plugins.install.checking") : t("plugins.install.check")}
        </Button>
      </>
    );
  } else if (step.kind === "report") {
    const report = step.report;
    const current = report.update
      ? installed.data?.find((p) => p.name === report.plugin)
      : undefined;
    body = (
      <>
        {step.changed ? (
          <div data-slot="install-changed">
            <Alert tone="warning">{t("plugins.install.changed")}</Alert>
          </div>
        ) : null}
        {report.update ? (
          <p
            data-slot="install-update"
            className="rounded-lg border border-info/40 bg-info/6 px-3 py-2.5 text-sm text-foreground"
          >
            <span>
              {current
                ? t("plugins.install.update", { name: report.plugin, version: current.version })
                : t("plugins.install.updateNoVersion", { name: report.plugin })}
            </span>{" "}
            <span>
              {report.added.length === 0
                ? t("plugins.install.nothingNew")
                : t("plugins.install.somethingNew", { count: report.added.length })}
            </span>
          </p>
        ) : null}
        {report.description ? (
          <p className="text-sm break-words text-muted-foreground">{report.description}</p>
        ) : null}
        <PermissionReport report={report} />
      </>
    );
    footer = (
      <>
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => {
            setFailure(null);
            setStep({ kind: "pick" });
          }}
        >
          {t("plugins.install.back")}
        </Button>
        <Button onClick={() => void install(report)} disabled={busy} aria-busy={busy || undefined}>
          {busy ? t("plugins.install.installing") : t("plugins.install.approve")}
        </Button>
      </>
    );
  } else {
    const p = step.plugin;
    body = (
      <div
        data-slot="install-done"
        className="flex items-start gap-3 rounded-lg border border-success/40 bg-success/6 px-3 py-3"
      >
        <CheckIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-success" />
        <div className="flex min-w-0 flex-col gap-1 text-sm">
          <p role="status" className="font-medium break-words text-foreground">
            {p.enabled
              ? t("plugins.install.doneEnabled", { name: p.name, version: p.version })
              : t("plugins.install.doneDisabled", { name: p.name, version: p.version })}
          </p>
          <p className="text-muted-foreground">
            {p.enabled ? t("plugins.install.doneEnabledHint") : t("plugins.install.doneHint")}
          </p>
          <Link
            to="/plugins/$name"
            params={{ name: p.name }}
            className="w-fit rounded-sm text-foreground underline underline-offset-4 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:outline-none"
          >
            {t("plugins.install.open", { name: p.name })}
          </Link>
        </div>
      </div>
    );
    footer = (
      <>
        <Button variant="outline" onClick={onClose} disabled={enabling}>
          {t("plugins.install.close")}
        </Button>
        {p.enabled ? null : (
          <Button
            onClick={() => void enable(p)}
            disabled={enabling}
            aria-busy={enabling || undefined}
          >
            {enabling ? t("plugins.enabling") : t("plugins.enable")}
          </Button>
        )}
      </>
    );
  }

  const index = STEPS.indexOf(step.kind);
  return (
    <div className="flex min-h-0 flex-col gap-4">
      <ol
        aria-label={t("plugins.install.stepsLabel")}
        className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs"
      >
        {STEPS.map((s, i) => (
          <li
            key={s}
            aria-current={i === index ? "step" : undefined}
            className={cn(
              "flex items-center gap-1.5",
              i === index ? "font-medium text-foreground" : "text-muted-foreground",
            )}
          >
            <span
              aria-hidden="true"
              className={cn(
                "flex size-5 items-center justify-center rounded-full border text-xs tabular-nums",
                i < index && "border-success/60 text-success",
                i === index && "border-primary bg-primary text-primary-foreground",
              )}
            >
              {i < index ? <CheckIcon className="size-3" /> : i + 1}
            </span>
            {t(`plugins.install.step.${s}`)}
            {i < STEPS.length - 1 ? (
              <span aria-hidden="true" className="ml-0.5 h-px w-4 bg-border" />
            ) : null}
          </li>
        ))}
      </ol>
      {/* Focusable, so the keyboard can scroll a long report (axe
          `scrollable-region-focusable`). */}
      <div
        tabIndex={0}
        aria-label={t(`plugins.install.step.${step.kind}`)}
        role="group"
        className="-mx-1 flex min-h-0 flex-col gap-4 overflow-y-auto rounded-md px-1 py-1 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        {body}
        {failure ? (
          failure.lines.length > 0 ? (
            <div
              role="alert"
              data-slot="install-problems"
              className="flex flex-col gap-2 rounded-lg border border-destructive/40 bg-destructive/6 px-3 py-2.5 text-sm"
            >
              {failure.lead ? <p className="font-medium text-foreground">{failure.lead}</p> : null}
              <ul className="flex list-disc flex-col gap-1 pl-5 font-mono text-xs break-words text-foreground">
                {failure.lines.map((l, i) => (
                  <li key={i}>{l}</li>
                ))}
              </ul>
            </div>
          ) : (
            <Alert tone="error">{failure.lead}</Alert>
          )
        ) : null}
        {progress !== null ? <ProgressBar value={progress} /> : null}
      </div>
      <DialogFooter>{footer}</DialogFooter>
    </div>
  );
}

function PickFiles({
  manifest,
  component,
  disabled,
  onManifest,
  onComponent,
  onDropFiles,
}: {
  manifest: File | null;
  component: File | null;
  disabled: boolean;
  onManifest: (f: File) => void;
  onComponent: (f: File) => void;
  onDropFiles: (files: File[]) => void;
}) {
  const { t } = useTranslation("admin");
  const depth = useRef(0);
  const [over, setOver] = useState(false);
  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    depth.current = 0;
    setOver(false);
    if (!disabled) onDropFiles(Array.from(e.dataTransfer.files));
  };
  return (
    <div
      data-slot="plugin-drop"
      data-over={over || undefined}
      className={cn(
        "flex flex-col gap-3 rounded-xl border-2 border-dashed p-4 transition-colors duration-(--dur-fast)",
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
      <div className="flex items-center gap-3">
        <span className="flex size-10 shrink-0 items-center justify-center rounded-lg border bg-card text-muted-foreground shadow-1">
          <PackagePlusIcon aria-hidden="true" className="size-5" />
        </span>
        <div className="flex min-w-0 flex-col gap-0.5">
          <p className="text-md text-foreground">
            {over ? t("plugins.install.overlay") : t("plugins.install.drop")}
          </p>
          <p className="text-sm text-muted-foreground">{t("plugins.install.dropHint")}</p>
        </div>
      </div>
      <FileSlot
        slot="plugin-manifest-input"
        icon={FileCogIcon}
        label={t("plugins.install.manifest")}
        choose={t("plugins.install.chooseManifest")}
        accept=".toml,application/toml"
        file={manifest}
        disabled={disabled}
        onFile={onManifest}
      />
      <FileSlot
        slot="plugin-component-input"
        icon={FileCodeIcon}
        label={t("plugins.install.component")}
        choose={t("plugins.install.chooseComponent")}
        accept=".wasm,application/wasm"
        file={component}
        disabled={disabled}
        onFile={onComponent}
      />
    </div>
  );
}

function FileSlot({
  slot,
  icon: Icon,
  label,
  choose,
  accept,
  file,
  disabled,
  onFile,
}: {
  slot: string;
  icon: typeof FileCogIcon;
  label: string;
  choose: string;
  accept: string;
  file: File | null;
  disabled: boolean;
  onFile: (f: File) => void;
}) {
  const { t } = useTranslation("admin");
  const input = useRef<HTMLInputElement>(null);
  const summary = file ? `${file.name}, ${formatBytes(file.size)}` : t("plugins.install.none");
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-3 rounded-lg border bg-card px-3 py-2.5">
      <Icon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="text-sm font-medium text-foreground">{label}</span>
        <span
          data-slot={`${slot}-name`}
          title={summary}
          className={cn(
            "truncate text-xs tabular-nums",
            file ? "text-foreground" : "text-muted-foreground",
          )}
        >
          {summary}
        </span>
      </div>
      <Button
        variant="outline"
        size="sm"
        className="shrink-0 pointer-coarse:min-h-11"
        disabled={disabled}
        onClick={() => input.current?.click()}
      >
        {file ? t("plugins.install.replaceFile") : choose}
      </Button>
      <input
        ref={input}
        data-slot={slot}
        type="file"
        hidden
        tabIndex={-1}
        accept={accept}
        onChange={(e) => {
          const f = e.currentTarget.files?.[0];
          e.currentTarget.value = "";
          if (f) onFile(f);
        }}
      />
    </div>
  );
}

function ProgressBar({ value }: { value: number }) {
  const { t } = useTranslation("admin");
  const percent = Math.round(value * 100);
  return (
    <div
      role="progressbar"
      aria-label={t("plugins.install.progress")}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      className="h-1 w-full overflow-hidden rounded-full bg-muted"
    >
      <div
        className="h-full origin-left rounded-full bg-primary transition-transform duration-(--dur-base) ease-out motion-reduce:transition-none"
        style={{ transform: `scaleX(${value})` }}
      />
    </div>
  );
}
