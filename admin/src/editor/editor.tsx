// The writing surface (11 §1, after Ghost): a calm, centred 720 px column; the title as
// an auto-growing field in Inter's display cut; the body at 17/28; a selection bubble and
// a `/` insert menu; the word count and reading time at the foot; a focus mode
// (`mod+shift+f`) that hides the shell. A thin bar above the column carries the way
// back, the status, the save state, the panel toggle (`mod+.`) and Save (`mod+s`). The
// side panel (`panel/`) holds status and publishing (the publish dialog, `mod+shift+p`),
// the address, excerpt, terms, parent and the danger zone. Read-only items get a banner
// and no way to save or change anything (Review Focus 3).
//
// Editor safety (Task 16, Review Focus 3): the save indicator shows the autosave slot as
// well as the item; a restored draft opens with the restore banner; a `412` opens the
// conflict dialog (and a transition that waited for the save says it did not run);
// leaving with unsaved changes asks (Save / Discard / Stay); a `401` keeps the draft in
// `pendingDrafts` and goes to sign in, and the screen takes it back afterwards.

import "@/styles/editor.css";
import { Link } from "@tanstack/react-router";
import { Selection } from "@tiptap/pm/state";
import { EditorContent, type Editor } from "@tiptap/react";
import {
  ArrowLeftIcon,
  MaximizeIcon,
  MinimizeIcon,
  PanelRightIcon,
  ShieldAlertIcon,
} from "lucide-react";
import { cn } from "cn";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { reportApiError } from "@/api/client";
import { metaQuery } from "@/api/meta";
import { deleteAutosave, getContent, isContentStatus, putAutosave } from "@/api/content";
import { qk } from "@/api/keys";
import type { MeDto } from "@/auth/session";
import { ApiError, isProblem } from "@/api/problem";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { SaveState } from "@/components/kit/save-state";
import { StatusBadge } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { storeEditorPanelOpen, storedEditorPanelOpen } from "@/design/preferences";
import { uiLocale } from "@/lib/locale";
import { useShortcut } from "@/lib/shortcuts";
import { useShellChrome } from "@/shell/chrome-context";
import { type Draft, newDraftKey, pendingDrafts } from "./autosave";
import { EditorBubbleMenu } from "./bubble-menu";
import { onPreconditionFailed } from "./conflict";
import { ConflictDialog, type OpenConflict } from "./conflict-dialog";
import {
  type ContentDto,
  type EditorDocument,
  type Handoff,
  type ReadOnlyReason,
  type RestoreSource,
  type WriteOp,
  autosaveInput,
  useEditorDocument,
} from "./editor-state";
import { LeaveDialog } from "./leave-dialog";
import { useLeaveGuard } from "./leave-guard";
import { RestoreBanner } from "./restore-banner";
import { useTermsById, useTypeTaxonomies, useWide } from "./panel/panel-state";
import { PublishDialog, type PublishMode } from "./panel/publish-dialog";
import { PublishBar } from "./panel/publish-bar";
import { PANEL_ID, SidePanel } from "./panel/side-panel";
import { blockText } from "./text";
import { WordStats } from "./word-stats";

/** What the screen reopens with (the route remounts it): Discard, Take theirs, Restore. */
export interface Reopen {
  /** The item to show (`null`: a new, empty one). */
  item: ContentDto | null;
  /** A working draft to open with (the restore banner says so). */
  restore?: RestoreSource;
  /** The user's own version, kept in the slot after Take theirs (the banner offers it). */
  kept?: Draft;
}

export interface EditorScreenProps {
  type: string;
  /** The type's label (`Posts`), for the way back. */
  typeLabel: string;
  /** `null` for a new item (created on the first save). */
  item: ContentDto | null;
  handoff?: Handoff | null;
  /** A working draft to open with instead of the item (the autosave slot, a kept draft). */
  restore?: RestoreSource | null;
  /** After Take theirs: the user's version, kept in the slot. */
  kept?: Draft | null;
  /** After a new item's first save (the route moves to its `/$id`). */
  onCreated?: (item: ContentDto, handoff: Handoff) => void;
  /** Remounts the screen with another starting point (see `Reopen`). */
  onReopen?: (next: Reopen) => void;
}

