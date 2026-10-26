// Import redirects from a CSV file (07 §1 `POST /redirects/import`): choose or drop one
// `.csv`, see what will be sent (`previewCsv`: the row count, whether the header row is
// skipped, the first rows), then Import posts it as `text/csv`. The server checks every
// row first and writes none if any fails: its `422` lists each problem by line (at most
// 100), shown here as text. Files over 1 MiB are refused before reading (the server's
// `413` says the same); more than 10 000 rows are refused before sending.

import { FileSpreadsheetIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import { ApiError } from "@/api/problem";
import { importRedirects } from "@/api/redirects";
import type { SchemaImportReportDto } from "@/api/schema";
import { DropZone } from "@/components/kit/drop-zone";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { uiLocale } from "@/lib/locale";
import { problemText } from "../taxonomies/terms-model";
import { type CsvPreview, previewCsv } from "./csv-preview";

/** The server's body limit (`MAX_CSV_BODY`). */
const MAX_BYTES = 1024 * 1024;
/** The server's row limit (`MAX_IMPORT_ROWS`). */
const MAX_ROWS = 10_000;

interface Chosen {
  name: string;
  text: string;
  preview: CsvPreview;
}

/** The server's refusal of the rows: its summary and one entry per bad line. */
interface Refusal {
  detail: string;
  lines: { line: string; detail: string }[];
}

export function ImportDialog({
  open,
  onOpenChange,
  onImported,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onImported: (report: SchemaImportReportDto) => void;
}) {
  const { t } = useTranslation("structure");
  const [chosen, setChosen] = useState<Chosen | null>(null);
  const [refusal, setRefusal] = useState<Refusal | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const reset = () => {
    setChosen(null);
    setRefusal(null);
    setFailure(null);
  };

  const choose = async (file: File) => {
    reset();
    if (file.size > MAX_BYTES) {
      setFailure(t("redirects.import.tooLarge", { name: file.name }));
      return;
    }
    try {
      const text = await file.text();
      setChosen({ name: file.name, text, preview: previewCsv(text) });
    } catch {
      setFailure(t("redirects.import.unreadable", { name: file.name }));
    }
  };

  const run = async () => {
    if (!chosen || busy) return;
    setBusy(true);
    setRefusal(null);
    setFailure(null);
    try {
      const report = await importRedirects(chosen.text);
      reset();
      onImported(report);
    } catch (err) {
      reportApiError(err);
      if (err instanceof ApiError && err.status === 422 && (err.problem.errors ?? []).length > 0) {
        setRefusal({
          detail: err.problem.detail,
          lines: (err.problem.errors ?? []).map((e) => ({
            line: e.field ?? "",
            detail: e.detail,
          })),
        });
      } else {
        setFailure(problemText(err));
      }
    } finally {
      setBusy(false);
    }
  };

  const rows = chosen?.preview.rows ?? 0;
  const tooMany = rows > MAX_ROWS;

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) reset();
        onOpenChange(o);
      }}
    >
      <DialogContent data-slot="import-dialog" className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("redirects.import.title")}</DialogTitle>
          <DialogDescription>{t("redirects.import.description")}</DialogDescription>
        </DialogHeader>
        <DropZone
          accept=".csv,text/csv"
          single
          labels={{
            choose: t("redirects.import.choose"),
            hint: t("redirects.import.hint"),
            overlay: t("redirects.import.overlay"),
          }}
          onFiles={(files) => {
            const file = files[0];
            if (file) void choose(file);
          }}
          onRejected={(files) => {
            reset();
            setFailure(t("redirects.import.notCsv", { name: files[0]?.name ?? "" }));
          }}
          className="min-w-0"
        >
          {chosen ? <Preview chosen={chosen} tooMany={tooMany} /> : null}
          {refusal ? <Refusals refusal={refusal} /> : null}
          {failure ? (
            <p role="alert" className="text-sm break-words text-destructive">
              {failure}
            </p>
          ) : null}
        </DropZone>
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              reset();
              onOpenChange(false);
            }}
          >
            {t("redirects.import.cancel")}
          </Button>
          <Button
            type="button"
            disabled={busy || !chosen || rows === 0 || tooMany}
            onClick={() => void run()}
          >
            {busy
              ? t("redirects.import.importing")
              : chosen && rows > 0
                ? t("redirects.import.submit", { count: rows })
                : t("redirects.import.submitNone")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Preview({ chosen, tooMany }: { chosen: Chosen; tooMany: boolean }) {
  const { t } = useTranslation("structure");
  const { rows, header, sample } = chosen.preview;
  const number = new Intl.NumberFormat(uiLocale());
  const cell = "max-w-0 truncate px-3 py-1.5 text-left font-mono text-sm";
  return (
    <section
      data-slot="import-preview"
      aria-label={t("redirects.import.previewLabel")}
      className="flex min-w-0 flex-col gap-3 rounded-lg border bg-card p-3 shadow-1"
    >
      <div className="flex min-w-0 items-center gap-3">
        <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
          <FileSpreadsheetIcon aria-hidden="true" className="size-4" />
        </span>
        <div className="flex min-w-0 flex-col">
          <span className="truncate font-medium" title={chosen.name}>
            {chosen.name}
          </span>
          <span className="text-sm text-muted-foreground tabular-nums">
            {t("redirects.import.rows", { count: rows, formatted: number.format(rows) })}
            {header ? ` · ${t("redirects.import.headerSkipped")}` : ""}
          </span>
        </div>
      </div>
      {tooMany ? (
        <p role="alert" className="text-sm text-destructive">
          {t("redirects.import.tooMany", { max: number.format(MAX_ROWS) })}
        </p>
      ) : null}
      {rows === 0 ? (
        <p className="text-sm text-muted-foreground">{t("redirects.import.noRows")}</p>
      ) : (
        <div className="overflow-hidden rounded-md border">
          <table className="w-full table-fixed border-collapse">
            <caption className="sr-only">{t("redirects.import.sampleCaption")}</caption>
            <thead className="bg-muted/50 text-sm text-muted-foreground">
              <tr>
                <th scope="col" className="px-3 py-1.5 text-left font-medium">
                  {t("redirects.columns.source")}
                </th>
                <th scope="col" className="px-3 py-1.5 text-left font-medium">
                  {t("redirects.columns.target")}
                </th>
                <th scope="col" className="w-16 px-3 py-1.5 text-left font-medium">
                  {t("redirects.columns.code")}
                </th>
              </tr>
            </thead>
            <tbody>
              {sample.map((r, i) => (
                <tr key={i} className="border-t">
                  {[0, 1, 2].map((j) => (
                    <td key={j} className={cell} title={r[j] ?? ""}>
                      {r[j] ?? ""}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
          {rows > sample.length ? (
            <p className="border-t px-3 py-1.5 text-sm text-muted-foreground tabular-nums">
              {t("redirects.import.more", {
                count: rows - sample.length,
                formatted: number.format(rows - sample.length),
              })}
            </p>
          ) : null}
        </div>
      )}
    </section>
  );
}

/** The server's per-line problems, as text (Review Focus 3). */
function Refusals({ refusal }: { refusal: Refusal }) {
  const { t } = useTranslation("structure");
  const capped = refusal.lines.length >= 100;
  return (
    <section
      role="alert"
      data-slot="import-errors"
      aria-labelledby="import-errors-title"
      className="flex min-w-0 flex-col gap-2 rounded-lg border border-destructive/40 bg-destructive/5 p-3"
    >
      <h3 id="import-errors-title" className="font-medium text-foreground">
        {t("redirects.import.errorsTitle")}
      </h3>
      {capped ? <p className="text-sm text-muted-foreground">{refusal.detail}</p> : null}
      <ul className="flex max-h-60 flex-col gap-1.5 overflow-y-auto text-sm">
        {refusal.lines.map((l, i) => {
          const n = /^line (\d+)$/.exec(l.line)?.[1];
          return (
            <li key={i} className="flex min-w-0 gap-3">
              <span className="w-16 shrink-0 font-medium text-foreground tabular-nums">
                {n !== undefined ? t("redirects.import.line", { n }) : l.line}
              </span>
              <span className="min-w-0 break-words text-muted-foreground">{l.detail}</span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
