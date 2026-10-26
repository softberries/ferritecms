// The signed-in area: every route under `_app` passes the login guard first (pre-flight
// F19). No session → `/login?next=…`; a session that must change its password →
// `/change-password` (Review Focus 4). Then the shell: sidebar, top bar, route
// transitions, global shortcuts, the ⌘K palette, the `?` sheet and the toaster. A route
// with `staticData.bare` (the styleguide) renders without the shell.

import { useQuery } from "@tanstack/react-query";
import {
  Outlet,
  createFileRoute,
  useMatches,
  useNavigate,
  useRouter,
} from "@tanstack/react-router";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { commentCountsQuery } from "@/api/comments";
import { metaQuery } from "@/api/meta";
import { settingsQuery } from "@/api/settings";
import { typesQuery } from "@/api/types";
import { type MeDto, enterShell, useLogout, useMe } from "@/auth/session";
import { ErrorState } from "@/components/kit/error-state";
import { SidebarInset, SidebarProvider, useSidebar } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { storeSidebarOpen, storedSidebarOpen } from "@/design/preferences";
import { registerShortcut, useShortcut } from "@/lib/shortcuts";
import { AppSidebar } from "@/shell/app-sidebar";
import { type ShellChrome, ShellChromeContext } from "@/shell/chrome-context";
import { MediaPickerHost } from "@/media/media-picker";
import { CommandPalette } from "@/palette/palette";
import { pushRecent } from "@/palette/recent";
import { type NavSection, navFor, navItemAt, withBadges } from "@/shell/nav";
import { RouteTransition } from "@/shell/route-transition";
import { ShortcutSheet } from "@/shell/shortcut-sheet";
import { TopBar } from "@/shell/top-bar";

export const Route = createFileRoute("/_app")({
  beforeLoad: async (ctx) => ({ me: await enterShell(ctx) }),
  component: AppLayout,
  errorComponent: AppError,
});

/** The guard could not ask the server (offline, 5xx): say so and offer a retry. */
function AppError({ error }: { error: unknown }) {
  const router = useRouter();
  return (
    <main id="main" className="mx-auto flex min-h-svh max-w-xl flex-col justify-center p-page">
      <ErrorState
        error={error}
        retry={() => {
          void router.invalidate();
        }}
      />
    </main>
  );
}

function AppLayout() {
  const bare = useMatches({ select: (ms) => ms.some((m) => m.staticData.bare === true) });
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  return bare ? <Outlet /> : <Shell me={me} />;
}

function Shell({ me }: { me: MeDto }) {
  const { t } = useTranslation("shell");
  const { t: tc } = useTranslation();
  const logout = useLogout();
  const canRead = me.capabilities.includes("read_admin");
  const types = useQuery({ ...typesQuery, enabled: canRead });
  const settings = useQuery({ ...settingsQuery, enabled: canRead });
  const site = useQuery(metaQuery);
  // `t` changes with the language, so the nav relabels with it.
  const sections = useMemo(() => navFor(me, types.data ?? [], t), [me, types.data, t]);
  // The pending comments on the Comments item, for those who moderate (the nav shows the
  // item only to them); refreshed after every moderation (`qk.comments.all`) and each
  // minute. Only the sidebar gets the badges: the chords, palette and Recent use
  // `sections`, so a new count never re-registers them.
  const canModerate = canRead && me.capabilities.includes("moderate_comments");
  const commentCounts = useQuery({
    ...commentCountsQuery(),
    enabled: canModerate,
    refetchInterval: 60_000,
  });
  const pendingComments = canModerate ? commentCounts.data?.pending : undefined;
  const badged = useMemo(
    () => withBadges(sections, { comments: pendingComments }),
    [sections, pendingComments],
  );

  const [sidebarOpen, setSidebarOpen] = useState(storedSidebarOpen);
  // The editor's focus mode: sidebar and top bar out of the way (11 §6 `mod+shift+f`).
  const [focusMode, setFocusMode] = useState(false);
  const chrome = useMemo<ShellChrome>(() => ({ focusMode, setFocusMode }), [focusMode]);
  const [palette, setPalette] = useState(false);
  const [sheet, setSheet] = useState(false);
  const openSheet = () => {
    setPalette(false);
    setSheet(true);
  };
  const closeOverlays = useCallback(() => {
    setPalette(false);
    setSheet(false);
  }, []);
  const signOut = () => {
    void logout();
  };
  useRecentScreens(sections);

  return (
    <TooltipProvider delayDuration={400}>
      <ShellChromeContext value={chrome}>
        {/* `--sticky-top`: the top bar's height (h-14), for sticky page parts (11 §4); 0
          while focus mode hides the bar. */}
        <SidebarProvider
          open={focusMode ? false : sidebarOpen}
          onOpenChange={(open) => {
            if (focusMode) setFocusMode(false);
            storeSidebarOpen(open);
            setSidebarOpen(open);
          }}
          className={focusMode ? "[--sticky-top:0px]" : "[--sticky-top:3.5rem]"}
          data-focus-mode={focusMode || undefined}
        >
          <a
            href="#main"
            className="fixed top-2 left-2 z-50 -translate-y-16 rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground shadow-2 focus-visible:translate-y-0"
          >
            {t("skipToContent")}
          </a>
          <AppSidebar
            sections={badged}
            loading={canRead && types.isPending}
            siteTitle={settings.data?.["site.title"] || tc("app.name")}
            siteUrl={site.data?.site_url ?? null}
            hidden={focusMode}
          />
          <SidebarInset className="min-w-0">
            <TopBar
              hidden={focusMode}
              me={me}
              discouraged={settings.data?.["reading.discourage_indexing"] === true}
              onPalette={() => setPalette(true)}
              onShortcuts={openSheet}
              onSignOut={signOut}
            />
            <main id="main" className="flex min-w-0 flex-1 flex-col px-page pt-6 pb-12">
              <RouteTransition>
                <Outlet />
              </RouteTransition>
            </main>
          </SidebarInset>
          <ShellShortcuts
            sections={sections}
            canCreate={me.capabilities.includes("edit_own")}
            togglePalette={() => setPalette((open) => !open)}
            openSheet={openSheet}
            closeOverlays={closeOverlays}
          />
          <CommandPalette
            open={palette}
            onOpenChange={setPalette}
            me={me}
            sections={sections}
            onShortcuts={openSheet}
          />
          <ShortcutSheet open={sheet} onOpenChange={setSheet} />
          <MediaPickerHost />
          {/* Toasts sit above a floating panel that sets `--toast-lift` (the media
              library's upload tray), never on top of it. */}
          <Toaster
            position="bottom-right"
            offset={{ bottom: "calc(24px + var(--toast-lift, 0px))" }}
            mobileOffset={{ bottom: "calc(16px + var(--toast-lift, 0px))" }}
          />
        </SidebarProvider>
      </ShellChromeContext>
    </TooltipProvider>
  );
}