export function EditorScreen({
  type,
  typeLabel,
  item,
  handoff,
  restore,
  kept: keptInitially,
  onCreated,
  onReopen,
}: EditorScreenProps) {
  const { t } = useTranslation("editor");
  const queryClient = useQueryClient();
  const [conflict, setConflict] = useState<OpenConflict | null>(null);
  const [publishMode, setPublishMode] = useState<PublishMode | null>(null);
  const doc: EditorDocument = useEditorDocument({
    type,
    item,
    handoff: handoff ?? null,
    restore: restore ?? null,
    placeholder: t("body.placeholder"),
    bodyLabel: t("body.label"),
    ...(onCreated ? { onCreated } : {}),
    onConflict: (e, op, context) => {
      // The user's text stays in the editor whatever happens next (Review Focus 3).
      const then = context.then ?? (op === "save" ? undefined : op);
      const state = onPreconditionFailed(e, doc.draft());
      setPublishMode(null);
      if (!state) {
        toast.error(t("save.conflict"), { id: "editor-save" });
        return;
      }
      if (state.fields.length === 0) {
        // Nothing of the draft differs from the newer version: take it as the base.
        doc.rebase(state.theirs);
        toast.info(then ? t("conflict.sameThen") : t("conflict.same"), { id: "editor-save" });
        return;
      }
      setConflict({ ...state, ...(then ? { then } : {}) });
    },
    onUnauthenticated: (e, draft) => {
      // Kept in memory for after the sign-in (never lost, F37); the global handler then
      // goes to `/login?next=<this screen>` through the router, past the leave guard.
      // Whose draft it is (read now: the guard has always loaded the account).
      const me = queryClient.getQueryData<MeDto>(qk.me);
      if (me) {
        pendingDrafts.set(item ? item.id : newDraftKey(type), {
          userId: me.id,
          draft,
          at: new Date().toISOString(),
        });
      }
      reportApiError(e);
    },
    onWriteError: (e, op: WriteOp) => {
      const detail =
        e instanceof ApiError
          ? e.problem.errors?.[0]?.detail || e.problem.detail || e.problem.title
          : null;
      const key = op === "save" ? ("save" as const) : (`transition.${op}` as const);
      toast.error(detail ? t(`${key}.failedWith`, { detail }) : t(`${key}.failed`), {
        id: "editor-save",
      });
    },
  });
  const { focusMode, setFocusMode } = useShellChrome();
  const group = t("shortcuts.group");

  // The side panel: beside the text from 1024 px (open unless closed before), else a
  // sheet (closed until asked for). Focus mode hides it.
  const wide = useWide();
  const [wideOpen, setWideOpen] = useState(storedEditorPanelOpen);
  const [sheetOpen, setSheetOpen] = useState(false);
  const panelOpen = wide ? wideOpen && !focusMode : sheetOpen;
  const toggleRef = useRef<HTMLButtonElement>(null);
  const setPanelOpen = useCallback(
    (open: boolean) => {
      if (wide) {
        if (open && focusMode) setFocusMode(false);
        setWideOpen(open);
        storeEditorPanelOpen(open);
        if (!open) toggleRef.current?.focus();
      } else setSheetOpen(open);
    },
    [wide, focusMode, setFocusMode],
  );
  const canPublish =
    doc.readOnly === null &&
    doc.item !== null &&
    (doc.item.allowed.publish || doc.item.allowed.schedule);
  const taxonomies = useTypeTaxonomies(type, publishMode !== null);
  const terms = useTermsById(publishMode !== null ? taxonomies : []);
  const site = useQuery({ ...metaQuery, enabled: publishMode !== null });

  useShortcut({
    id: "editor.panel",
    keys: "mod+.",
    label: t("shortcuts.panel"),
    group,
    allowInInput: true,
    run: () => setPanelOpen(!panelOpen),
  });
  useShortcut({
    id: "editor.publish",
    keys: "mod+shift+p",
    label: t("shortcuts.publish"),
    group,
    allowInInput: true,
    when: () =>
      (canPublish || (doc.item === null && doc.readOnly === null)) &&
      document.querySelector('[role="dialog"]') === null,
    run: () => {
      // A new item has no `allowed` yet: it is saved as a draft first.
      if (canPublish) setPublishMode("now");
      else toast.info(t("publish.saveFirst"), { id: "editor-transition" });
    },
  });
  useShortcut({
    id: "editor.panelClose",
    keys: "esc",
    label: t("shortcuts.panelClose"),
    group,
    allowInInput: true,
    // The wide panel only (the sheet closes itself), and not over a dialog or a menu.
    when: () =>
      wide &&
      panelOpen &&
      !focusMode &&
      document.querySelector(
        '[role="dialog"], [role="alertdialog"], [data-radix-popper-content-wrapper]',
      ) === null,
    run: () => setPanelOpen(false),
  });

  useShortcut({
    id: "editor.save",
    keys: "mod+s",
    label: t("shortcuts.save"),
    group,
    allowInInput: true,
    run: () => {
      void doc.save();
    },
  });
  useShortcut({
    id: "editor.focus",
    keys: "mod+shift+f",
    label: t("shortcuts.focus"),
    group,
    allowInInput: true,
    run: () => setFocusMode(!focusMode),
  });
  useShortcut({
    id: "editor.focusExit",
    keys: "esc",
    label: t("shortcuts.focusExit"),
    group,
    allowInInput: true,
    // Not while a dialog or sheet is open: its own Esc closes it (the HTML sheet).
    when: () => focusMode && document.querySelector('[role="dialog"]') === null,
    run: () => setFocusMode(false),
  });
  // Leaving the editor leaves focus mode.
  useEffect(() => () => setFocusMode(false), [setFocusMode]);

  // Leaving with changes the item does not hold asks first; closing the tab only while
  // the slot does not hold them either (F38).
  const guard = useLeaveGuard(doc.dirty && doc.readOnly === null, { unload: doc.unprotected });

  // The restore banner's state: the restored draft (from the document), or the user's own
  // version kept in the slot after Take theirs.
  const [kept, setKept] = useState<Draft | null>(keptInitially ?? null);
  const [bannerBusy, setBannerBusy] = useState(false);
  const [restoredShown, setRestoredShown] = useState(true);

  /** Drops the slot (Discard), with Undo putting `draft` back. */
  const discardSlot = async (draft: Draft, reopenWith: ContentDto | null) => {
    if (!doc.item) {
      // A new item has no slot: back to an empty screen.
      onReopen?.({ item: null });
      return;
    }
    const id = doc.item.id;
    setBannerBusy(true);
    try {
      await deleteAutosave(type, id);
      const fresh = reopenWith ?? (await getContent(type, id));
      toast.success(t("restore.discarded"), {
        id: "editor-restore",
        action: {
          label: t("restore.undo"),
          onClick: () => {
            void putAutosave(type, id, autosaveInput(draft))
              .then(() => getContent(type, id))
              .then((now) =>
                onReopen?.({ item: now, restore: { draft, at: new Date().toISOString() } }),
              )
              .catch(() => toast.error(t("restore.undoFailed"), { id: "editor-restore" }));
          },
        },
      });
      if (reopenWith) setKept(null);
      else onReopen?.({ item: fresh });
    } catch (e) {
      if (isProblem(e, "unauthenticated")) reportApiError(e);
      else toast.error(t("restore.discardFailed"), { id: "editor-restore" });
    } finally {
      setBannerBusy(false);
    }
  };

  const banner =
    doc.restored && restoredShown && doc.readOnly === null ? (
      <RestoreBanner
        mode="restored"
        at={doc.restored.at}
        stale={doc.restored.stale}
        busy={bannerBusy}
        onDiscard={() => {
          setRestoredShown(false);
          void discardSlot(doc.draft(), null);
        }}
      />
    ) : kept && doc.item && doc.readOnly === null ? (
      <RestoreBanner
        mode="kept"
        busy={bannerBusy}
        onRestore={() => {
          if (doc.item)
            onReopen?.({ item: doc.item, restore: { draft: kept, at: new Date().toISOString() } });
        }}
        onDiscard={() => {
          void discardSlot(kept, doc.item);
        }}
      />
    ) : null;

  const takeTheirs = async (open: OpenConflict) => {
    // Mine goes to the slot first: Take theirs never loses it (Restore brings it back).
    await doc.keepInSlot(open.mine);
    setConflict(null);
    onReopen?.({ item: open.theirs, kept: open.mine });
  };

  return (
    <div
      data-slot="editor-screen"
      data-focus-mode={focusMode || undefined}
      className="editor-screen -mx-page -mt-6 -mb-12 flex min-h-[calc(100svh-var(--sticky-top))] flex-1 flex-col"
    >
      <EditorBar
        type={type}
        typeLabel={typeLabel}
        doc={doc}
        focusMode={focusMode}
        onFocusMode={setFocusMode}
        panelOpen={panelOpen}
        onPanel={() => setPanelOpen(!panelOpen)}
        toggleRef={toggleRef}
        wide={wide}
      />
      <div className="flex flex-1 items-start">
        <div className="min-w-0 flex-1 px-page">
          <WritingSurface doc={doc} typeLabel={typeLabel} focusMode={focusMode} banner={banner} />
        </div>
        {focusMode && wide ? null : (
          <SidePanel
            doc={doc}
            type={type}
            typeLabel={typeLabel}
            open={panelOpen}
            onOpenChange={setPanelOpen}
            wide={wide}
            onPublish={(mode) => {
              if (!wide) setSheetOpen(false);
              setPublishMode(mode);
            }}
          />
        )}
      </div>
      <ConflictDialog
        type={type}
        conflict={conflict}
        onKeepMine={async () => {
          const open = conflict;
          if (!open) return;
          const next = await doc.keepMine(open.theirs);
          // Closed unless the save met yet another newer version (a new conflict).
          setConflict((c) => (c === open ? null : c));
          if (next) {
            toast.success(open.then ? t("conflict.keptThen") : t("conflict.kept"), {
              id: "editor-save",
            });
          }
        }}
        onTakeTheirs={async () => {
          if (conflict) await takeTheirs(conflict);
        }}
        onCopyMine={async () => {
          if (!conflict) return;
          await navigator.clipboard.writeText(blockText(conflict.mine.body));
          toast.success(t("conflict.copied"), { id: "editor-copy" });
          await takeTheirs(conflict);
        }}
      />
      <LeaveDialog
        open={guard.blocked}
        isNew={doc.item === null}
        onStay={guard.stay}
        onSave={async () => {
          const saved = await doc.save();
          if (saved && !doc.isDirtyNow()) guard.leave();
          else guard.stay();
        }}
        onDiscard={async () => {
          try {
            await doc.discard();
          } catch {
            // The slot stays: the next visit offers it back, nothing is lost.
          }
          guard.leave();
        }}
      />
      <PublishDialog
        doc={doc}
        mode={canPublish ? publishMode : null}
        onClose={() => setPublishMode(null)}
        siteUrl={site.data?.site_url ?? null}
        taxonomies={taxonomies}
        terms={terms}
      />
      {wide || focusMode ? (
        <EditorFooter
          editor={doc.editor}
          {...(doc.readOnly?.kind === "invalid" ? { fixedText: blockText(doc.storedBody) } : {})}
        />
      ) : (
        <PublishBar
          doc={doc}
          onSettings={() => setSheetOpen(true)}
          settingsOpen={sheetOpen}
          onPublish={setPublishMode}
          {...(doc.readOnly?.kind === "invalid" ? { fixedText: blockText(doc.storedBody) } : {})}
        />
      )}
    </div>
  );
}

