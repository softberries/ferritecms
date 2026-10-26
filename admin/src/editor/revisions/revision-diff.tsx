// A word diff as text with marks (Task 17): added text underlined in `success`, removed
// text struck through in `destructive`, each on a light tint. Neither mark is colour
// only (an underline, a strike-through), and each says what it is to a screen reader
// through a visually hidden "added:"/"removed:" in front of its text (ruling: no
// `aria-label`, which ARIA 1.2 prohibits on the insertion/deletion roles). Paragraph breaks are kept
// as paragraphs, and a change that is only a paragraph break shows as ¶.

import { cn } from "cn";
import { useTranslation } from "react-i18next";
import type { DiffPart } from "../diff";

export interface RevisionDiffProps {
  parts: DiffPart[];
  className?: string;
}

const ADDED_MARK =
  "rounded-[3px] bg-success/14 px-px text-foreground underline decoration-success decoration-2 underline-offset-[3px] box-decoration-clone";
const REMOVED_MARK =
  "rounded-[3px] bg-destructive/10 px-px text-foreground line-through decoration-destructive decoration-[1.5px] box-decoration-clone";

interface Segment {
  kind: DiffPart["kind"];
  text: string;
  /** A paragraph break that was added or removed on its own (shown as ¶). */
  pilcrow?: boolean;
}

/**
 * The parts as paragraphs (one per line of the plain text), each a run of segments. A
 * change that is only a paragraph break gets a ¶ segment at the end of its paragraph,
 * so splitting or joining paragraphs is never an invisible change.
 */
function paragraphsOf(parts: readonly DiffPart[]): Segment[][] {
  const out: Segment[][] = [[]];
  for (const p of parts) {
    const lines = p.text.split("\n");
    const breakOnly = p.kind !== "same" && p.text.trim() === "" && lines.length > 1;
    lines.forEach((line, i) => {
      if (i > 0) {
        if (breakOnly) out.at(-1)?.push({ kind: p.kind, text: "", pilcrow: true });
        out.push([]);
      }
      if (line !== "") out.at(-1)?.push({ kind: p.kind, text: line });
    });
  }
  return out.filter((para) => para.length > 0);
}

export function RevisionDiff({ parts, className }: RevisionDiffProps) {
  const { t } = useTranslation("editor");
  const paragraphs = paragraphsOf(parts);
  if (paragraphs.length === 0) {
    return (
      <p data-slot="revision-diff" className={cn("text-muted-foreground italic", className)}>
        {t("revisions.diff.empty")}
      </p>
    );
  }
  const mark = (seg: Segment, key: number, afterOther: boolean) => {
    const content = seg.pilcrow ? (
      <>
        <span className="sr-only">{t("revisions.diff.lineBreak")}</span>
        <span aria-hidden="true" className="px-0.5 text-muted-foreground">
          ¶
        </span>
      </>
    ) : (
      seg.text
    );
    // A removal right next to an addition ("~~guide~~notes") gets a hairline gap.
    const gap = afterOther ? "ms-0.5" : undefined;
    return seg.kind === "added" ? (
      <ins key={key} data-diff="added" className={cn(ADDED_MARK, gap)}>
        <span className="sr-only">{t("revisions.diff.addedSr")} </span>
        {content}
      </ins>
    ) : (
      <del key={key} data-diff="removed" className={cn(REMOVED_MARK, gap)}>
        <span className="sr-only">{t("revisions.diff.removedSr")} </span>
        {content}
      </del>
    );
  };
  return (
    <div
      data-slot="revision-diff"
      className={cn("flex flex-col gap-[0.6em] break-words whitespace-pre-wrap", className)}
    >
      {paragraphs.map((para, i) => (
        <p key={i}>
          {para.map((seg, j) => {
            const before = para[j - 1]?.kind;
            return seg.kind === "same" ? (
              <span key={j}>{seg.text}</span>
            ) : (
              mark(seg, j, before !== undefined && before !== "same" && before !== seg.kind)
            );
          })}
        </p>
      ))}
    </div>
  );
}

/** The two marks with their names (shown, so the marks need no colour to be read). */
export function DiffLegend({ className }: { className?: string }) {
  const { t } = useTranslation("editor");
  return (
    <p
      aria-hidden="true"
      data-slot="diff-legend"
      className={cn("flex items-center gap-3 text-xs text-muted-foreground", className)}
    >
      <span className="inline-flex items-center gap-1.5">
        <ins className={ADDED_MARK}>abc</ins>
        {t("revisions.legendAdded")}
      </span>
      <span className="inline-flex items-center gap-1.5">
        <del className={REMOVED_MARK}>abc</del>
        {t("revisions.legendRemoved")}
      </span>
    </p>
  );
}
