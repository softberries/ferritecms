// The writing surface (11 §1, after Ghost): a calm, centred 720 px column; the title as
// an auto-growing field in Inter's display cut; the body at 17/28; a selection bubble and
// a `/` insert menu; the word count and reading time at the foot; a focus mode
// (`mod+shift+f`) that hides the shell. A thin bar above the column carries the way
// back, the status, the save state, the panel toggle (`mod+.`) and Save (`mod+s`). The
// side panel (`panel/`) holds status and publishing (the publish dialog, `mod+shift+p`),
// the address, excerpt, terms, parent and the danger zone. Read-only items get a banner
// and no way to save or change anything (Review Focus 3).

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
import { metaQuery } from "@/api/meta";
import { isContentStatus } from "@/api/content";
import { qk } from "@/api/keys";
import type { MeDto } from "@/auth/session";
import { ApiError } from "@/api/problem";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { SaveState } from "@/components/kit/save-state";
import { StatusBadge } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { storeEditorPanelOpen, storedEditorPanelOpen } from "@/design/preferences";
import { uiLocale } from "@/lib/locale";
import { useShortcut } from "@/lib/shortcuts";
import { useShellChrome } from "@/shell/chrome-context";
import { EditorBubbleMenu } from "./bubble-menu";
import {
  type ContentDto,
  type EditorDocument,
  type Handoff,
  type ReadOnlyReason,
  type WriteOp,
  useEditorDocument,
} from "./editor-state";
import { useTermsById, useTypeTaxonomies, useWide } from "./panel/panel-state";
import { PublishDialog, type PublishMode } from "./panel/publish-dialog";
import { PublishBar } from "./panel/publish-bar";
import { PANEL_ID, SidePanel } from "./panel/side-panel";
import { blockText } from "./text";
import { readingMinutes, wordCount } from "./word-count";

export interface EditorScreenProps {
  type: string;
  /** The type's label (`Posts`), for the way back. */
  typeLabel: string;
  /** `null` for a new item (created on the first save). */
  item: ContentDto | null;
  handoff?: Handoff | null;
  /** After a new item's first save (the route moves to its `/$id`). */
  onCreated?: (item: ContentDto, handoff: Handoff) => void;
}

export function EditorScreen({ type, typeLabel, item, handoff, onCreated }: EditorScreenProps) {
  const { t } = useTranslation("editor");
  const doc = useEditorDocument({
    type,
    item,
    handoff: handoff ?? null,
    placeholder: t("body.placeholder"),
    bodyLabel: t("body.label"),
    ...(onCreated ? { onCreated } : {}),
    onConflict: () => {
      // Task 16 replaces this with the side-by-side conflict dialog; the text stays.
      toast.error(t("save.conflict"), { id: "editor-save" });
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
  const [publishMode, setPublishMode] = useState<PublishMode | null>(null);
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
          <WritingSurface doc={doc} typeLabel={typeLabel} focusMode={focusMode} />
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
  const showState = doc.item !== null || doc.dirty || doc.saveState !== "saved";
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
          <SaveState
            state={doc.saveState}
            {...(doc.savedAt && doc.saveState === "saved" ? { at: doc.savedAt } : {})}
          />
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
}: {
  doc: EditorDocument;
  typeLabel: string;
  focusMode: boolean;
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
      {readOnly ? <ReadOnlyBanner reason={readOnly} /> : null}
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

/** Words and reading time, recounted at most once a frame while the body changes. */
function useWordCount(editor: Editor): number {
  const [words, setWords] = useState(() => wordCount(blockText(editor.getJSON())));
  useEffect(() => {
    let frame = 0;
    const onUpdate = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setWords(wordCount(blockText(editor.getJSON()))));
    };
    editor.on("update", onUpdate);
    return () => {
      cancelAnimationFrame(frame);
      editor.off("update", onUpdate);
    };
  }, [editor]);
  return words;
}

function EditorFooter({ editor, fixedText }: { editor: Editor; fixedText?: string }) {
  const { t } = useTranslation("editor");
  const live = useWordCount(editor);
  const words = fixedText === undefined ? live : wordCount(fixedText);
  const minutes = readingMinutes(words);
  const n = new Intl.NumberFormat(uiLocale());
  return (
    <div
      data-slot="editor-footer"
      className="sticky bottom-0 flex h-10 items-center justify-end gap-3 px-page text-xs text-muted-foreground tabular-nums max-xl:bg-background"
    >
      <span>{t("footer.words", { count: words, formatted: n.format(words) })}</span>
      <span aria-hidden="true">·</span>
      <span>{t("footer.minutes", { count: minutes, formatted: n.format(minutes) })}</span>
    </div>
  );
}
