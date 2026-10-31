// The ⌘K command palette (plan Task 11; docs/design/11-admin-ui.md §7): a shadcn
// `CommandDialog` opened by `mod+k` and the top bar's trigger. Groups: Recent (the
// viewer's last 8 screens and items, only before anything is typed), Go to (every nav
// item and the `?` sheet), Create (one per content type the nav shows, with
// `edit_own`, Upload media with `upload_media`, New content type when the nav has
// Content types, New taxonomy when it has Taxonomies and the viewer has
// `manage_settings`, New menu when it has Menus and New redirect when it has
// Redirects), Content (live search, from 2 characters) and Preferences (theme, density).
// cmdk's own filter ranks the rows inside the static groups; content hits come from the
// server already matched, so they keep the server's order, in the last group.

import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMatches, useNavigate } from "@tanstack/react-router";
import { Command as CommandPrimitive, defaultFilter, useCommandState } from "cmdk";
import { cn } from "cn";
import {
  BlocksIcon,
  FilePlusIcon,
  FilesIcon,
  KeyboardIcon,
  ListTreeIcon,
  LoaderCircleIcon,
  MonitorIcon,
  MoonIcon,
  Rows3Icon,
  Rows4Icon,
  SearchIcon,
  SearchXIcon,
  ShapesIcon,
  SignpostIcon,
  SunIcon,
  TagsIcon,
  UploadIcon,
  UserRoundIcon,
  UserPlusIcon,
  XIcon,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { isContentStatus } from "@/api/content";
import { qk } from "@/api/keys";
import type { MeDto } from "@/auth/session";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { StatusDot } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandDialog,
  CommandGroup,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";
import { DialogClose } from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { useDensity, type Density } from "@/design/density";
import { useTheme, type ThemePreference } from "@/design/theme";
import { requestUpload } from "@/media/upload-intent";
import { navItemAt, type NavItem, type NavSection } from "@/shell/nav";
import { listRecent, pushRecent, recentId, type RecentEntry } from "./recent";
import { bodySnippet, queryWords, searchContent, searchable, type PaletteHit } from "./sources";

export interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  me: MeDto;
  /** The shell's nav (`navFor`): Go to, Create and the searched types follow it. */
  sections: NavSection[];
  /** Opens the `?` sheet (the palette closes first). */
  onShortcuts: () => void;
}

/** Content hits' cmdk values start with this; the filter ranks them itself. */
const CONTENT_VALUE = "content:";
/**
 * The rank of a content hit (cmdk scores 0..1). The server matched it already, so it
 * stays whatever is typed; a strong static match (a screen's name) still sorts above.
 */
const CONTENT_SCORE = 0.5;
/** A search slower than this shows its loading state; faster ones never flash it. */
const SLOW_MS = 250;

function paletteFilter(value: string, search: string, keywords?: string[]): number {
  return value.startsWith(CONTENT_VALUE) ? CONTENT_SCORE : defaultFilter(value, search, keywords);
}

export function CommandPalette({
  open,
  onOpenChange,
  me,
  sections,
  onShortcuts,
}: CommandPaletteProps) {
  const { t } = useTranslation("palette");
  // Every opening starts afresh (empty query, Recent read again), even one that lands
  // while the previous closing animation still holds the old content mounted.
  const [openings, setOpenings] = useState(0);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setOpenings((n) => n + 1);
  }
  return (
    <CommandDialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("title")}
      description={t("description")}
      // Pinned near the top so the frame never jumps while results change (Linear,
      // Raycast); a fixed list height does the rest. Phones get a near full-screen sheet.
      className={cn(
        "top-[12vh] flex flex-col gap-0 border bg-popover shadow-3 ring-0 sm:max-w-[40rem]",
        "duration-(--dur-fast) ease-out data-open:slide-in-from-top-1 data-open:zoom-in-[0.98]! data-closed:zoom-out-[0.98]! data-closed:duration-(--dur-instant)",
        "max-sm:top-2 max-sm:h-[calc(100dvh-1rem)] max-sm:max-w-[calc(100%-1rem)]",
      )}
    >
      <PaletteBody
        key={openings}
        me={me}
        sections={sections}
        close={() => onOpenChange(false)}
        onShortcuts={onShortcuts}
      />
    </CommandDialog>
  );
}

interface BodyProps {
  me: MeDto;
  sections: NavSection[];
  close: () => void;
  onShortcuts: () => void;
}

