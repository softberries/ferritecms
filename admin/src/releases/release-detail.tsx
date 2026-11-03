// One release (`/admin/releases/$id`, design 15 §11.3): its title and description edited in
// place (`PATCH` with `If-Match`), the status, a failed release's report on top, the
// entries (chips, review state, conflicts with Rebase, remove), Check with the report
// inline, Preview site (a 1 h release link in a new tab), Share preview…, Schedule… /
// Unschedule, Apply now… (with the admins' override) and Delete. Every write answers the
// whole release, which replaces the cached one; a `412` reloads it and says so.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  ArrowLeftIcon,
  CalendarClockIcon,
  CalendarXIcon,
  ExternalLinkIcon,
  ListChecksIcon,
  RocketIcon,
  Share2Icon,
  Trash2Icon,
  TriangleAlertIcon,
} from "lucide-react";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { createPreviewLink, linkPath } from "@/api/preview-links";
import { ApiError } from "@/api/problem";
import {
  type ReleaseCheck,
  type ReleaseDetail,
  type ReleaseEntry,
  MAX_RELEASE_DESCRIPTION,
  MAX_RELEASE_TITLE,
  checkRelease,
  deleteRelease,
  failureOf,
  patchRelease,
  rebaseEntry,
  releaseEditable,
  releaseKeys,
  releaseQuery,
  removeEntry,
  unscheduleRelease,
} from "@/api/releases";
import { useMe } from "@/auth/session";
import { ErrorState } from "@/components/kit/error-state";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import { formatWhen } from "@/editor/panel/publishing";
import { problemText } from "@/structure/taxonomies/terms-model";
import { ApplyDialog } from "./apply-dialog";
import { CheckReport } from "./check-report";
import { EntryTable } from "./entry-table";
import { ReleaseBadge } from "./release-badge";
import { ScheduleDialog } from "./schedule-dialog";
import { ShareDialog } from "./share-dialog";

/** Opens a release's whole-site preview in a new tab through a fresh 1 h link (§5.3). */
async function openSitePreview(releaseId: string): Promise<void> {
  // Opened now, inside the click, so no popup blocker stops it; pointed at the link once
  // it exists.
  const tab = window.open("", "_blank");
  try {
    const link = await createPreviewLink({ kind: "release", id: releaseId }, 1);
    const path = linkPath(link);
    if (!path) throw new Error("no link");
    if (tab) {
      tab.opener = null;
      tab.location.href = path;
    } else window.open(path, "_blank", "noopener");
  } catch (e) {
    tab?.close();
    throw e;
  }
}

export function ReleaseDetailScreen({ id }: { id: string }) {
  const { t } = useTranslation("releases");
  const query = useQuery(releaseQuery(id));
  if (query.isError && !query.data) {
    return (
      <div className="mx-auto w-full max-w-5xl">
        <ErrorState error={query.error} retry={() => void query.refetch()} />
      </div>
    );
  }
  if (!query.data) {
    return (
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-4" aria-busy="true">
        <Skeleton className="h-9 w-72" />
        <Skeleton className="h-40 w-full" />
        <span className="sr-only">{t("title")}</span>
      </div>
    );
  }
  return <Detail release={query.data} />;
}