/**
 * Feeds the palette's Recent group: every screen of the nav the viewer lands on, however
 * they got there (sidebar, chord, palette, link). Items are pushed by whoever opens them.
 */
function useRecentScreens(sections: NavSection[]): void {
  const leaf = useMatches({ select: (ms) => ms.at(-1)?.pathname ?? "" });
  // The viewer is already set: the `_app` guard (`enterShell`) sets it before rendering.
  useEffect(() => {
    const item = navItemAt(sections, leaf);
    if (item) pushRecent({ kind: "screen", id: item.id });
  }, [leaf, sections]);
}

/** The shell's global bindings (11 §6): only those whose screens exist. */
function ShellShortcuts({
  sections,
  canCreate,
  togglePalette,
  openSheet,
  closeOverlays,
}: {
  sections: NavSection[];
  canCreate: boolean;
  togglePalette: () => void;
  openSheet: () => void;
  closeOverlays: () => void;
}) {
  const { t } = useTranslation("shell");
  const { toggleSidebar } = useSidebar();
  const navigate = useNavigate();
  const general = t("shortcuts.group.general");
  const goTo = t("shortcuts.group.goTo");

  useShortcut({
    id: "shell.palette",
    keys: "mod+k",
    label: t("shortcuts.palette"),
    group: general,
    allowInInput: true,
    run: togglePalette,
  });
  useShortcut({
    id: "shell.sheet",
    keys: "?",
    label: t("shortcuts.sheet"),
    group: general,
    run: openSheet,
  });
  // Not inside text fields or the editor, where `mod+b` is bold (11 §7).
  useShortcut({
    id: "shell.sidebar",
    keys: "mod+b",
    label: t("shortcuts.toggleSidebar"),
    group: general,
    run: toggleSidebar,
  });

  // `c p` / `c g`: a new post / page in the editor (only with `edit_own`).
  useEffect(() => {
    if (!canCreate) return;
    const offs = (
      [
        ["c p", "post", t("topBar.newPost")],
        ["c g", "page", t("topBar.newPage")],
      ] as const
    ).map(([keys, type, label]) =>
      registerShortcut({
        id: `create.${type}`,
        keys,
        label,
        group: general,
        run: () => {
          closeOverlays();
          void navigate({ to: "/content/$type/new", params: { type } });
        },
      }),
    );
    return () => offs.forEach((off) => off());
  }, [canCreate, general, t, navigate, closeOverlays]);

  // `g <key>` for every nav item that has one.
  useEffect(() => {
    const offs = sections
      .flatMap((s) => s.items)
      .flatMap((item) =>
        item.shortcut
          ? [
              registerShortcut({
                id: `nav.${item.id}`,
                keys: item.shortcut,
                label: t("shortcuts.goTo", { label: item.label }),
                group: goTo,
                run: () => {
                  closeOverlays();
                  void navigate({ to: item.to as "/" });
                },
              }),
            ]
          : [],
      );
    return () => offs.forEach((off) => off());
  }, [sections, goTo, t, navigate, closeOverlays]);

  return null;
}