/** One opening of the palette (keyed by the opening, so its state never carries over). */
function PaletteBody({ me, sections, close, onShortcuts }: BodyProps) {
  const { t } = useTranslation("palette");
  const navigate = useNavigate();
  const [q, setQ] = useState("");
  // Read once per opening; the list only changes by navigating, which closes it.
  const [recent] = useState(listRecent);
  const here = useMatches({ select: (ms) => ms.at(-1)?.pathname ?? "" });

  const navItems = useMemo(() => sections.flatMap((s) => s.items), [sections]);
  const typeItems = useMemo(() => navItems.filter((i) => i.contentType), [navItems]);
  const types = useMemo(() => typeItems.flatMap((i) => i.contentType ?? []), [typeItems]);
  const typeItem = (type: string) => typeItems.find((i) => i.contentType === type);
  const createTypes = me.capabilities.includes("edit_own") ? typeItems : [];
  // "Upload media" (Create): the Media screen is in the nav and the viewer may upload.
  const mediaItem = me.capabilities.includes("upload_media")
    ? navItems.find((i) => i.id === "media")
    : undefined;
  // "New content type" (Create): the nav shows Content types (`manage_settings`).
  const typesItem = navItems.find((i) => i.id === "types");
  // "New taxonomy" (Create): `manage_terms` alone sees Taxonomies but cannot create one.
  const taxonomiesItem = me.capabilities.includes("manage_settings")
    ? navItems.find((i) => i.id === "taxonomies")
    : undefined;
  // "New menu" (Create): the nav shows Menus (`manage_menus`).
  const menusItem = navItems.find((i) => i.id === "menus");
  // "New redirect" (Create): the nav shows Redirects (`manage_redirects`).
  const redirectsItem = navItems.find((i) => i.id === "redirects");
  // "New user" (Create): the nav shows Users (`manage_users`).
  const usersItem = navItems.find((i) => i.id === "users");

  const query = q.trim();
  const active = searchable(query) && types.length > 0;
  const qc = useQueryClient();
  const search = useQuery({
    queryKey: qk.paletteSearch(query, types),
    // A type deleted meanwhile: the sidebar's types are read again.
    queryFn: ({ signal }) =>
      searchContent(query, types, signal, () => void qc.invalidateQueries({ queryKey: qk.types })),
    enabled: active,
    // The last answer stays up while the next loads, so results never blink out.
    placeholderData: keepPreviousData,
    // The footer and the empty state say it failed; no toast on top (review m2).
    meta: INLINE_ERRORS,
  });
  const hits = active ? (search.data ?? []) : [];
  const pending = active && search.isFetching;
  const slow = useSlow(pending, query);

  const go = (to: string, entry?: RecentEntry) => {
    if (entry) pushRecent(entry);
    close();
    void navigate({ to: to as "/" });
  };
  const openHit = (hit: Pick<PaletteHit, "type" | "id" | "title" | "status">) => {
    const { type, id, title, status } = hit;
    go(`/content/${type}/${id}`, { kind: "item", type, id, title, status });
  };

  const current = navItemAt(sections, here);
  const recentRows = recent.flatMap((entry): { entry: RecentEntry; nav?: NavItem }[] => {
    if (entry.kind === "item") return [{ entry }];
    const nav = navItems.find((i) => i.id === entry.id);
    // A screen the nav no longer shows (or the one already open) is left out.
    return nav && nav.id !== current?.id ? [{ entry, nav }] : [];
  });

  const status = search.isError
    ? t("status.failed")
    : slow
      ? t("status.searching")
      : active && !pending
        ? t("status.results", { count: hits.length })
        : "";

  return (
    <Command
      label={t("title")}
      filter={paletteFilter}
      loop
      // cmdk's Ctrl+K/J/N/P list moves would swallow `mod+k` (closing) on Windows/Linux.
      vimBindings={false}
      className="min-h-0 flex-1 rounded-none! bg-transparent p-0"
    >
      <div
        data-slot="palette-header"
        className="flex h-14 shrink-0 items-center gap-3 border-b px-4 max-sm:pr-2"
      >
        {/* A slow search turns the field's icon into a spinner (with reduced motion it
            stands still; its shape still differs): the cue for rows that are about to
            change, which keep their full colour (P81). */}
        {slow ? (
          <LoaderCircleIcon
            aria-hidden="true"
            data-slot="palette-spinner"
            className="size-4.5 shrink-0 animate-spin text-muted-foreground motion-reduce:animate-none"
          />
        ) : (
          <SearchIcon aria-hidden="true" className="size-4.5 shrink-0 text-muted-foreground" />
        )}
        {/* No focus ring or fill: the open dialog is the focused context (Linear,
            Raycast); the active row's highlight carries the keyboard position. */}
        <CommandPrimitive.Input
          data-slot="palette-input"
          value={q}
          onValueChange={setQ}
          // Radix focuses it on a normal open; this covers a reopening that lands while
          // the closing animation still holds the dialog (its body is keyed anew).
          autoFocus
          placeholder={t("placeholder")}
          className="h-full min-w-0 flex-1 bg-transparent text-base text-foreground outline-none placeholder:text-muted-foreground focus-visible:outline-none sm:text-[0.9375rem]"
        />
        {q ? (
          <Button
            variant="ghost"
            size="icon-sm"
            className="text-muted-foreground hover:text-foreground"
            aria-label={t("clear")}
            onClick={() => setQ("")}
          >
            <XIcon />
          </Button>
        ) : null}
        <span aria-hidden="true" className="max-sm:hidden">
          <KbdCombo keys="esc" />
        </span>
        <DialogClose asChild>
          <Button variant="ghost" className="sm:hidden">
            {t("cancel")}
          </Button>
        </DialogClose>
      </div>

      <div className="relative flex min-h-0 flex-1 flex-col sm:flex-none">
        <CommandList
          data-slot="palette-list"
          className="max-h-none min-h-0 flex-1 scroll-py-2 overscroll-contain p-2 sm:h-[min(26rem,calc(76svh-7rem))] sm:flex-none"
        >
          {query === "" && recentRows.length > 0 ? (
            <CommandGroup heading={t("group.recent")}>
              {recentRows.map(({ entry, nav }) =>
                entry.kind === "item" ? (
                  <HitRow
                    key={recentId(entry)}
                    value={`recent ${recentId(entry)}`}
                    hit={entry}
                    typeItem={typeItem(entry.type)}
                    onSelect={() => openHit(entry)}
                  />
                ) : nav ? (
                  <Row
                    key={recentId(entry)}
                    value={`recent ${recentId(entry)}`}
                    icon={nav.icon}
                    label={nav.label}
                    hint={t("hit.screen")}
                    onSelect={() => go(nav.to)}
                  />
                ) : null,
              )}
            </CommandGroup>
          ) : null}

          <CommandGroup heading={t("group.goTo")}>
            {navItems.map((item) => (
              <Row
                key={item.id}
                value={item.label}
                keywords={[t("goTo.keywords"), item.id]}
                icon={item.icon}
                label={item.label}
                shortcut={item.shortcut}
                onSelect={() => go(item.to)}
              />
            ))}
            <Row
              value={t("goTo.account")}
              keywords={[t("goTo.accountKeywords")]}
              icon={UserRoundIcon}
              label={t("goTo.account")}
              onSelect={() => go("/account")}
            />
            <Row
              value={t("goTo.shortcuts")}
              keywords={["help", "keys", "?"]}
              icon={KeyboardIcon}
              label={t("goTo.shortcuts")}
              shortcut="?"
              onSelect={() => {
                close();
                onShortcuts();
              }}
            />
          </CommandGroup>

          {createTypes.length > 0 ||
          mediaItem ||
          typesItem ||
          taxonomiesItem ||
          menusItem ||
          redirectsItem ||
          usersItem ? (
            <CommandGroup heading={t("group.create")}>
              {createTypes.map((item) => {
                const type = item.contentType ?? "";
                const label =
                  type === "post"
                    ? t("create.post")
                    : type === "page"
                      ? t("create.page")
                      : t("create.other", { label: item.label });
                return (
                  <Row
                    key={item.id}
                    value={label}
                    keywords={[t("create.keywords"), type]}
                    icon={type === "post" ? FilePlusIcon : type === "page" ? FilesIcon : ShapesIcon}
                    label={label}
                    onSelect={() => go(`/content/${type}/new`)}
                  />
                );
              })}
              {mediaItem ? (
                <Row
                  value={t("create.upload")}
                  keywords={[t("create.uploadKeywords")]}
                  icon={UploadIcon}
                  label={t("create.upload")}
                  onSelect={() => {
                    // The library opens its file chooser on arrival (`upload-intent.ts`).
                    requestUpload();
                    go(mediaItem.to);
                  }}
                />
              ) : null}
              {typesItem ? (
                <Row
                  value={t("create.type")}
                  keywords={[t("create.typeKeywords")]}
                  icon={BlocksIcon}
                  label={t("create.type")}
                  onSelect={() => {
                    close();
                    void navigate({ to: "/structure/types", search: { new: true } });
                  }}
                />
              ) : null}
              {taxonomiesItem ? (
                <Row
                  value={t("create.taxonomy")}
                  keywords={[t("create.taxonomyKeywords")]}
                  icon={TagsIcon}
                  label={t("create.taxonomy")}
                  onSelect={() => {
                    close();
                    void navigate({ to: "/structure/taxonomies", search: { new: true } });
                  }}
                />
              ) : null}
              {menusItem ? (
                <Row
                  value={t("create.menu")}
                  keywords={[t("create.menuKeywords")]}
                  icon={ListTreeIcon}
                  label={t("create.menu")}
                  onSelect={() => {
                    close();
                    void navigate({ to: "/structure/menus", search: { new: true } });
                  }}
                />
              ) : null}
              {redirectsItem ? (
                <Row
                  value={t("create.redirect")}
                  keywords={[t("create.redirectKeywords")]}
                  icon={SignpostIcon}
                  label={t("create.redirect")}
                  onSelect={() => {
                    close();
                    void navigate({ to: "/structure/redirects", search: { new: true } });
                  }}
                />
              ) : null}
              {usersItem ? (
                <Row
                  value={t("create.user")}
                  keywords={[t("create.userKeywords")]}
                  icon={UserPlusIcon}
                  label={t("create.user")}
                  onSelect={() => {
                    close();
                    void navigate({ to: "/users", search: { new: true } });
                  }}
                />
              ) : null}
            </CommandGroup>
          ) : null}

          <PreferenceRows close={close} />

          {/* Last: groups stay in render order (cmdk 1.1.1 sorts the rows inside a group;
              its group sort looks groups up by id in `data-value`, which holds the
              heading, so it never moves one), and the hits are the open-ended part. */}
          {/* Stale hits (the previous query's) are not dimmed: at 60 % muted text fell
              below 4.5:1. The spinner and `aria-busy` on the hits only (not the whole
              listbox, so Go to, Create and Preferences are read as usual) say a search
              runs (P81). */}
          <div data-slot="palette-content" aria-busy={slow || undefined}>
            {hits.length > 0 ? (
              <CommandGroup heading={t("group.content")}>
                {hits.map((hit) => (
                  <HitRow
                    key={`${hit.type}:${hit.id}`}
                    value={`${CONTENT_VALUE}${hit.type}:${hit.id}`}
                    hit={hit}
                    slug={hit.slug}
                    query={query}
                    snippet={bodySnippet(hit, query)}
                    typeItem={typeItem(hit.type)}
                    // One type only: the icon and the heading say it; a chip per row is noise.
                    showType={new Set(hits.map((h) => h.type)).size > 1}
                    onSelect={() => openHit(hit)}
                  />
                ))}
              </CommandGroup>
            ) : slow ? (
              <SearchingRows />
            ) : null}
          </div>
        </CommandList>
        {pending ? null : <EmptyState q={query} failed={search.isError} />}
      </div>

      <div
        data-slot="palette-footer"
        className="flex h-10 shrink-0 items-center gap-4 border-t px-4 text-xs text-muted-foreground max-sm:sr-only"
      >
        <span role="status" className="mr-auto truncate">
          {status}
        </span>
        <FooterHint label={t("footer.move")}>
          <KbdCombo keys="up" />
          <KbdCombo keys="down" />
        </FooterHint>
        <FooterHint label={t("footer.open")}>
          <KbdCombo keys="enter" />
        </FooterHint>
        <FooterHint label={t("footer.close")}>
          <KbdCombo keys="esc" />
        </FooterHint>
      </div>
    </Command>
  );
}

