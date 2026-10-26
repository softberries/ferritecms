// The 412 conflict dialog (07 §3, Review Focus 3): someone saved the item while this
// screen had unsaved changes. The two versions sit side by side, member by member, with
// what only one of them has marked (struck through on the saved side, underlined on
// yours), so "which one do I keep" is a decision the user can actually make. Three ways
// out, none of which drops text: Keep mine (saved over theirs with their version as the
// `If-Match`), Take theirs (theirs becomes the working draft; mine stays in the autosave
// slot until discarded), Copy my text (to the clipboard, then Take theirs). It never
// closes on an outside click or Esc: one of the three is the answer.

import { useQuery } from "@tanstack/react-query";
import { CopyIcon, GitCompareArrowsIcon, CirclePauseIcon } from "lucide-react";
import { cn } from "cn";
import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { parentRefQuery } from "@/content/parent-ref";
import { type ConflictField, type ConflictState, seoText } from "./conflict";
import { fieldChanges, fieldText } from "./revisions/revision-model";
import { type DiffPart, diffRevision } from "./diff";
import type { Paused } from "./editor-state";
import { useTermsById, useTypeTaxonomies } from "./panel/panel-state";

/** Unchanged text longer than this is shortened to its ends (the change is the point). */
const CONTEXT_CHARS = 160;

export interface OpenConflict extends ConflictState {
  /** The transition that did not run because of the conflict. */
  then?: Paused;
}

export interface ConflictDialogProps {
  type: string;
  conflict: OpenConflict | null;
  onKeepMine: () => Promise<void>;
  onTakeTheirs: () => Promise<void>;
  onCopyMine: () => Promise<void>;
}

export function ConflictDialog(props: ConflictDialogProps) {
  return (
    <Dialog open={props.conflict !== null}>
      {props.conflict ? <ConflictBody {...props} conflict={props.conflict} /> : null}
    </Dialog>
  );
}

type Choice = "mine" | "theirs" | "copy";

