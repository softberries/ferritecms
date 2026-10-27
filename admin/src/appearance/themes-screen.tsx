// The Themes screen (07 §3 Themes; `manage_themes`): every installed theme and every
// built-in one without a row, the active one first. Upload a theme (a folder packed in
// the browser, or a `.tar` bundle; P42); activate one, after a confirm, then follow the
// nodes as they swap (H-T1: `GET /themes` every 2 s for up to 20 s; a node still on the
// old theme after that is a note, not an error); customize one that declares settings.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { UploadIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type ThemeDto, activateTheme, themesQuery } from "@/api/themes";
import { Alert } from "@/auth/auth-layout";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { problemText } from "@/structure/taxonomies/terms-model";
import { type NodeWatch, ThemeCard } from "./theme-card";
import { NODE_POLL_FOR_MS, NODE_POLL_MS, nodesSettled } from "./theme-model";
import { ThemeSettingsSheet } from "./theme-settings-sheet";
import { LoadProblems, UploadThemeDialog } from "./upload-theme-dialog";

/** The active theme first, then by name. */
function ordered(themes: readonly ThemeDto[]): ThemeDto[] {
  return [...themes].sort(
    (a, b) => Number(b.active) - Number(a.active) || a.name.localeCompare(b.name),
  );
}

export function ThemesScreen() {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  // The activation being followed: its theme and when following stops.
  const [watching, setWatching] = useState<{ name: string; until: number } | null>(null);
  const [timedOut, setTimedOut] = useState(false);
  const themes = useQuery({
    ...themesQuery,
    refetchInterval: (query) =>
      watching && Date.now() < watching.until && !nodesSettled(query.state.data, watching.name)
        ? NODE_POLL_MS
        : false,
  });
  useEffect(() => {
    if (!watching) return;
    const timer = setTimeout(() => setTimedOut(true), Math.max(0, watching.until - Date.now()));
    return () => clearTimeout(timer);
  }, [watching]);
  const [uploading, setUploading] = useState(false);
  const [activating, setActivating] = useState<ThemeDto | null>(null);
  const [customizing, setCustomizing] = useState<string | null>(null);
  const list = useMemo(() => ordered(themes.data ?? []), [themes.data]);
  const active = list.find((th) => th.active);

  let watch: NodeWatch = null;
  if (watching && active?.name === watching.name && !nodesSettled(themes.data, watching.name)) {
    watch = timedOut ? "pending" : "checking";
  }

  return (
    <div data-slot="themes-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={t("themes.title")}
        description={t("themes.description")}
        actions={
          <Button onClick={() => setUploading(true)}>
            <UploadIcon data-icon="inline-start" />
            {t("themes.upload")}
          </Button>
        }
      />
      {themes.isError && !themes.data ? (
        <ErrorState error={themes.error} retry={() => void themes.refetch()} />
      ) : themes.isPending ? (
        <div aria-busy="true" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-56 w-full rounded-xl" />
          ))}
        </div>
      ) : (
        <section aria-label={t("themes.list")}>
          <ul data-slot="theme-list" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {list.map((theme) => (
              <li key={theme.name} className="flex min-w-0 flex-col [&>*]:flex-1">
                <ThemeCard
                  theme={theme}
                  watch={theme.active ? watch : null}
                  onActivate={() => setActivating(theme)}
                  onCustomize={() => setCustomizing(theme.name)}
                />
              </li>
            ))}
          </ul>
        </section>
      )}
      <UploadThemeDialog
        open={uploading}
        onOpenChange={setUploading}
        themes={themes.data ?? []}
        onInstalled={(theme) => {
          setUploading(false);
          toast.success(t("themes.uploadDialog.installed", { name: theme.name }));
          void queryClient.invalidateQueries({ queryKey: qk.themes });
        }}
      />
      <ActivateDialog
        theme={activating}
        current={active ?? null}
        onClose={() => setActivating(null)}
        onActivated={(theme) => {
          setActivating(null);
          toast.success(t("themes.confirm.done", { name: theme.name }));
          setTimedOut(false);
          setWatching({ name: theme.name, until: Date.now() + NODE_POLL_FOR_MS });
          // Settings and media sizes follow the active theme.
          void queryClient.invalidateQueries({ queryKey: qk.themes });
          void queryClient.invalidateQueries({ queryKey: qk.menus.list });
        }}
      />
      {customizing ? (
        <ThemeSettingsSheet name={customizing} onClose={() => setCustomizing(null)} />
      ) : null}
    </div>
  );
}

/** The activation confirm; a `422` lists why the theme does not load (nothing changes). */
function ActivateDialog({
  theme,
  current,
  onClose,
  onActivated,
}: {
  theme: ThemeDto | null;
  current: ThemeDto | null;
  onClose: () => void;
  onActivated: (theme: ThemeDto) => void;
}) {
  const { t } = useTranslation("admin");
  // The last theme asked about, so the closing animation keeps its text.
  const [shown, setShown] = useState(theme);
  if (theme && theme !== shown) setShown(theme);
  const [opened, setOpened] = useState(theme);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<{ text: string; problems: string[] } | null>(null);
  if (theme !== opened) {
    setOpened(theme);
    setFailure(null);
  }

  const run = async () => {
    if (!theme || pending) return;
    setPending(true);
    setFailure(null);
    try {
      onActivated(await activateTheme(theme.name));
    } catch (error) {
      reportApiError(error);
      if (error instanceof ApiError && error.status === 422) {
        const problems = (error.problem.errors ?? []).map((e) => e.detail);
        setFailure({
          text: t("themes.confirm.doesNotLoad"),
          problems: problems.length > 0 ? problems : [error.message],
        });
      } else {
        setFailure({ text: problemText(error), problems: [] });
      }
    } finally {
      setPending(false);
    }
  };

  const name = shown?.name ?? "";
  return (
    <AlertDialog open={theme !== null} onOpenChange={(open) => !open && !pending && onClose()}>
      <AlertDialogContent data-slot="activate-theme">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">
            {t("themes.confirm.title", { name })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {current && current.name !== name
              ? t("themes.confirm.body", { current: current.name })
              : t("themes.confirm.bodyNoCurrent")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {failure ? (
          failure.problems.length > 0 ? (
            <LoadProblems lead={failure.text} problems={failure.problems} />
          ) : (
            <Alert tone="error">{failure.text}</Alert>
          )
        ) : null}
        <AlertDialogFooter>
          <Button variant="outline" onClick={onClose} disabled={pending}>
            {t("themes.confirm.cancel")}
          </Button>
          <Button onClick={() => void run()} disabled={pending} aria-busy={pending || undefined}>
            {pending ? t("themes.confirm.activating") : t("themes.confirm.confirm")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
