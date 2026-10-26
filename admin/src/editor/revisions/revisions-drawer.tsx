// The revisions drawer (Task 17; 07 §3, 11 §5; after Google Docs' version history and
// Linear's history pane): a drawer from the right (`--dur-slow`, `mod+alt+r`). On the
// left, every kept revision (newest first, at most 50) by day, with who saved it, when,
// and the `restored from …` note of a restore; on the right, the selected revision as
// text with what differs marked: compared with the current item (what a restore brings
// back and what it removes), or with the save before it (what that save changed). One
// primary action: Restore this version. Below 768 px it is a full-screen drawer that
// shows the list first, then the chosen revision with a way back.
//
// The drawer reads; restoring is the editor's (`onRestore`: the unsaved-edits question,
// the `412` conflict flow, the Undo toast).

import { useQuery } from "@tanstack/react-query";
import { ArrowLeftIcon, CheckIcon, HistoryIcon, RotateCcwIcon } from "lucide-react";
import { cn } from "cn";
import { useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { revisionQuery, revisionsQuery } from "@/api/content";
import { ErrorState } from "@/components/kit/error-state";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { diffRevision, type RevisionSide } from "../diff";
import type { ContentDto } from "../editor-state";
import { DiffLegend, RevisionDiff } from "./revision-diff";
import {
  type RevisionDiffs,
  type RevisionSummary,
  anyChange,
  changed,
  fmt,
  groupByDay,
  initials,
  restoredFrom,
  timeOf,
  totalStats,
  whenOf,
} from "./revision-model";

export type CompareWith = "current" | "previous";

export interface RevisionsDrawerProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  type: string;
  /** The item as the server holds it now (what "current" compares with). */
  item: ContentDto;
  /** The revision being restored, else `null`. */
  restoring: string | null;
  onRestore: (revision: RevisionSummary) => void;
}

const sideOf = (r: { title: string; excerpt: string; body: unknown }): RevisionSide => ({
  title: r.title,
  excerpt: r.excerpt,
  body: r.body,
});

const authorName = (r: RevisionSummary) => r.author.display_name || r.author.login;

export function RevisionsDrawer({
  open,
  onOpenChange,
  type,
  item,
  restoring,
  onRestore,
}: RevisionsDrawerProps) {
  const { t } = useTranslation("editor");
  // Where the focus was when the drawer opened (it is opened by a shortcut or the panel,
  // never by a Radix trigger): it goes back there on close.
  const opener = useRef<HTMLElement | null>(null);
  const list = useQuery({ ...revisionsQuery(type, item.id), enabled: open });
  const items = useMemo(() => list.data?.items ?? [], [list.data]);
  const [picked, setPicked] = useState<string | null>(null);
  const [compare, setCompare] = useState<CompareWith>("current");
  // Below 768 px the list and the revision take turns.
  const [narrowView, setNarrowView] = useState<"list" | "detail">("list");

  // The first revision is the current one; the one before it is the first worth a look.
  const selected = items.find((r) => r.id === picked) ?? items[1] ?? items[0] ?? null;
  const index = selected ? items.indexOf(selected) : -1;
  const previous = index >= 0 ? (items[index + 1] ?? null) : null;

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        data-slot="revisions-drawer"
        onOpenAutoFocus={() => {
          // Still the element that had the focus before the drawer took it.
          opener.current = document.activeElement as HTMLElement | null;
        }}
        onCloseAutoFocus={(e) => {
          const back = opener.current;
          opener.current = null;
          if (back?.isConnected) {
            e.preventDefault();
            back.focus();
          }
        }}
        className="gap-0 p-0 duration-(--dur-slow) ease-(--ease-out) data-[side=right]:w-full data-[side=right]:sm:max-w-none data-[side=right]:lg:w-[min(76rem,calc(100vw-4rem))]"
      >
        <SheetHeader className="h-14 shrink-0 flex-row items-center gap-3 border-b py-0 pr-14 pl-4 sm:pl-5">
          <span
            aria-hidden="true"
            className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
          >
            <HistoryIcon className="size-4" />
          </span>
          <div className="flex min-w-0 flex-col">
            <SheetTitle className="flex items-center gap-2 text-sm font-semibold">
              {t("revisions.title")}
              <KbdCombo keys="mod+alt+r" className="max-sm:hidden" />
            </SheetTitle>
            <SheetDescription className="truncate text-xs tabular-nums">
              {list.data
                ? `${t("revisions.count", { count: items.length })} · ${t("revisions.kept")}`
                : t("revisions.description")}
            </SheetDescription>
          </div>
        </SheetHeader>
        <div className="flex min-h-0 flex-1">
          <nav
            aria-label={t("revisions.list")}
            data-slot="revisions-list"
            className={cn(
              "min-h-0 w-full flex-col border-r bg-muted/25 md:flex md:w-72 md:shrink-0 lg:w-80",
              narrowView === "list" ? "flex" : "hidden",
            )}
          >
            {list.isPending ? (
              <ListSkeleton />
            ) : list.isError ? (
              <div className="p-4">
                <ErrorState error={list.error} retry={() => void list.refetch()} />
              </div>
            ) : items.length === 0 ? (
              <p className="px-5 py-8 text-sm text-muted-foreground">{t("revisions.empty")}</p>
            ) : (
              <RevisionList
                items={items}
                selected={selected?.id ?? null}
                onSelect={(id) => {
                  setPicked(id);
                  setNarrowView("detail");
                }}
              />
            )}
          </nav>
          <section
            aria-label={selected ? whenOf(selected.created_at) : t("revisions.title")}
            data-slot="revision-detail"
            className={cn(
              "min-h-0 min-w-0 flex-1 flex-col md:flex",
              narrowView === "detail" ? "flex" : "hidden",
            )}
          >
            {selected ? (
              <RevisionDetail
                key={selected.id}
                type={type}
                item={item}
                revision={selected}
                previous={previous}
                isNewest={index === 0}
                restoredFromAt={restoredAt(items, selected)}
                compare={compare}
                onCompare={setCompare}
                restoring={restoring}
                onRestore={() => onRestore(selected)}
                onBack={() => setNarrowView("list")}
              />
            ) : list.isPending ? (
              <DetailSkeleton />
            ) : null}
          </section>
        </div>
      </SheetContent>
    </Sheet>
  );
}