function EditorBar({
  type,
  typeLabel,
  doc,
  focusMode,
  onFocusMode,
  panelOpen,
  onPanel,
  toggleRef,
  wide,
}: {
  type: string;
  typeLabel: string;
  doc: EditorDocument;
  focusMode: boolean;
  onFocusMode: (on: boolean) => void;
  panelOpen: boolean;
  onPanel: () => void;
  toggleRef: RefObject<HTMLButtonElement | null>;
  /** Below 1024 px the status and the settings button live in the publish bar. */
  wide: boolean;
}) {
  const { t } = useTranslation("editor");
  const status = doc.item?.status;
  const saving = doc.saveState === "saving";
  const showState = doc.item !== null || doc.dirty || doc.indicator !== "saved";
  return (
    <div
      data-slot="editor-bar"
      className={cn(
        "sticky top-(--sticky-top) z-10 flex h-12 items-center gap-2 bg-background/85 px-page backdrop-blur-md transition-opacity duration-(--dur-base) supports-backdrop-filter:bg-background/70",
        focusMode && "bg-transparent backdrop-blur-none supports-backdrop-filter:bg-transparent",
      )}
    >
      <Button
        asChild
        variant="ghost"
        size="sm"
        className={cn("-ml-2 gap-1.5 text-muted-foreground", focusMode && "invisible")}
      >
        <Link to={`/content/${type}` as "/"}>
          <ArrowLeftIcon data-icon="inline-start" />
          <span className="max-w-40 truncate">{typeLabel}</span>
        </Link>
      </Button>
      {status && isContentStatus(status) && !focusMode && wide ? (
        <StatusBadge status={status} />
      ) : null}
      <div className="ml-auto flex items-center gap-1.5 sm:gap-2">
        {showState && !doc.readOnly ? (
          <SaveState state={doc.indicator} {...(doc.indicatorAt ? { at: doc.indicatorAt } : {})} />
        ) : null}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-pressed={focusMode}
              aria-label={focusMode ? t("focus.exit") : t("focus.enter")}
              onClick={() => onFocusMode(!focusMode)}
              className="text-muted-foreground"
            >
              {focusMode ? <MinimizeIcon /> : <MaximizeIcon />}
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom" className="flex items-center gap-2">
            {focusMode ? t("focus.exit") : t("focus.enter")}
            <KbdCombo keys="mod+shift+f" />
          </TooltipContent>
        </Tooltip>
        {wide ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                ref={toggleRef}
                type="button"
                variant="ghost"
                size="icon"
                aria-expanded={panelOpen}
                aria-controls={panelOpen ? PANEL_ID : undefined}
                aria-label={t("panel.toggle")}
                aria-keyshortcuts="Meta+Period Control+Period"
                data-slot="panel-toggle"
                onClick={onPanel}
                className={cn("text-muted-foreground", panelOpen && "bg-muted text-foreground")}
              >
                <PanelRightIcon />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" className="flex items-center gap-2">
              {t("panel.toggle")}
              <KbdCombo keys="mod+." />
            </TooltipContent>
          </Tooltip>
        ) : null}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="outline"
              // Nothing to save: disabled, like the panel's own save action.
              disabled={doc.readOnly !== null || saving || (doc.item !== null && !doc.dirty)}
              aria-keyshortcuts="Meta+S Control+S"
              onClick={() => {
                void doc.save();
              }}
            >
              {t("save.button")}
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom" className="flex items-center gap-2">
            {t("save.button")}
            <KbdCombo keys="mod+s" />
          </TooltipContent>
        </Tooltip>
      </div>
    </div>
  );
}