/**
 * True once a search for `key` has been pending for `SLOW_MS`. Keyed by the query, so a
 * new keystroke starts a new wait without resetting state inside an effect.
 */
function useSlow(pending: boolean, key: string): boolean {
  const [slowKey, setSlowKey] = useState<string | null>(null);
  useEffect(() => {
    if (!pending) return;
    const timer = setTimeout(() => setSlowKey(key), SLOW_MS);
    return () => clearTimeout(timer);
  }, [pending, key]);
  return pending && slowKey === key;
}

function FooterHint({ label, children }: { label: string; children: ReactNode }) {
  return (
    <span aria-hidden="true" className="flex shrink-0 items-center gap-1.5">
      <span className="flex items-center gap-0.5">{children}</span>
      {label}
    </span>
  );
}

const ROW =
  "h-10 gap-3 rounded-lg px-2 before:absolute before:inset-y-2.5 before:left-0 before:w-0.5 before:rounded-full before:bg-primary before:opacity-0 data-selected:before:opacity-100 pointer-coarse:h-12";

/** The row's icon in a small tile; ember while the row is active. */
function Tile({ icon: Icon }: { icon: LucideIcon }) {
  return (
    <span
      aria-hidden="true"
      className="flex size-7 shrink-0 items-center justify-center rounded-md border bg-background text-muted-foreground shadow-1 transition-colors group-data-selected/command-item:border-primary/35 group-data-selected/command-item:text-primary"
    >
      <Icon className="size-3.5" />
    </span>
  );
}