/** When the revision a restore came from was saved: `undefined` if it is no restore,
 * `null` if that revision is no longer kept. */
function restoredAt(
  items: readonly RevisionSummary[],
  r: RevisionSummary,
): string | null | undefined {
  const from = restoredFrom(r.note);
  if (from === null) return undefined;
  return items.find((x) => x.id === from)?.created_at ?? null;
}

function RevisionList({
  items,
  selected,
  onSelect,
}: {
  items: RevisionSummary[];
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  const { t } = useTranslation("editor");
  const groups = groupByDay(items);
  const root = useRef<HTMLDivElement>(null);
  const first = items[0]?.id;

  // ↑/↓ move through the list (and select), like a list of messages.
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const buttons = [...(root.current?.querySelectorAll<HTMLButtonElement>("[data-rid]") ?? [])];
    const at = buttons.findIndex((b) => b === document.activeElement);
    const next =
      buttons[Math.max(0, Math.min(buttons.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)))];
    if (!next) return;
    e.preventDefault();
    next.focus();
    const id = next.dataset.rid;
    if (id) onSelect(id);
  };

  return (
    <div
      ref={root}
      onKeyDown={onKey}
      className="min-h-0 flex-1 overflow-y-auto overscroll-contain pb-[env(safe-area-inset-bottom)]"
    >
      {groups.map((g) => (
        <div key={g.key} role="group" aria-labelledby={`rev-day-${g.key}`}>
          <h3
            id={`rev-day-${g.key}`}
            className="sticky top-0 z-10 bg-muted/90 px-5 pt-4 pb-1.5 text-xs font-medium text-muted-foreground backdrop-blur-sm supports-backdrop-filter:bg-muted/70"
          >
            {g.day === "earlier"
              ? fmt({
                  weekday: "long",
                  day: "numeric",
                  month: "long",
                  ...(g.date.getFullYear() === new Date().getFullYear() ? {} : { year: "numeric" }),
                }).format(g.date)
              : t(`revisions.${g.day}`)}
          </h3>
          <ul className="flex flex-col px-2 pb-1">
            {g.items.map((r) => (
              <li key={r.id}>
                <RevisionRow
                  revision={r}
                  current={r.id === first}
                  selected={r.id === selected}
                  restoredFromAt={restoredAt(items, r)}
                  onSelect={() => onSelect(r.id)}
                />
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

function RevisionRow({
  revision: r,
  current,
  selected,
  restoredFromAt,
  onSelect,
}: {
  revision: RevisionSummary;
  current: boolean;
  selected: boolean;
  restoredFromAt: string | null | undefined;
  onSelect: () => void;
}) {
  const { t } = useTranslation("editor");
  const name = authorName(r);
  return (
    <button
      type="button"
      data-rid={r.id}
      data-slot="revision-row"
      aria-current={selected ? "true" : undefined}
      onClick={onSelect}
      className={cn(
        "group/row relative flex min-h-11 w-full gap-3 rounded-lg py-2.5 pr-3 pl-3 text-left transition-colors duration-(--dur-instant) hover:bg-muted",
        selected && "bg-background shadow-(--shadow-1) ring-1 ring-border hover:bg-background",
      )}
    >
      {/* The timeline: a dot per save on a thin rail. */}
      <span aria-hidden="true" className="relative flex w-3 shrink-0 justify-center pt-1.5">
        <span
          className={cn(
            "size-2.5 rounded-full border-2 border-muted-foreground/50 bg-background",
            selected && "border-primary bg-primary",
            current && !selected && "border-primary",
          )}
        />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="flex items-center gap-2">
          <span className="text-sm font-medium text-foreground tabular-nums">
            <time dateTime={r.created_at} title={whenOf(r.created_at)}>
              {timeOf(r.created_at)}
            </time>
          </span>
          {current ? (
            <span className="rounded-full bg-primary/10 px-1.5 py-px text-[0.6875rem] font-medium text-foreground ring-1 ring-primary/30 ring-inset">
              {t("revisions.current")}
            </span>
          ) : null}
          <RelativeTime
            at={r.created_at}
            className="ml-auto shrink-0 text-xs text-muted-foreground"
          />
        </span>
        <span
          className="truncate text-sm text-muted-foreground"
          title={r.title || t("title.untitled")}
        >
          {r.title || t("title.untitled")}
        </span>
        <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Initials name={name} />
          <span className="truncate" title={name}>
            {name}
          </span>
        </span>
        {restoredFromAt !== undefined ? <RestoredNote at={restoredFromAt} /> : null}
      </span>
    </button>
  );
}

function RestoredNote({ at }: { at: string | null }) {
  const { t } = useTranslation("editor");
  return (
    <span
      data-slot="revision-note"
      className="inline-flex w-fit max-w-full items-center gap-1 rounded-md bg-info/10 px-1.5 py-0.5 text-xs text-foreground"
    >
      <RotateCcwIcon aria-hidden="true" className="size-3 shrink-0 text-info" />
      <span className="truncate">
        {at
          ? t("revisions.restoredFrom", { when: whenOf(at) })
          : t("revisions.restoredFromUnknown")}
      </span>
    </span>
  );
}

function Initials({ name, large }: { name: string; large?: boolean }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "flex shrink-0 items-center justify-center rounded-full bg-primary/12 font-semibold text-foreground",
        large ? "size-6 text-[0.625rem]" : "size-4.5 text-[0.5625rem]",
      )}
    >
      {initials(name)}
    </span>
  );
}

function RevisionDetail({
  type,
  item,
  revision,
  previous,
  isNewest,
  restoredFromAt,
  compare,
  onCompare,
  restoring,
  onRestore,
  onBack,
}: {
  type: string;
  item: ContentDto;
  revision: RevisionSummary;
  previous: RevisionSummary | null;
  isNewest: boolean;
  restoredFromAt: string | null | undefined;
  compare: CompareWith;
  onCompare: (c: CompareWith) => void;
  restoring: string | null;
  onRestore: () => void;
  onBack: () => void;
}) {
  const { t } = useTranslation("editor");
  const full = useQuery(revisionQuery(type, item.id, revision.id));
  const mode: CompareWith = compare === "previous" && previous ? "previous" : "current";
  const prev = useQuery({
    ...revisionQuery(type, item.id, previous?.id ?? ""),
    enabled: previous !== null && mode === "previous",
  });

  const vsCurrent = useMemo(
    () => (full.data ? diffRevision(sideOf(item), sideOf(full.data)) : null),
    [full.data, item],
  );
  const vsPrevious = useMemo(
    () => (full.data && prev.data ? diffRevision(sideOf(prev.data), sideOf(full.data)) : null),
    [full.data, prev.data],
  );
  const diff = mode === "previous" ? vsPrevious : vsCurrent;
  const isCurrent = vsCurrent !== null && !anyChange(vsCurrent);
  const busy = restoring !== null;
  const name = authorName(revision);

  const restoreButton = (className?: string) => (
    <Button
      type="button"
      data-slot="revision-restore"
      className={className}
      disabled={busy || full.data === undefined || isCurrent}
      onClick={onRestore}
      {...(isCurrent ? { title: t("revisions.isCurrent") } : {})}
    >
      <RotateCcwIcon data-icon="inline-start" />
      {restoring === revision.id ? t("revisions.restoring") : t("revisions.restore")}
    </Button>
  );

  return (
    <>
      <div className="flex shrink-0 flex-col gap-3 border-b px-4 py-3 sm:px-6 sm:py-4">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="-ml-2 h-11 w-fit gap-1.5 text-muted-foreground md:hidden"
          onClick={onBack}
        >
          <ArrowLeftIcon data-icon="inline-start" />
          {t("revisions.back")}
        </Button>
        <div className="flex items-start gap-4">
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <h3 className="text-base font-semibold text-foreground tabular-nums">
              {whenOf(revision.created_at)}
            </h3>
            <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground">
              <span className="inline-flex items-center gap-1.5">
                <Initials name={name} large />
                {t("revisions.savedBy", { name })}
              </span>
              <span aria-hidden="true">·</span>
              <RelativeTime at={revision.created_at} />
              {isNewest ? (
                <span className="rounded-full bg-primary/10 px-1.5 py-px text-xs font-medium text-foreground ring-1 ring-primary/30 ring-inset">
                  {t("revisions.current")}
                </span>
              ) : null}
            </p>
            {restoredFromAt !== undefined ? (
              <span className="mt-1">
                <RestoredNote at={restoredFromAt} />
              </span>
            ) : null}
          </div>
          {restoreButton("max-md:hidden")}
        </div>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <div className="flex items-center gap-2">
            <span
              id={`rev-compare-${revision.id}`}
              className="text-xs font-medium text-muted-foreground"
            >
              {t("revisions.compare")}
            </span>
            <ToggleGroup
              type="single"
              spacing={0}
              value={mode}
              aria-labelledby={`rev-compare-${revision.id}`}
              onValueChange={(v) => {
                if (v === "current" || v === "previous") onCompare(v);
              }}
              className="rounded-lg bg-muted p-0.5"
            >
              {(["current", "previous"] as const).map((m) => (
                <ToggleGroupItem
                  key={m}
                  value={m}
                  disabled={m === "previous" && previous === null}
                  className="h-11 rounded-md! border-0 px-3 text-xs text-muted-foreground data-[state=on]:bg-background data-[state=on]:text-foreground data-[state=on]:shadow-(--shadow-1) sm:h-7"
                >
                  {m === "current" ? t("revisions.compareCurrent") : t("revisions.comparePrevious")}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
          </div>
          {diff ? <Stats diff={diff} /> : null}
          <DiffLegend className="ml-auto max-sm:hidden" />
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain" tabIndex={0}>
        <div className="mx-auto w-full max-w-[42rem] px-4 pt-5 pb-16 sm:px-8 sm:pt-8">
          {full.isError ? (
            <ErrorState error={full.error} retry={() => void full.refetch()} />
          ) : mode === "previous" && prev.isError ? (
            <ErrorState error={prev.error} retry={() => void prev.refetch()} />
          ) : !diff || !full.data ? (
            <DiffSkeleton />
          ) : (
            <DiffBody diff={diff} mode={mode} isCurrent={isCurrent} />
          )}
        </div>
      </div>

      <div className="shrink-0 border-t bg-popover px-4 pt-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] md:hidden">
        {restoreButton("h-11 w-full")}
      </div>
    </>
  );
}

function Stats({ diff }: { diff: RevisionDiffs }) {
  const { t } = useTranslation("editor");
  const { added, removed } = totalStats(diff);
  if (added === 0 && removed === 0) return null;
  return (
    <p
      data-slot="revision-stats"
      className="flex items-center gap-2 text-xs font-medium tabular-nums"
    >
      <span className="rounded-md bg-success/12 px-1.5 py-0.5 text-foreground">
        <span aria-hidden="true">+{added}</span>
        <span className="sr-only">{t("revisions.added", { count: added })}</span>
      </span>
      <span className="rounded-md bg-destructive/10 px-1.5 py-0.5 text-foreground">
        <span aria-hidden="true">−{removed}</span>
        <span className="sr-only">{t("revisions.removed", { count: removed })}</span>
      </span>
    </p>
  );
}

function DiffBody({
  diff,
  mode,
  isCurrent,
}: {
  diff: RevisionDiffs;
  mode: CompareWith;
  isCurrent: boolean;
}) {
  const { t } = useTranslation("editor");
  const same = !anyChange(diff);
  let note: ReactNode;
  if (same) {
    note = (
      <p className="flex items-center gap-2 rounded-lg border bg-card px-3 py-2.5 text-sm text-foreground">
        <CheckIcon aria-hidden="true" className="size-4 shrink-0 text-success" />
        {mode === "current" || isCurrent ? t("revisions.same") : t("revisions.samePrevious")}
      </p>
    );
  } else {
    note = (
      <p className="text-sm text-muted-foreground">
        {mode === "current" ? t("revisions.explainCurrent") : t("revisions.explainPrevious")}
      </p>
    );
  }
  return (
    <article data-slot="revision-text" className="flex flex-col gap-6">
      {note}
      <Field label={t("revisions.fields.title")}>
        <RevisionDiff
          parts={diff.title}
          className="text-2xl leading-tight font-semibold tracking-tight text-foreground sm:text-[1.75rem]"
        />
      </Field>
      {changed(diff.excerpt) ? (
        <Field label={t("revisions.fields.excerpt")}>
          <RevisionDiff parts={diff.excerpt} className="text-sm leading-6 text-muted-foreground" />
        </Field>
      ) : null}
      <Field label={t("revisions.fields.body")}>
        <RevisionDiff parts={diff.body} className="text-[1.0625rem] leading-7 text-foreground" />
      </Field>
    </article>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section aria-label={label} className="flex flex-col gap-1.5">
      <h4 className="text-[0.6875rem] font-semibold tracking-wider text-muted-foreground uppercase">
        {label}
      </h4>
      {children}
    </section>
  );
}

function ListSkeleton() {
  return (
    <div className="flex flex-col gap-1 px-4 pt-5" aria-hidden="true">
      <Skeleton className="mb-2 h-3 w-16" />
      {[0, 1, 2, 3, 4].map((i) => (
        <div key={i} className="flex gap-3 py-2.5">
          <Skeleton className="mt-1 size-2.5 rounded-full" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-3.5 w-24" />
            <Skeleton className="h-3 w-40" />
          </div>
        </div>
      ))}
    </div>
  );
}

function DetailSkeleton() {
  return (
    <div className="flex flex-col gap-3 px-6 py-5" aria-hidden="true">
      <Skeleton className="h-4 w-48" />
      <Skeleton className="h-3 w-32" />
      <div className="mx-auto mt-6 w-full max-w-[42rem]">
        <DiffSkeleton />
      </div>
    </div>
  );
}

function DiffSkeleton() {
  return (
    <div className="flex flex-col gap-3" aria-hidden="true">
      <Skeleton className="h-7 w-3/4" />
      <Skeleton className="mt-4 h-4 w-full" />
      <Skeleton className="h-4 w-full" />
      <Skeleton className="h-4 w-5/6" />
      <Skeleton className="h-4 w-2/3" />
    </div>
  );
}

/**
 * Restoring over unsaved edits (they are replaced, the saved version stays in the
 * history): asked once. Cancel is the safe default (it has the focus).
 */
export function RestoreConfirmDialog({
  open,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation("editor");
  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onCancel();
      }}
    >
      <AlertDialogContent data-slot="revision-restore-confirm" className="sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("revisions.confirmTitle")}</AlertDialogTitle>
          <AlertDialogDescription>{t("revisions.confirmDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <Button type="button" variant="outline" autoFocus onClick={onCancel}>
            {t("revisions.confirmCancel")}
          </Button>
          <Button type="button" variant="destructive" onClick={onConfirm}>
            {t("revisions.confirmReplace")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
