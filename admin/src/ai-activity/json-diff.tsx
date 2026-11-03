// A generic diff of two JSON images (design 17 §11): one row per changed leaf by dotted
// path, the removed value struck through and the added one underlined, with the same
// marks and legend as the revisions drawer (never colour alone).

import { cn } from "cn";
import { useTranslation } from "react-i18next";
import { type JsonChange, jsonDiff, valueText } from "./model";

const ADDED =
  "rounded-[3px] bg-success/14 px-px text-foreground underline decoration-success decoration-2 underline-offset-[3px] box-decoration-clone";
const REMOVED =
  "rounded-[3px] bg-destructive/10 px-px text-foreground line-through decoration-destructive decoration-[1.5px] box-decoration-clone";

export function JsonDiffTable({
  changes,
  className,
}: {
  changes: JsonChange[];
  className?: string;
}) {
  const { t } = useTranslation("ai");
  const { t: te } = useTranslation("editor");
  if (changes.length === 0) {
    return <p className="text-sm text-muted-foreground italic">{t("activity.noDiff")}</p>;
  }
  return (
    <table data-slot="json-diff" className={cn("w-full table-fixed text-sm", className)}>
      <thead className="sr-only">
        <tr>
          <th scope="col">{t("activity.diff.path")}</th>
          <th scope="col">{t("activity.diff.before")}</th>
          <th scope="col">{t("activity.diff.after")}</th>
        </tr>
      </thead>
      <tbody className="divide-y">
        {changes.map((c) => (
          <tr key={c.path} className="align-top">
            <th
              scope="row"
              className="w-1/4 py-2 pr-3 text-left font-mono text-xs font-normal break-all text-muted-foreground"
            >
              {c.path}
            </th>
            <td className="py-2 pr-3 break-words">
              {c.before === undefined ? (
                <span className="text-muted-foreground">{t("activity.diff.absent")}</span>
              ) : (
                <del className={REMOVED}>
                  <span className="sr-only">{te("revisions.diff.removedSr")} </span>
                  {valueText(c.before)}
                </del>
              )}
            </td>
            <td className="py-2 break-words">
              {c.after === undefined ? (
                <span className="text-muted-foreground">{t("activity.diff.absent")}</span>
              ) : (
                <ins className={ADDED}>
                  <span className="sr-only">{te("revisions.diff.addedSr")} </span>
                  {valueText(c.after)}
                </ins>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** The diff of two whole images. */
export function JsonDiff({ before, after }: { before: unknown; after: unknown }) {
  return <JsonDiffTable changes={jsonDiff(before ?? undefined, after ?? undefined)} />;
}