interface RowProps {
  value: string;
  keywords?: string[];
  icon: LucideIcon;
  label: string;
  /** A `shortcuts.ts` key string shown as keycaps. */
  shortcut?: string | undefined;
  /** Muted text at the end of the row. */
  hint?: string;
  checked?: boolean;
  onSelect: () => void;
}

function Row({ value, keywords, icon, label, shortcut, hint, checked, onSelect }: RowProps) {
  const { t } = useTranslation("palette");
  return (
    <CommandItem
      value={value}
      {...(keywords ? { keywords } : {})}
      onSelect={onSelect}
      data-checked={checked ? "true" : undefined}
      className={ROW}
    >
      <Tile icon={icon} />
      <span className="min-w-0 flex-1 truncate" title={label}>
        {label}
      </span>
      {checked ? <span className="sr-only">{t("preferences.current")}</span> : null}
      {shortcut ? (
        <CommandShortcut className="tracking-normal group-data-selected/command-item:**:data-[slot=kbd]:bg-background">
          <KbdCombo keys={shortcut} />
        </CommandShortcut>
      ) : hint ? (
        <CommandShortcut className="tracking-normal">{hint}</CommandShortcut>
      ) : null}
    </CommandItem>
  );
}

interface HitRowProps {
  value: string;
  hit: Pick<PaletteHit, "type" | "id" | "title" | "status">;
  slug?: string;
  /** The search, whose words are marked in the title. */
  query?: string;
  /** A hit matched only in its text: the excerpt's start (`bodySnippet`). */
  snippet?: string | null;
  typeItem: NavItem | undefined;
  showType?: boolean;
  onSelect: () => void;
}