function Detail({ release }: { release: ReleaseDetail }) {
  const { t } = useTranslation("releases");
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const me = useMe();
  const isAdmin = me?.role === "admin";
  const editable = releaseEditable(release);
  const [busy, setBusy] = useState(false);
  const [check, setCheck] = useState<ReleaseCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [dialog, setDialog] = useState<"apply" | "schedule" | "share" | "delete" | null>(null);
  const titles = new Map(release.entries.map((e) => [e.item_id, e.title]));
  const failure = release.status === "failed" ? failureOf(release) : null;

  const take = (next: ReleaseDetail) => {
    queryClient.setQueryData(releaseKeys.item(release.id), next);
    void queryClient.invalidateQueries({ queryKey: releaseKeys.lists });
  };
  const stale = () => {
    toast.info(t("detail.stale"), { id: "release-stale" });
    void queryClient.invalidateQueries({ queryKey: releaseKeys.item(release.id) });
  };

  /** One write with the release's version; its answer replaces the cached release. */
  const write = async (run: (version: number) => Promise<ReleaseDetail>, done?: string) => {
    setBusy(true);
    try {
      take(await run(release.version));
      setCheck(null);
      if (done) toast.success(done, { id: "release-write" });
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.status === 412) stale();
      else
        toast.error(t("detail.actionFailed", { detail: problemText(e) }), { id: "release-write" });
      return false;
    } finally {
      setBusy(false);
    }
  };

  const runCheck = async () => {
    setChecking(true);
    try {
      setCheck(await checkRelease(release.id));
    } catch (e) {
      toast.error(t("detail.actionFailed", { detail: problemText(e) }), { id: "release-check" });
    } finally {
      setChecking(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      await deleteRelease(release.id, release.version);
      queryClient.removeQueries({ queryKey: releaseKeys.item(release.id) });
      void queryClient.invalidateQueries({ queryKey: releaseKeys.lists });
      toast.success(t("detail.deleted"), { id: "release-write" });
      void navigate({ to: "/releases" });
    } catch (e) {
      if (e instanceof ApiError && e.status === 412) stale();
      else
        toast.error(t("detail.actionFailed", { detail: problemText(e) }), { id: "release-write" });
    } finally {
      setBusy(false);
      setDialog(null);
    }
  };

  return (
    <div data-slot="release-detail" className="mx-auto flex w-full max-w-5xl flex-col gap-5">
      <div className="flex flex-col gap-3">
        <Button
          asChild
          variant="ghost"
          size="sm"
          className="-ml-2 w-fit gap-1.5 text-muted-foreground"
        >
          <Link to="/releases">
            <ArrowLeftIcon data-icon="inline-start" />
            {t("detail.back")}
          </Link>
        </Button>
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex min-w-0 flex-1 flex-col gap-2">
            <h1 className="sr-only">{release.title}</h1>
            <InPlace
              key={`title-${release.version}`}
              label={t("detail.titleLabel")}
              value={release.title}
              max={MAX_RELEASE_TITLE}
              disabled={!editable || busy}
              onSave={(title) =>
                write((v) => patchRelease(release.id, v, { title }), t("detail.saved"))
              }
              render={(props) => (
                <Input
                  {...props}
                  className="h-auto border-transparent bg-transparent px-1 py-0.5 text-2xl font-semibold shadow-none hover:border-border focus-visible:border-ring md:text-2xl"
                />
              )}
            />
            <InPlace
              key={`desc-${release.version}`}
              label={t("detail.descriptionLabel")}
              value={release.description}
              max={MAX_RELEASE_DESCRIPTION}
              allowEmpty
              disabled={!editable || busy}
              placeholder={t("detail.descriptionPlaceholder")}
              onSave={(description) =>
                write((v) => patchRelease(release.id, v, { description }), t("detail.saved"))
              }
              render={(props) => (
                <Textarea
                  {...props}
                  rows={2}
                  className="min-h-0 resize-none border-transparent bg-transparent px-1 text-sm text-muted-foreground shadow-none hover:border-border focus-visible:border-ring"
                />
              )}
            />
          </div>
          <ReleaseBadge status={release.status} className="mt-2" />
        </div>
        <dl className="flex flex-wrap gap-x-6 gap-y-1 text-sm">
          <Meta label={t("detail.created")} value={formatWhen(release.created_at)} />
          {release.scheduled_at && release.status === "scheduled" ? (
            <Meta label={t("detail.goesLive")} value={formatWhen(release.scheduled_at)} />
          ) : null}
          {release.applied_at ? (
            <Meta label={t("detail.applied")} value={formatWhen(release.applied_at)} />
          ) : null}
        </dl>
      </div>

      {failure ? (
        <div
          role="alert"
          data-slot="release-failure"
          className="flex flex-col gap-2 rounded-lg border border-destructive/40 bg-destructive/6 px-4 py-3"
        >
          <p className="flex items-center gap-2 text-sm font-medium text-foreground">
            <TriangleAlertIcon aria-hidden="true" className="size-4 text-destructive" />
            {t("failure.title")}
            {failure.at ? (
              <span className="font-normal text-muted-foreground">
                · {t("failure.at", { when: formatWhen(failure.at) })}
              </span>
            ) : null}
          </p>
          <p className="text-sm text-muted-foreground">{failure.reason ?? t("failure.body")}</p>
          {failure.entries.length > 0 ? (
            <CheckReport entries={failure.entries} titles={titles} heading={false} problemsOnly />
          ) : null}
        </div>
      ) : null}
      {release.status === "scheduled" ? (
        <p role="note" className="rounded-lg bg-muted/60 px-4 py-2.5 text-sm text-muted-foreground">
          {t("detail.locked")}
        </p>
      ) : release.status === "applied" ? (
        <p role="note" className="rounded-lg bg-muted/60 px-4 py-2.5 text-sm text-muted-foreground">
          {t("detail.appliedNote")}
        </p>
      ) : null}

      {release.status !== "applied" ? (
        <div data-slot="release-actions" className="flex flex-wrap gap-2">
          {editable ? (
            <Button type="button" disabled={busy} onClick={() => setDialog("apply")}>
              <RocketIcon data-icon="inline-start" />
              {t("detail.apply")}
            </Button>
          ) : null}
          {editable ? (
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => setDialog("schedule")}
            >
              <CalendarClockIcon data-icon="inline-start" />
              {t("detail.schedule")}
            </Button>
          ) : (
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() =>
                void write((v) => unscheduleRelease(release.id, v), t("detail.unscheduled"))
              }
            >
              <CalendarXIcon data-icon="inline-start" />
              {t("detail.unschedule")}
            </Button>
          )}
          <Button
            type="button"
            variant="outline"
            disabled={checking}
            onClick={() => void runCheck()}
          >
            <ListChecksIcon data-icon="inline-start" />
            {checking ? t("detail.checking") : t("detail.check")}
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              openSitePreview(release.id).catch((e: unknown) =>
                toast.error(t("detail.previewFailed", { detail: problemText(e) }), {
                  id: "release-preview",
                }),
              );
            }}
          >
            <ExternalLinkIcon data-icon="inline-start" />
            {t("detail.previewSite")}
          </Button>
          <Button type="button" variant="outline" onClick={() => setDialog("share")}>
            <Share2Icon data-icon="inline-start" />
            {t("detail.share")}
          </Button>
          {editable ? (
            <Button
              type="button"
              variant="ghost"
              className="text-destructive hover:text-destructive sm:ml-auto"
              disabled={busy}
              onClick={() => setDialog("delete")}
            >
              <Trash2Icon data-icon="inline-start" />
              {t("detail.delete")}
            </Button>
          ) : null}
        </div>
      ) : null}

      {check ? <CheckReport entries={check.entries} titles={titles} /> : null}

      <EntryTable
        releaseId={release.id}
        entries={release.entries}
        editable={editable}
        busy={busy}
        onRebase={(e: ReleaseEntry) =>
          void write((v) => rebaseEntry(release.id, e.item_id, v), t("entries.rebased"))
        }
        onRemove={(e: ReleaseEntry) => void write((v) => removeEntry(release.id, e.item_id, v))}
      />

      <ApplyDialog
        release={release}
        open={dialog === "apply"}
        onOpenChange={(o) => setDialog(o ? "apply" : null)}
        canOverride={isAdmin}
        onApplied={take}
        onStale={stale}
      />
      <ScheduleDialog
        release={release}
        open={dialog === "schedule"}
        onOpenChange={(o) => setDialog(o ? "schedule" : null)}
        canOverride={isAdmin}
        onScheduled={take}
        onStale={stale}
      />
      <ShareDialog
        target={{ kind: "release", id: release.id }}
        open={dialog === "share"}
        onOpenChange={(o) => setDialog(o ? "share" : null)}
      />
      <AlertDialog open={dialog === "delete"} onOpenChange={(o) => setDialog(o ? "delete" : null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("detail.deleteConfirm", { title: release.title })}
            </AlertDialogTitle>
            <AlertDialogDescription>{t("detail.deleteBody")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("newDialog.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={(e) => {
                e.preventDefault();
                void remove();
              }}
            >
              {t("detail.deleteAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex gap-1.5">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="text-foreground tabular-nums">{value}</dd>
    </div>
  );
}

interface InPlaceControl {
  "aria-label": string;
  value: string;
  disabled: boolean;
  maxLength: number;
  placeholder?: string;
  onChange: (e: { target: { value: string } }) => void;
  onBlur: () => void;
  onKeyDown: (e: { key: string; currentTarget: HTMLElement; preventDefault: () => void }) => void;
}

/** A text edited in place: saved on blur (or Enter in a one-line field) when it changed. */
function InPlace({
  label,
  value,
  max,
  allowEmpty = false,
  disabled,
  placeholder,
  onSave,
  render,
}: {
  label: string;
  value: string;
  max: number;
  allowEmpty?: boolean;
  disabled: boolean;
  placeholder?: string;
  onSave: (next: string) => Promise<boolean>;
  render: (props: InPlaceControl) => ReactNode;
}) {
  const [text, setText] = useState(value);
  const commit = () => {
    const next = allowEmpty ? text : text.trim();
    if (next === value) return;
    if (!allowEmpty && next === "") {
      setText(value);
      return;
    }
    void onSave(next).then((ok) => {
      if (!ok) setText(value);
    });
  };
  return render({
    "aria-label": label,
    value: text,
    disabled,
    maxLength: max,
    ...(placeholder ? { placeholder } : {}),
    onChange: (e) => setText(e.target.value),
    onBlur: commit,
    onKeyDown: (e) => {
      if (e.key === "Enter" && e.currentTarget instanceof HTMLInputElement) {
        e.preventDefault();
        e.currentTarget.blur();
      } else if (e.key === "Escape") setText(value);
    },
  });
}