function ConflictBody({
  type,
  conflict,
  onKeepMine,
  onTakeTheirs,
  onCopyMine,
}: ConflictDialogProps & { conflict: OpenConflict }) {
  const { t } = useTranslation("editor");
  const [busy, setBusy] = useState<Choice | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { theirs, mine, fields } = conflict;
  const diff = diffRevision(
    { title: theirs.title, excerpt: theirs.excerpt, body: theirs.body },
    { title: mine.title, excerpt: mine.excerpt ?? "", body: mine.body },
  );

  const choose = async (choice: Choice) => {
    setBusy(choice);
    setError(null);
    try {
      if (choice === "mine") await onKeepMine();
      else if (choice === "theirs") await onTakeTheirs();
      else await onCopyMine();
    } catch {
      setError(choice === "copy" ? t("conflict.copyFailed") : t("conflict.failed"));
    } finally {
      setBusy(null);
    }
  };

  return (
    <DialogContent
      data-slot="conflict-dialog"
      showCloseButton={false}
      onInteractOutside={(e) => e.preventDefault()}
      onEscapeKeyDown={(e) => e.preventDefault()}
      className="flex max-h-[calc(100dvh-1rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-4xl"
    >
      <DialogHeader className="flex-row items-start gap-3 border-b px-5 pt-5 pb-4 sm:px-6">
        <span
          aria-hidden="true"
          className="mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-full bg-primary/10 text-primary"
        >
          <GitCompareArrowsIcon className="size-4.5" />
        </span>
        <div className="flex min-w-0 flex-col gap-1.5">
          <DialogTitle className="text-lg leading-snug">{t("conflict.title")}</DialogTitle>
          <DialogDescription>
            {t("conflict.descriptionBefore")} <RelativeTime at={theirs.updated_at} />
            {t("conflict.descriptionAfter")}
          </DialogDescription>
        </div>
      </DialogHeader>

      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4 sm:px-6">
        {conflict.then ? (
          <p
            data-slot="conflict-paused"
            className="mb-4 flex items-start gap-2 rounded-lg bg-muted/60 px-3 py-2.5 text-sm text-foreground"
          >
            <CirclePauseIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-info" />
            {t(`conflict.paused.${conflict.then}`)}
          </p>
        ) : null}
        <div
          aria-hidden="true"
          className="sticky top-0 z-10 -mx-1 mb-3 hidden grid-cols-2 gap-4 bg-popover px-1 pb-2 text-xs font-medium text-muted-foreground md:grid"
        >
          <span>{t("conflict.theirs")}</span>
          <span>{t("conflict.mine")}</span>
        </div>
        <div className="flex flex-col gap-5">
          {fields.map((field) => (
            <FieldDiff key={field} field={field} type={type} conflict={conflict} diff={diff} />
          ))}
        </div>
        <Legend />
      </div>

      <div className="border-t bg-muted/40 px-5 pt-3 sm:px-6">
        <p className="text-xs leading-relaxed text-muted-foreground">{t("conflict.explain")}</p>
        {error ? (
          <p role="alert" className="mt-2 text-sm text-destructive">
            {error}
          </p>
        ) : null}
      </div>
      <DialogFooter className="m-0 rounded-b-xl border-t-0 bg-muted/40 px-5 pt-3 pb-4 sm:px-6">
        <Button
          type="button"
          variant="ghost"
          className="text-muted-foreground sm:mr-auto"
          disabled={busy !== null}
          onClick={() => {
            void choose("copy");
          }}
        >
          <CopyIcon data-icon="inline-start" />
          {t("conflict.copyMine")}
        </Button>
        <Button
          type="button"
          variant="outline"
          data-choice="theirs"
          disabled={busy !== null}
          onClick={() => {
            void choose("theirs");
          }}
        >
          {busy === "theirs" ? t("conflict.taking") : t("conflict.takeTheirs")}
        </Button>
        <Button
          type="button"
          data-choice="mine"
          disabled={busy !== null}
          onClick={() => {
            void choose("mine");
          }}
        >
          {busy === "mine" ? t("conflict.saving") : t("conflict.keepMine")}
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

const FIELD_LABEL = {
  title: "conflict.fields.title",
  excerpt: "conflict.fields.excerpt",
  body: "conflict.fields.body",
  slug: "conflict.fields.slug",
  terms: "conflict.fields.terms",
  parent: "conflict.fields.parent",
  menu_order: "conflict.fields.menuOrder",
  fields: "conflict.fields.fields",
  seo: "conflict.fields.seo",
} as const satisfies Record<ConflictField, string>;

function FieldDiff({
  field,
  type,
  conflict,
  diff,
}: {
  field: ConflictField;
  type: string;
  conflict: OpenConflict;
  diff: ReturnType<typeof diffRevision>;
}) {
  const { t } = useTranslation("editor");
  const { theirs, mine } = conflict;
  let left: ReactNode;
  let right: ReactNode;
  switch (field) {
    case "title":
    case "excerpt":
    case "body": {
      const parts = diff[field];
      left = <DiffText parts={parts} side="theirs" long={field === "body"} />;
      right = <DiffText parts={parts} side="mine" long={field === "body"} />;
      break;
    }
    case "slug":
      left = <code className="font-mono text-xs break-all">/{theirs.slug}</code>;
      right = <code className="font-mono text-xs break-all">/{mine.slug ?? theirs.slug}</code>;
      break;
    case "terms":
      left = <TermList type={type} ids={theirs.terms} />;
      right = <TermList type={type} ids={mine.terms ?? theirs.terms} />;
      break;
    case "parent":
      left = <ParentName type={type} id={theirs.parent_id ?? null} />;
      right = <ParentName type={type} id={mine.parent_id} />;
      break;
    case "menu_order":
      left = <span className="tabular-nums">{theirs.menu_order}</span>;
      right = <span className="tabular-nums">{mine.menu_order ?? theirs.menu_order}</span>;
      break;
    case "fields": {
      // Only the custom fields that differ, by name.
      const changes = fieldChanges(theirs.fields, mine.fields);
      left = <FieldValues rows={changes.map((c) => ({ name: c.name, value: c.before }))} />;
      right = <FieldValues rows={changes.map((c) => ({ name: c.name, value: c.after }))} />;
      break;
    }
    case "seo": {
      const mineSeo = mine.seo ?? {};
      // A member the draft leaves out keeps the stored one; `null` is "not set".
      const minePart = (v: string | null | undefined, stored: string | null) =>
        v === undefined ? stored : seoText(v);
      const rows = (side: "theirs" | "mine") => [
        {
          name: t("conflict.seoTitle"),
          value: side === "theirs" ? theirs.seo.title : minePart(mineSeo.title, theirs.seo.title),
        },
        {
          name: t("conflict.seoDescription"),
          value:
            side === "theirs"
              ? theirs.seo.description
              : minePart(mineSeo.description, theirs.seo.description),
        },
      ];
      left = <FieldValues rows={rows("theirs")} plain />;
      right = <FieldValues rows={rows("mine")} plain />;
      break;
    }
  }
  const label = t(FIELD_LABEL[field]);
  return (
    <section data-slot="conflict-field" data-field={field} aria-label={label}>
      <h3 className="mb-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
        {label}
      </h3>
      <div className="grid gap-2 md:grid-cols-2 md:gap-4">
        <Side kind="theirs">{left}</Side>
        <Side kind="mine">{right}</Side>
      </div>
    </section>
  );
}

/** Named values, one per line (`name: value`); `plain` names are words, not field names. */
function FieldValues({
  rows,
  plain = false,
}: {
  rows: { name: string; value: unknown }[];
  plain?: boolean;
}) {
  return (
    <dl className="flex flex-col gap-1">
      {rows.map((r) => (
        <div key={r.name} className="flex min-w-0 flex-wrap gap-x-1.5">
          <dt className={cn("text-muted-foreground", !plain && "font-mono text-xs leading-6")}>
            {r.name}
          </dt>
          <dd className="min-w-0 break-words">{fieldText(r.value)}</dd>
        </div>
      ))}
    </dl>
  );
}

function Side({ kind, children }: { kind: "theirs" | "mine"; children: ReactNode }) {
  const { t } = useTranslation("editor");
  return (
    <div
      data-side={kind}
      className={cn(
        "min-w-0 rounded-lg border px-3 py-2.5 text-sm leading-relaxed break-words",
        kind === "mine" ? "border-primary/30 bg-primary/[0.03]" : "bg-muted/30",
      )}
    >
      {/* Below `md` the columns stack, so each side says whose it is. */}
      <span className="mb-1 block text-xs font-medium text-muted-foreground md:sr-only">
        {kind === "mine" ? t("conflict.mine") : t("conflict.theirs")}
      </span>
      {children}
    </div>
  );
}

/** Unchanged text cut to its two ends when it is long (a body's untouched paragraphs). */
function shorten(text: string, first: boolean, last: boolean): string {
  if (text.length <= CONTEXT_CHARS * 2) return text;
  const head = first ? "" : text.slice(0, CONTEXT_CHARS);
  const tail = last ? "" : text.slice(-CONTEXT_CHARS);
  if (first && last) return text.slice(0, CONTEXT_CHARS * 2) + " …";
  return `${head}${head ? " … " : "… "}${tail}`;
}

/** Unchanged text this short between two highlights joins them (spaces, `-`, `,`). */
const GLUE = /^[\s\p{P}]{1,3}$/u;

/**
 * The parts one side shows: unchanged text and what only this side has. Highlights
 * separated only by a space or a punctuation mark become one ("The first", not "The"
 * and "first"), so a changed phrase reads as one change.
 */
function sideParts(parts: readonly DiffPart[], side: "theirs" | "mine"): DiffPart[] {
  const own = side === "theirs" ? "removed" : "added";
  const out: DiffPart[] = [];
  for (const p of parts) {
    if (p.kind !== "same" && p.kind !== own) continue;
    const last = out.at(-1);
    const beforeLast = out.at(-2);
    if (
      p.kind === own &&
      last?.kind === "same" &&
      GLUE.test(last.text) &&
      beforeLast?.kind === own
    ) {
      out.pop();
      beforeLast.text += last.text + p.text;
    } else if (last && last.kind === p.kind) {
      last.text += p.text;
    } else {
      out.push({ ...p });
    }
  }
  return out;
}

/**
 * One side of a text diff: the saved side shows what only it has struck through, yours
 * what only yours has underlined; the other side's parts are left out.
 */
function DiffText({
  parts,
  side,
  long,
}: {
  parts: DiffPart[];
  side: "theirs" | "mine";
  long: boolean;
}) {
  const { t } = useTranslation("editor");
  const shown = sideParts(parts, side);
  if (shown.length === 0) {
    return <span className="text-muted-foreground italic">{t("conflict.empty")}</span>;
  }
  return (
    <p
      className={cn("whitespace-pre-wrap", long && "max-h-72 overflow-y-auto")}
      // A scrolling region is reachable by keyboard.
      {...(long ? { tabIndex: 0 } : {})}
    >
      {shown.map((p, i) =>
        p.kind === "same" ? (
          <span key={i}>{long ? shorten(p.text, i === 0, i === shown.length - 1) : p.text}</span>
        ) : p.kind === "removed" ? (
          <del
            key={i}
            className="rounded-[3px] bg-destructive/12 px-0.5 text-foreground decoration-destructive/70 decoration-1"
          >
            <span className="sr-only">{t("conflict.onlyTheirsSr")}</span>
            {p.text}
          </del>
        ) : (
          <ins
            key={i}
            className="rounded-[3px] bg-success/15 px-0.5 text-foreground underline decoration-success decoration-2 underline-offset-2"
          >
            <span className="sr-only">{t("conflict.onlyMineSr")}</span>
            {p.text}
          </ins>
        ),
      )}
    </p>
  );
}

function Legend() {
  const { t } = useTranslation("editor");
  return (
    <p
      aria-hidden="true"
      className="mt-5 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground"
    >
      <span className="inline-flex items-center gap-1.5">
        <del className="rounded-[3px] bg-destructive/12 px-1 text-foreground decoration-destructive/70">
          abc
        </del>
        {t("conflict.legendTheirs")}
      </span>
      <span className="inline-flex items-center gap-1.5">
        <ins className="rounded-[3px] bg-success/15 px-1 text-foreground underline decoration-success decoration-2 underline-offset-2">
          abc
        </ins>
        {t("conflict.legendMine")}
      </span>
    </p>
  );
}

function TermList({ type, ids }: { type: string; ids: readonly string[] }) {
  const { t } = useTranslation("editor");
  const taxonomies = useTypeTaxonomies(type);
  const terms = useTermsById(taxonomies);
  if (ids.length === 0) return <span className="text-muted-foreground">{t("conflict.none")}</span>;
  return (
    <span className="flex flex-wrap gap-1">
      {ids.map((id) => (
        <span
          key={id}
          className="inline-flex h-6 items-center rounded-md border bg-background px-2 text-xs"
        >
          {terms.get(id)?.name ?? t("conflict.unknownTerm")}
        </span>
      ))}
    </span>
  );
}

function ParentName({ type, id }: { type: string; id: string | null }) {
  const { t } = useTranslation("editor");
  const ref = useQuery({ ...parentRefQuery(type, id ?? ""), enabled: id !== null });
  if (id === null) return <span className="text-muted-foreground">{t("parent.none")}</span>;
  if (ref.data) return <span>{ref.data.title || t("title.untitled")}</span>;
  return (
    <span className="text-muted-foreground">
      {ref.isPending ? t("parent.loading") : t("parent.unavailable")}
    </span>
  );
}