/** A content item: title (slug), status dot and label, and its type. */
function HitRow({
  value,
  hit,
  slug,
  query = "",
  snippet = null,
  typeItem,
  showType = true,
  onSelect,
}: HitRowProps) {
  const { t } = useTranslation("palette");
  const { t: tc } = useTranslation();
  const title = hit.title.trim() || t("hit.untitled");
  const status = isContentStatus(hit.status) ? hit.status : null;
  return (
    <CommandItem
      value={value}
      onSelect={onSelect}
      className={cn(ROW, snippet !== null && "h-auto min-h-10 py-1.5 pointer-coarse:h-auto")}
    >
      <Tile icon={typeItem?.icon ?? ShapesIcon} />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="flex min-w-0 items-baseline gap-2">
          <span className="truncate" title={title}>
            <Highlight text={title} query={query} />
          </span>
          {slug ? (
            <span
              className="min-w-0 shrink truncate font-mono text-xs text-muted-foreground max-sm:hidden"
              title={`/${slug}`}
            >
              /{slug}
            </span>
          ) : null}
        </span>
        {snippet !== null ? (
          <span
            data-slot="palette-snippet"
            // Phones: the label on its own line and up to two lines of the text (no hover
            // for the `title`); wider: one truncated line.
            className="text-xs text-muted-foreground max-sm:flex max-sm:flex-col sm:truncate"
            title={snippet || undefined}
          >
            <span className="font-medium">{t("hit.inText")}</span>
            {snippet ? (
              <span className="max-sm:line-clamp-2">
                <span className="max-sm:hidden"> · </span>
                {snippet}
              </span>
            ) : null}
          </span>
        ) : null}
      </span>
      <CommandShortcut className="flex shrink-0 items-center gap-2 tracking-normal">
        {status ? (
          <span className="flex items-center gap-1.5">
            <StatusDot status={status} />
            <span>{tc(`status.${status}`)}</span>
          </span>
        ) : null}
        {typeItem && showType ? (
          <span className="rounded-sm border px-1.5 py-px text-[0.6875rem] leading-4 max-sm:hidden">
            {typeItem.label}
          </span>
        ) : null}
      </CommandShortcut>
    </CommandItem>
  );
}