function ReadOnlyBanner({ reason }: { reason: ReadOnlyReason }) {
  const { t } = useTranslation("editor");
  let text: ReactNode;
  if (reason.kind === "unsupported") {
    text =
      reason.unknown.length > 0
        ? t("readOnly.unsupported", {
            types: new Intl.ListFormat(uiLocale(), { type: "conjunction" }).format(reason.unknown),
          })
        : t("readOnly.invalid");
  } else {
    text = t(`readOnly.${reason.kind}`);
  }
  return (
    <div
      data-slot="read-only-banner"
      role="note"
      className="mt-6 flex items-start gap-3 rounded-lg border border-warning/40 bg-warning/8 px-4 py-3 text-sm text-foreground"
    >
      <ShieldAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
      <p>{text}</p>
    </div>
  );
}

function WritingSurface({
  doc,
  typeLabel,
  focusMode,
  banner,
}: {
  doc: EditorDocument;
  typeLabel: string;
  focusMode: boolean;
  /** The restore banner, when there is one. */
  banner: ReactNode;
}) {
  const { t } = useTranslation("editor");
  const { editor, readOnly } = doc;
  const title = useRef<HTMLTextAreaElement>(null);
  const editable = readOnly === null;

  // ↑ at the very start of the body goes back to the title.
  useEffect(() => {
    const dom = editor.view.dom;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "ArrowUp" || e.shiftKey || e.altKey || e.metaKey || e.ctrlKey) return;
      const { empty, from } = editor.state.selection;
      if (!empty || from > 1) return;
      const before = editor.view.coordsAtPos(from).top;
      const first = editor.view.coordsAtPos(1).top;
      if (before > first) return;
      e.preventDefault();
      const el = title.current;
      if (el) {
        el.focus();
        el.setSelectionRange(el.value.length, el.value.length);
      }
    };
    dom.addEventListener("keydown", onKey);
    return () => dom.removeEventListener("keydown", onKey);
  }, [editor]);

  // A new item opens with the cursor in the title.
  const [focusTitle] = useState(() => doc.initialFocus === "title");

  const onTitleKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    const el = e.currentTarget;
    const atEnd = el.selectionStart === el.value.length && el.selectionEnd === el.value.length;
    if (e.key === "Enter" || (e.key === "ArrowDown" && atEnd)) {
      e.preventDefault();
      // Synchronously, so the next keystroke already lands in the body (the `focus`
      // command can defer to the next frame).
      editor.commands.command(({ tr }) => {
        tr.setSelection(Selection.atStart(tr.doc));
        return true;
      });
      editor.view.focus();
    }
  };

  return (
    <article className="mx-auto w-full max-w-[720px] pb-[30vh]">
      <h1 className="sr-only">
        {doc.item
          ? t("heading.edit", { title: doc.title || t("title.untitled") })
          : t("heading.new", { label: typeLabel })}
      </h1>
      {readOnly ? <ReadOnlyBanner reason={readOnly} /> : banner}
      <textarea
        ref={title}
        data-slot="editor-title"
        rows={1}
        autoFocus={focusTitle}
        value={doc.title}
        readOnly={!editable}
        aria-label={t("title.label")}
        placeholder={t("title.placeholder")}
        onChange={(e) => doc.setTitle(e.target.value.replace(/\r?\n/g, " "))}
        onKeyDown={onTitleKey}
        className="editor-title mt-10 block w-full resize-none bg-transparent text-2xl font-semibold text-foreground placeholder:text-muted-foreground/60 focus-visible:outline-none sm:mt-16 sm:text-3xl"
      />
      {readOnly?.kind === "invalid" ? <StoredText body={doc.storedBody} /> : null}
      {/* Kept mounted (the editor's view lives in it), hidden when the stored body is
          shown as plain text instead. */}
      <EditorContent
        editor={editor}
        className="mt-4 sm:mt-6"
        hidden={readOnly?.kind === "invalid"}
      />
      {editable ? <EditorBubbleMenu editor={editor} layoutKey={focusMode} /> : null}
    </article>
  );
}