/**
 * `text` with every word of `query` (2+ characters, any case) marked, so it shows why a
 * title matched. A hit matched only in its body has nothing marked.
 */
function Highlight({ text, query }: { text: string; query: string }) {
  const words = queryWords(query);
  if (words.length === 0) return text;
  const parts = text.split(new RegExp(`(${words.join("|")})`, "gi"));
  return parts.map((part, i) =>
    i % 2 === 1 ? (
      <mark
        key={i}
        className="rounded-xs bg-transparent font-semibold text-foreground underline decoration-primary/70 decoration-2 underline-offset-3"
      >
        {part}
      </mark>
    ) : (
      part
    ),
  );
}

/** Placeholder rows while a slow first search runs (decorative; the footer says it). */
function SearchingRows() {
  const { t } = useTranslation("palette");
  return (
    <div data-slot="palette-searching" aria-hidden="true" className="p-1">
      <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">
        {t("group.content")}
      </div>
      {[0.62, 0.48, 0.55].map((w, i) => (
        <div key={i} className="flex h-10 items-center gap-3 px-2">
          <Skeleton className="size-7 rounded-md" />
          <Skeleton className="h-3 rounded-sm" style={{ width: `${w * 100}%` }} />
        </div>
      ))}
    </div>
  );
}

/**
 * Shown over the list when nothing matches. Outside the listbox (not `CommandEmpty`):
 * a listbox holding text but no option fails axe `aria-required-children`.
 */
function EmptyState({ q, failed }: { q: string; failed: boolean }) {
  const { t } = useTranslation("palette");
  const empty = useCommandState((s) => s.search !== "" && s.filtered.count === 0);
  if (!empty) return null;
  return (
    <div
      data-slot="palette-empty"
      className="absolute inset-0 flex flex-col items-center justify-center gap-3 px-6 text-center"
    >
      <span
        aria-hidden="true"
        className="flex size-10 items-center justify-center rounded-full border bg-muted text-muted-foreground"
      >
        <SearchXIcon className="size-4.5" />
      </span>
      <div className="flex max-w-sm flex-col gap-1">
        <p className="font-medium text-foreground">{t("empty.title", { q })}</p>
        <p className="text-sm text-muted-foreground">
          {failed ? t("status.failed") : t("empty.body")}
        </p>
      </div>
    </div>
  );
}

const THEMES: { value: ThemePreference; icon: LucideIcon }[] = [
  { value: "light", icon: SunIcon },
  { value: "dark", icon: MoonIcon },
  { value: "system", icon: MonitorIcon },
];
const DENSITIES: { value: Density; icon: LucideIcon }[] = [
  { value: "comfortable", icon: Rows3Icon },
  { value: "compact", icon: Rows4Icon },
];

function PreferenceRows({ close }: { close: () => void }) {
  const { t } = useTranslation("palette");
  const { theme, setTheme } = useTheme();
  const { density, setDensity } = useDensity();
  return (
    <CommandGroup heading={t("group.preferences")}>
      {THEMES.map(({ value, icon }) => (
        <Row
          key={value}
          value={t(`preferences.${value}`)}
          keywords={[t("preferences.themeKeywords")]}
          icon={icon}
          label={t(`preferences.${value}`)}
          checked={theme === value}
          onSelect={() => {
            setTheme(value);
            close();
          }}
        />
      ))}
      {DENSITIES.map(({ value, icon }) => (
        <Row
          key={value}
          value={t(`preferences.${value}`)}
          keywords={[t("preferences.densityKeywords")]}
          icon={icon}
          label={t(`preferences.${value}`)}
          checked={density === value}
          onSelect={() => {
            setDensity(value);
            close();
          }}
        />
      ))}
    </CommandGroup>
  );
}