/**
 * A body TipTap's schema refused, shown without it: its text (`blockText`, the text the
 * site indexes) one paragraph per line, and, for accounts that edit others' content, the
 * stored JSON behind a disclosure. Nothing goes through the editor, so nothing is
 * dropped from view.
 */
function StoredText({ body }: { body: unknown }) {
  const { t } = useTranslation("editor");
  const queryClient = useQueryClient();
  const me = queryClient.getQueryData<MeDto>(qk.me);
  const lines = blockText(body)
    .split("\n")
    .filter((l) => l.trim() !== "");
  return (
    <div data-slot="stored-text" className="editor-body mt-4 sm:mt-6">
      {lines.length > 0 ? (
        lines.map((line, i) => <p key={i}>{line}</p>)
      ) : (
        <p data-slot="stored-text-empty" className="text-muted-foreground italic">
          {t("readOnly.noText")}
        </p>
      )}
      {me?.capabilities.includes("edit_others") ? (
        <details className="editor-stored-json">
          <summary>{t("readOnly.showJson")}</summary>
          {/* Focusable: a scrolling region must be reachable by keyboard. */}
          <pre tabIndex={0}>{JSON.stringify(body, null, 2)}</pre>
        </details>
      ) : null}
    </div>
  );
}

function EditorFooter({ editor, fixedText }: { editor: Editor; fixedText?: string }) {
  return (
    <div
      data-slot="editor-footer"
      className="sticky bottom-0 flex h-10 items-center justify-end gap-3 px-page text-xs text-muted-foreground tabular-nums max-xl:bg-background"
    >
      <WordStats editor={editor} {...(fixedText === undefined ? {} : { fixedText })} />
    </div>
  );
}
