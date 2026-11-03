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
//
// Revisions (Task 17): the drawer (`mod+alt+r`, or the panel's "Revisions (n)") lists the
// saves; Restore asks first only when there are unsaved edits (they would be replaced),
// writes the revision back (`doc.restoreRevision`; a `412` opens the conflict dialog) and
// remounts the screen on the restored item. The new screen shows the toast with Undo, so
// the Undo always acts on the screen that is there (and goes with it): it restores the
// revision that was current before (the one after the restore's own snapshot).
//
// Media, fields and SEO (M2b-2 Task 8): the bar's image button and the slash menu's
// "Image" insert from the media picker; image files dropped on the body upload and land
// where they were dropped; the custom fields sit in the panel, or in a card under the
// body on wide screens when the type has more than 4; a save refused for field values
// opens the panel and puts the cursor on the first field that says why.

import "@/styles/editor.css";
import { Link } from "@tanstack/react-router";
import { Selection } from "@tiptap/pm/state";
import { EditorContent, type Editor } from "@tiptap/react";
import {
  ArrowLeftIcon,
  ImagePlusIcon,
  MaximizeIcon,
  EyeIcon,
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
import { getContent, isContentStatus, listRevisions, putAutosave } from "@/api/content";
import { qk } from "@/api/keys";
import { typesQuery } from "@/api/types";
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
import { UNDO_TOAST_MS } from "@/content/list-actions";
import { useShellChrome } from "@/shell/chrome-context";
import { type Draft, PREVIEW_DEBOUNCE_MS, newDraftKey, pendingDrafts } from "./autosave";
import { EditorBubbleMenu } from "./bubble-menu";
import { onPreconditionFailed } from "./conflict";
import { ConflictDialog, type OpenConflict } from "./conflict-dialog";
import type { Handoff } from "./handoff";
import {
  type ContentDto,
  type EditorDocument,
  type ReadOnlyReason,
  type RestoreSource,
  type WriteOp,
  autosaveInput,
  useEditorDocument,
} from "./editor-state";
import { FieldsSection } from "./fields/fields-section";
import { CARD_THRESHOLD, fieldsFor, fieldsPlacement } from "./fields/fields-model";
import { pickAndInsertImage, useImageDrops } from "./insert-image";
import { LeaveDialog } from "./leave-dialog";
import type { Backup } from "./local-backup";
import type { UploadQueue } from "@/media/upload-queue";
import { useLeaveGuard } from "./leave-guard";
import { RestoreBanner } from "./restore-banner";
import { RestoreConfirmDialog, RevisionsDrawer } from "./revisions/revisions-drawer";
import { undoTarget, whenOf } from "./revisions/revision-model";
import { useSplit, useTermsById, useTypeTaxonomies, useWide } from "./panel/panel-state";
import { PublishDialog, type PublishMode } from "./panel/publish-dialog";
import { PublishBar } from "./panel/publish-bar";
import { PANEL_ID, SidePanel } from "./panel/side-panel";
import { PreviewPane, SplitDivider } from "./preview/preview-pane";
import { storeSplit, storedSplit } from "./preview/preview-prefs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
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
  /** A revision restore (or its Undo) remounted the screen: it says so, with Undo. */
  revision?: RevisionNotice;
  /** The restored draft's Discard remounted the screen: it says so, with Undo (this draft). */
  discarded?: Draft;
  /**
   * This browser's backup still on offer (it stays offered until restored or discarded);
   * absent: the screen's offer, if any, is carried over; `null`: none.
   */
  backup?: Backup | null;
}

/** What the screen says after a revision restore remounted it. */
export type RevisionNotice =
  /**
   * Revision `rid` (saved `at`) was restored, giving the item `version`: the toast offers
   * Undo while the item is still at that version.
   */
  | { kind: "restored"; rid: string; at: string; version: number }
  /** The Undo of a restore ran. */
  | { kind: "undone" };

/**
 * A revision to restore: `at` is when it was saved; `undo` when it is a restore's Undo,
 * which goes with the version the restore produced as its `If-Match` (`version`).
 */
interface RestoreTarget {
  rid: string;
  at: string;
  undo: boolean;
  version?: number;
}

/** The kept Discard's toast (with Undo), dismissed when the screen goes. */
const KEPT_TOAST = "editor-kept";
/** The restored draft's Discard toast (with Undo), dismissed when the screen goes. */
const DISCARD_TOAST = "editor-discarded";
/** The revision restore's toast (with Undo), dismissed when the screen goes. */
const REVISION_TOAST = "editor-revision";
/**
 * The options of a message that takes over the revision toast (progress, failure, Undo
 * done): sonner merges an update into the toast with its id, so the restore's
 * description, its Undo and its long duration are cleared, not carried over.
 */
const REVISION_UPDATE = {
  id: REVISION_TOAST,
  description: undefined,
  action: undefined,
  duration: undefined,
} as const;

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
  /** This browser's backup of the item, found when the screen opened (offered if it differs). */
  backup?: Backup | null;
  /** After a new item's first save (the route moves to its `/$id`). */
  onCreated?: (item: ContentDto, handoff: Handoff) => void;
  /** Remounts the screen with another starting point (see `Reopen`). */
  onReopen?: (next: Reopen) => void;
  /** The screen was remounted by a revision restore (or its Undo). */
  revision?: RevisionNotice | null;
  /** The screen was remounted by the restored draft's Discard: the draft its Undo puts back. */
  discarded?: Draft | null;
  /**
   * The route's queue of images dropped on the body (`body-uploads.ts`): it outlives the
   * screen's remounts.
   */
  uploads: UploadQueue;
}

export function EditorScreen({
  type,
  typeLabel,
  item,
  handoff,
  restore,
  kept: keptInitially,
  backup,
  onCreated,
  onReopen: reopenScreen,
  revision,
  discarded,
  uploads,
}: EditorScreenProps) {
  const { t } = useTranslation("editor");
  const queryClient = useQueryClient();
  const [conflict, setConflict] = useState<OpenConflict | null>(null);
  // A hand-over from `/new` whose publish dialog was asked for opens with it.
  // Only when the account may publish or schedule it (else nothing would show).
  const [publishMode, setPublishMode] = useState<PublishMode | null>(() =>
    handoff?.openPublishDialog && (item?.allowed.publish || item?.allowed.schedule) ? "now" : null,
  );
  // `mod+shift+p` on a new item: its first save runs and the dialog opens on `/$id`.
  const publishAfterCreate = useRef(false);
  const [revisionsOpen, setRevisionsOpen] = useState(false);
  const types = useQuery(typesQuery);
  const typeDto = types.data?.find((ty) => ty.name === type);
  const fieldDefs = typeDto ? fieldsFor(typeDto) : null;
  const { focusMode, setFocusMode } = useShellChrome();

  // The live preview (design 15 §3.3): beside the text from 1280 px (a split view, the
  // side panel then a sheet), else in place of it ("Write | Preview"). Focus mode hides it.
  const screenWide = useWide();
  const split = useSplit();
  const [previewOn, setPreviewOn] = useState(false);
  const previewShown = previewOn && !focusMode;
  const splitView = previewShown && split;
  const [splitAt, setSplitAt] = useState(storedSplit);
  const splitBox = useRef<HTMLDivElement>(null);

  // The side panel: beside the text from 1024 px (open unless closed before), else a
  // sheet (closed until asked for). Focus mode hides it; so does the split preview (the
  // panel is a sheet then).
  const wide = screenWide && !splitView;
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
    [wide, focusMode, setFocusMode, setSheetOpen],
  );
  const fieldsInCard = wide && (fieldDefs?.length ?? 0) > CARD_THRESHOLD;
  /** A save refused for field values: the panel opens on the first field that says why. */
  const showInvalid = (source: "client" | "server") => {
    if (source === "client") toast.error(t("fields.blocked"), { id: "editor-save" });
    // Focus mode hides the card and the panel: the field must be reachable.
    if (focusMode) setFocusMode(false);
    // Unless every field is in the card (an SEO message from the server is in the panel).
    if (!fieldsInCard || source === "server") setPanelOpen(true);
    window.setTimeout(() => {
      const first = document.querySelector<HTMLElement>(
        "[data-slot=panel-fields] [aria-invalid=true], [data-slot=fields-card] [aria-invalid=true], [data-slot=panel-seo] [aria-invalid=true]",
      );
      first?.focus();
      first?.scrollIntoView({ block: "center" });
    }, 120);
  };
  // Every reopen carries a backup still on offer (only its Restore or Discard ends it).
  const onReopen = reopenScreen
    ? (next: Reopen) => {
        const offered = next.backup === undefined ? doc.backupNow() : next.backup;
        reopenScreen(offered ? { ...next, backup: offered } : next);
      }
    : undefined;
  const doc: EditorDocument = useEditorDocument({
    type,
    item,
    handoff: handoff ?? null,
    restore: restore ?? null,
    kept: keptInitially ?? null,
    backupUser: queryClient.getQueryData<MeDto>(qk.me)?.id ?? null,
    backup: backup ?? null,
    placeholder: t("body.placeholder"),
    bodyLabel: t("body.label"),
    fieldDefs,
    onInvalid: showInvalid,
    ...(onCreated
      ? {
          onCreated: (created: ContentDto, h: Handoff) =>
            onCreated(created, {
              ...h,
              openPublishDialog: publishAfterCreate.current,
              // Running drop uploads go on on the `/$id` screen.
              uploads: drops.handOver(),
            }),
        }
      : {}),
    onConflict: (e, op, context) => {
      // The user's text stays in the editor whatever happens next (Review Focus 3).
      const then = context.then ?? (op === "save" ? undefined : op);
      const state = onPreconditionFailed(e, doc.draft());
      setPublishMode(null);
      setRevisionsOpen(false);
      if (!state) {
        toast.error(t("save.conflict"), { id: "editor-save" });
        return;
      }
      if (!doc.isDirtyNow()) {
        // Nothing was changed here (a transition met someone else's save): there is
        // no "mine" to keep. The screen moves to their version and says so.
        toast.info(then ? t(`conflict.changedThen.${then}`) : t("conflict.changed"), {
          id: "editor-save",
        });
        if (onReopen) {
          onReopen({ item: state.theirs, ...(doc.kept ? { kept: doc.kept } : {}) });
        } else doc.rebase(state.theirs);
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
      // A revision restore handles its own failures in place (`onError`).
      if (op === "revision") return;
      const key = op === "save" ? ("save" as const) : (`transition.${op}` as const);
      toast.error(detail ? t(`${key}.failedWith`, { detail }) : t(`${key}.failed`), {
        id: "editor-save",
      });
    },
  });
  const group = t("shortcuts.group");

  // The preview reloads after each completed autosave (2 s idle while it shows) and each
  // save (a new item version).
  const [refreshKey, setRefreshKey] = useState(0);
  const [previewAt, setPreviewAt] = useState<Date | null>(null);
  const { onAutosaved } = doc;
  useEffect(
    () =>
      onAutosaved(() => {
        setPreviewAt(new Date());
        setRefreshKey((k) => k + 1);
      }),
    [onAutosaved],
  );
  const shownVersion = doc.item?.version;
  const firstVersion = useRef(shownVersion);
  useEffect(() => {
    if (shownVersion === undefined || shownVersion === firstVersion.current) return;
    firstVersion.current = shownVersion;
    setPreviewAt(null);
    setRefreshKey((k) => k + 1);
  }, [shownVersion]);
  const { setAutosaveDebounce } = doc;
  useEffect(() => {
    setAutosaveDebounce(previewShown ? PREVIEW_DEBOUNCE_MS : null);
  }, [previewShown, setAutosaveDebounce]);

  const me = queryClient.getQueryData<MeDto>(qk.me);
  const canUploadMedia = me?.capabilities.includes("upload_media") === true;
  const drops = useImageDrops(doc.editor, {
    queue: uploads,
    carried: handoff?.uploads?.at ?? null,
    canUpload: canUploadMedia,
    onForbidden: () => toast.error(t("image.uploadForbidden"), { id: "editor-drop" }),
  });
  const { uploading } = drops;
  const placement = fieldsPlacement(doc.fieldDefs, doc.meta.fields, wide);
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
      if (canPublish) {
        setPublishMode("now");
        return;
      }
      // A new item has no `allowed` yet: it is saved as a draft first (or the save in
      // flight is joined), and the dialog opens on its `/$id` screen.
      publishAfterCreate.current = true;
      void doc.save().then((created) => {
        if (!created) publishAfterCreate.current = false;
      });
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
    // Not under a modal dialog (a second PATCH under the conflict dialog would replace
    // its state); popovers (terms, parent) and the settings sheet still save. The binding
    // still fires under a dialog, so the browser's own Save page never opens.
    run: () => {
      const modal = '[data-slot="dialog-overlay"], [data-slot="alert-dialog-overlay"]';
      if (document.querySelector(modal) !== null) return;
      void doc.save();
    },
  });
  useShortcut({
    id: "editor.preview",
    keys: "mod+alt+p",
    label: t("shortcuts.preview"),
    group,
    allowInInput: true,
    run: () => {
      if (focusMode) setFocusMode(false);
      setPreviewOn(!previewShown);
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

  // Revisions: only for an item the server lets this account edit (R26; the API
  // answers 404 otherwise).
  const canRevisions = doc.item !== null && doc.item.allowed.edit;
  const [restoring, setRestoring] = useState<string | null>(null);
  const [confirmRestore, setConfirmRestore] = useState<RestoreTarget | null>(null);
  useShortcut({
    id: "editor.revisions",
    keys: "mod+alt+r",
    label: t("shortcuts.revisions"),
    group,
    allowInInput: true,
    when: () =>
      canRevisions &&
      (revisionsOpen || document.querySelector('[role="dialog"], [role="alertdialog"]') === null),
    run: () => setRevisionsOpen(!revisionsOpen),
  });

  const runRestore = async (target: RestoreTarget) => {
    setRestoring(target.rid);
    // The editor is locked meanwhile; the toast says what is happening (the drawer is
    // closed for an Undo).
    toast.loading(t("revisions.restoring"), REVISION_UPDATE);
    let failed = false;
    const next = await doc.restoreRevision(target.rid, {
      ...(target.version === undefined ? {} : { version: target.version }),
      onError: (e) => {
        failed = true;
        const detail =
          e instanceof ApiError
            ? e.problem.errors?.[0]?.detail || e.problem.detail || e.problem.title
            : null;
        toast.error(
          detail ? t("revisions.failedWith", { detail }) : t("revisions.failed"),
          REVISION_UPDATE,
        );
      },
    });
    if (!next || !onReopen) {
      // A 412 or a sign-in has its own message; only the progress toast goes.
      if (!failed) toast.dismiss(REVISION_TOAST);
      setRestoring(null);
      return;
    }
    // The slot is gone and the autosaver stopped: the screen reopens on the restored
    // item, which then offers Undo.
    onReopen({
      item: next,
      ...(doc.kept ? { kept: doc.kept } : {}),
      revision: target.undo
        ? { kind: "undone" }
        : { kind: "restored", rid: target.rid, at: target.at, version: next.version },
    });
  };
  /** Restore (or Undo): unsaved edits would be replaced, so that asks first. */
  const requestRestore = (target: RestoreTarget) => {
    if (doc.readOnly === null && doc.isDirtyNow()) setConfirmRestore(target);
    else void runRestore(target);
  };
  const requestRestoreRef = useRef(requestRestore);
  useEffect(() => {
    requestRestoreRef.current = requestRestore;
  });

  // After a restore remounted the screen: the toast, with Undo when there is a revision
  // to go back to (none when someone saved right after the restore).
  const [notice] = useState(() => revision ?? null);
  const noticeItem = item?.id;
  // Undo only while the item is still what the restore made it: any save or transition
  // after it ends the offer, so Undo never reverts work saved since (the `If-Match` of
  // the Undo is the backstop: a change it missed answers 412).
  const undoable = useRef(true);
  const itemVersion = doc.item?.version;
  useEffect(() => {
    if (notice?.kind !== "restored" || itemVersion === undefined) return;
    if (itemVersion !== notice.version && undoable.current) {
      undoable.current = false;
      toast.dismiss(REVISION_TOAST);
    }
  }, [notice, itemVersion]);
  useEffect(() => {
    if (!notice || !noticeItem) return;
    let live = true;
    if (notice.kind === "undone") {
      toast.success(t("revisions.undone"), REVISION_UPDATE);
    } else {
      // The restore landed: "Restoring…" goes now, not when the list below answers (its
      // Undo is added then).
      toast.success(t("revisions.restored"), { ...REVISION_UPDATE, duration: UNDO_TOAST_MS });
      void listRevisions(type, noticeItem, { limit: 2 })
        .then((list) => undoTarget(list.items, notice.rid))
        .catch(() => null)
        .then((undo) => {
          if (!live || !undoable.current) return;
          toast.success(t("revisions.restored"), {
            id: REVISION_TOAST,
            duration: UNDO_TOAST_MS,
            description: undo
              ? t("revisions.restoredDescription", { when: whenOf(notice.at) })
              : t("revisions.undoUnavailable"),
            ...(undo
              ? {
                  action: {
                    label: t("revisions.undo"),
                    onClick: (event) => {
                      // Sonner removes a toast 200 ms after its action runs unless the
                      // click's default is prevented; the Undo's progress and its result
                      // reuse this toast (CI run 40404431115: they went with it).
                      event.preventDefault();
                      requestRestoreRef.current({
                        rid: undo,
                        at: "",
                        undo: true,
                        version: notice.version,
                      });
                    },
                  },
                }
              : {}),
          });
        });
    }
    // The Undo belongs to this screen: it goes with it.
    return () => {
      live = false;
      toast.dismiss(REVISION_TOAST);
    };
  }, [notice, noticeItem, type, t]);

  // Leaving with changes the item does not hold asks first; closing the tab only while
  // the slot does not hold them either (F38).
  const [leaveError, setLeaveError] = useState<string | null>(null);
  // Images dropped on the body that are still uploading count too: leaving cancels them,
  // so it asks first (never silently).
  const guard = useLeaveGuard(
    (doc.dirty && doc.readOnly === null) || doc.keptUnstored || uploading > 0,
    { unload: doc.unprotected || uploading > 0 },
  );

  // The restore banner's state: the restored draft (from the document), or the user's own
  // version kept in the slot after Take theirs (`doc.kept`), and when it was kept.
  const [keptAt] = useState(() => new Date().toISOString());
  const [bannerBusy, setBannerBusy] = useState(false);
  const [restoredShown, setRestoredShown] = useState(true);

  // The kept Discard's Undo belongs to this screen: it must not run once the screen has
  // gone (the next one may have autosaved newer edits into the slot).
  useEffect(
    () => () => {
      toast.dismiss(KEPT_TOAST);
    },
    [],
  );

  // After the restored draft's Discard: the toast, with Undo (it writes the draft back to
  // the slot and reopens on it). The Undo belongs to this screen: it goes with it, and
  // with the first edit here (it would replace that edit unseen).
  const [discardedDraft] = useState(() => discarded ?? null);
  const reopenRef = useRef(onReopen);
  useEffect(() => {
    reopenRef.current = onReopen;
  });
  const discardedItem = item?.id;
  useEffect(() => {
    if (!discardedDraft || !discardedItem) return;
    const id = discardedItem;
    toast.success(t("restore.discarded"), {
      id: DISCARD_TOAST,
      action: {
        label: t("restore.undo"),
        onClick: () => {
          void putAutosave(type, id, autosaveInput(discardedDraft))
            .then(() => getContent(type, id))
            .then((now) =>
              reopenRef.current?.({
                item: now,
                restore: { draft: discardedDraft, at: new Date().toISOString() },
              }),
            )
            .catch(() => toast.error(t("restore.undoFailed"), { id: "editor-restore" }));
        },
      },
    });
    return () => {
      toast.dismiss(DISCARD_TOAST);
    };
  }, [discardedDraft, discardedItem, type, t]);
  useEffect(() => {
    if (discardedDraft && doc.dirty) toast.dismiss(DISCARD_TOAST);
  }, [discardedDraft, doc.dirty]);

  const failedDiscard = (e: unknown) => {
    if (isProblem(e, "unauthenticated")) reportApiError(e);
    else toast.error(t("restore.discardFailed"), { id: "editor-restore" });
  };

  /**
   * The restored draft's Discard: the slot goes for good (`doc.discard` holds the
   * autosaver, lets a running PUT land first and writes nothing after), then the screen
   * reopens on the stored item. Undo puts `draft` back.
   */
  const discardRestored = async (draft: Draft) => {
    if (!doc.item) {
      // A new item has no slot: back to an empty screen.
      onReopen?.({ item: null });
      return;
    }
    const id = doc.item.id;
    setBannerBusy(true);
    try {
      await doc.discard();
      // The slot is gone; a failed reload of the item still leaves this screen (on the
      // copy it has), so no screen goes on without a working autosaver. The reopened
      // screen says so, with Undo (which goes with it).
      const fresh = await getContent(type, id).catch(() => doc.item);
      onReopen?.({ item: fresh, discarded: draft });
    } catch (e) {
      // Nothing changed: the restored draft is still the working draft and still guarded.
      setRestoredShown(true);
      failedDiscard(e);
    } finally {
      setBannerBusy(false);
    }
  };

  /** The kept copy's Discard; the working draft is autosaved from then on. Undo keeps it again. */
  const discardKept = async (copy: Draft) => {
    setBannerBusy(true);
    try {
      await doc.discardKept();
      toast.success(t("restore.keptDiscarded"), {
        id: KEPT_TOAST,
        action: {
          label: t("restore.undo"),
          onClick: () => {
            void doc
              .keepInSlot(copy, { asKept: true })
              .catch(() => toast.error(t("restore.undoFailed"), { id: "editor-restore" }));
          },
        },
      });
    } catch (e) {
      failedDiscard(e);
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
        inMemory={doc.item === null}
        busy={bannerBusy}
        onDiscard={() => {
          setRestoredShown(false);
          void discardRestored(doc.draft());
        }}
      />
    ) : doc.kept && doc.item && doc.readOnly === null ? (
      <RestoreBanner
        mode="kept"
        busy={bannerBusy}
        stored={!doc.keptUnstored}
        edited={doc.dirty}
        onRestore={() => {
          const copy = doc.kept;
          // The slot already holds it, but written again: the reopened screen then
          // knows the slot holds exactly what it shows. The edits it replaces go, and
          // with them their local backup.
          if (doc.item && copy) {
            doc.dropBackup();
            onReopen?.({ item: doc.item, restore: { draft: copy, at: keptAt, flush: true } });
          }
        }}
        onDiscard={() => {
          if (doc.kept) void discardKept(doc.kept);
        }}
      />
    ) : null;

  /**
   * This browser's backup brings back edits made over a kept copy: what the slot holds
   * stays kept (written there first when it is this screen's working draft) and the
   * backup becomes the working draft, as it was before the page went away.
   */
  const restoreBackup = async (b: Backup) => {
    if (!doc.item) return;
    setBannerBusy(true);
    try {
      const slot = doc.kept ?? (doc.isDirtyNow() ? doc.draft() : null);
      if (slot && !doc.kept) await doc.keepInSlot(slot);
      onReopen?.(
        slot
          ? {
              item: doc.item,
              kept: slot,
              restore: { draft: b.draft, at: b.saved_at, local: true },
              backup: null,
            }
          : {
              item: doc.item,
              // Written to the slot as the screen opens; then the backup is not needed.
              restore: { draft: b.draft, at: b.saved_at, flush: true, onStored: doc.dropBackup },
              backup: null,
            },
      );
    } catch (e) {
      // A lost session was handed on by `keepInSlot` (the draft is kept for the sign-in).
      if (!isProblem(e, "unauthenticated") && !isProblem(e, "password-reset-required")) {
        toast.error(t("restore.backupFailed"), { id: "editor-restore" });
      }
    } finally {
      setBannerBusy(false);
    }
  };
  const backupBanner =
    doc.backup && doc.item && doc.readOnly === null ? (
      <RestoreBanner
        mode="backup"
        at={doc.backup.saved_at}
        busy={bannerBusy}
        onRestore={() => {
          if (doc.backup) void restoreBackup(doc.backup);
        }}
        onDiscard={() => {
          doc.dropBackup();
          toast.success(t("restore.backupDiscarded"), { id: "editor-restore" });
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
        wide={screenWide}
        split={split}
        previewOn={previewShown}
        onPreview={(on) => {
          if (on && focusMode) setFocusMode(false);
          setPreviewOn(on);
        }}
      />
      <div ref={splitBox} className="flex flex-1 items-start">
        <div
          data-slot="writing-column"
          hidden={previewShown && !split}
          style={splitView ? { width: `${splitAt}%` } : undefined}
          className={cn("min-w-0 px-page", splitView ? "shrink-0" : "flex-1")}
        >
          <WritingSurface
            doc={doc}
            typeLabel={typeLabel}
            focusMode={focusMode}
            banner={
              banner || backupBanner ? (
                <>
                  {banner}
                  {backupBanner}
                </>
              ) : null
            }
            fieldsCard={placement === "card" && !focusMode}
          />
        </div>
        {previewShown ? (
          <>
            {splitView ? (
              <SplitDivider
                value={splitAt}
                container={splitBox}
                onChange={(n) => {
                  setSplitAt(n);
                  storeSplit(n);
                }}
              />
            ) : null}
            <PreviewPane
              type={type}
              itemId={doc.item?.id ?? null}
              title={doc.title}
              refreshKey={refreshKey}
              updatedAt={previewAt}
              onSave={() => {
                void doc.save();
              }}
              className={cn(
                "sticky top-[calc(var(--sticky-top)+3rem)] h-[calc(100svh-var(--sticky-top)-5.5rem)] flex-1",
                !split && "max-lg:h-[calc(100svh-var(--sticky-top)-6.5rem)]",
              )}
            />
          </>
        ) : null}
        {focusMode && wide ? null : (
          <SidePanel
            doc={doc}
            type={type}
            typeLabel={typeLabel}
            open={panelOpen}
            onOpenChange={setPanelOpen}
            wide={wide}
            fieldsHere={placement === "panel"}
            onPublish={(mode) => {
              if (!wide) setSheetOpen(false);
              setPublishMode(mode);
            }}
            {...(canRevisions
              ? {
                  onRevisions: () => {
                    if (!wide) setSheetOpen(false);
                    setRevisionsOpen(true);
                  },
                }
              : {})}
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
        autosaved={!doc.unprotected}
        kept={doc.kept !== null && !doc.keptUnstored}
        keptOnlyHere={doc.keptUnstored}
        uploading={uploading}
        changes={(doc.dirty && doc.readOnly === null) || doc.keptUnstored}
        error={leaveError}
        onStay={() => {
          setLeaveError(null);
          guard.stay();
        }}
        onSave={async () => {
          setLeaveError(null);
          const saved = await doc.save();
          if (!saved || doc.isDirtyNow()) {
            guard.stay();
            return;
          }
          // A kept copy only on this page goes to the slot before leaving, or nobody leaves.
          if (!(await doc.storeKept())) {
            setLeaveError(t("leave.keptFailed"));
            return;
          }
          guard.leave();
        }}
        onDiscard={async () => {
          setLeaveError(null);
          // Discard drops the new edits, never the kept copy: it is stored first.
          if (!(await doc.storeKept())) {
            setLeaveError(t("leave.keptFailed"));
            return;
          }
          try {
            await doc.discard();
          } catch {
            // The slot stays: the next visit offers it back, nothing is lost.
          }
          guard.leave();
        }}
      />
      {doc.item && canRevisions ? (
        <RevisionsDrawer
          open={revisionsOpen}
          onOpenChange={setRevisionsOpen}
          type={type}
          item={doc.item}
          restoring={restoring}
          onRestore={(r) => requestRestore({ rid: r.id, at: r.created_at, undo: false })}
          onFocusFallback={() => {
            // Opened from the phone's settings sheet (gone now): its button, else the body.
            const toggle = document.querySelector<HTMLElement>("[data-slot=panel-toggle]");
            if (toggle) toggle.focus();
            else if (!doc.editor.isDestroyed) doc.editor.view.dom.focus();
          }}
        />
      ) : null}
      <RestoreConfirmDialog
        open={confirmRestore !== null}
        onCancel={() => setConfirmRestore(null)}
        onConfirm={() => {
          const target = confirmRestore;
          setConfirmRestore(null);
          if (target) void runRestore(target);
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
  split,
  previewOn,
  onPreview,
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
  /** From 1280 px the preview sits beside the text (a toggle); below, Write | Preview. */
  split: boolean;
  previewOn: boolean;
  onPreview: (on: boolean) => void;
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
        // The back link gives way first on a narrow bar: its label truncates (down to the
        // arrow alone) so the save state and Save always fit, whatever the font metrics.
        className={cn(
          "-ml-2 min-w-0 shrink gap-1.5 text-muted-foreground",
          focusMode && "invisible",
        )}
      >
        <Link to={`/content/${type}` as "/"} title={typeLabel}>
          <ArrowLeftIcon data-icon="inline-start" />
          <span className="max-w-40 min-w-0 truncate">{typeLabel}</span>
        </Link>
      </Button>
      {status && isContentStatus(status) && !focusMode && wide ? (
        <StatusBadge status={status} />
      ) : null}
      <div className="ml-auto flex shrink-0 items-center gap-1.5 sm:gap-2">
        {showState && !doc.readOnly ? (
          <SaveState state={doc.indicator} {...(doc.indicatorAt ? { at: doc.indicatorAt } : {})} />
        ) : null}
        {doc.readOnly === null && !focusMode ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label={t("image.insert")}
                data-slot="insert-image"
                disabled={doc.restoring}
                onClick={() => void pickAndInsertImage(doc.editor)}
                // Phones insert images from the `/` menu: the bar has no room.
                className="text-muted-foreground max-sm:hidden"
              >
                <ImagePlusIcon />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">{t("image.insert")}</TooltipContent>
          </Tooltip>
        ) : null}
        {split ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-pressed={previewOn}
                aria-label={t("preview.toggle")}
                aria-keyshortcuts="Meta+Alt+P Control+Alt+P"
                data-slot="preview-toggle"
                onClick={() => onPreview(!previewOn)}
                className={cn("text-muted-foreground", previewOn && "bg-muted text-foreground")}
              >
                <EyeIcon />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" className="flex items-center gap-2">
              {t("preview.toggle")}
              <KbdCombo keys="mod+alt+p" />
            </TooltipContent>
          </Tooltip>
        ) : (
          <ToggleGroup
            type="single"
            spacing={0}
            value={previewOn ? "preview" : "write"}
            aria-label={t("preview.view")}
            data-slot="preview-switch"
            onValueChange={(v) => {
              if (v === "write" || v === "preview") onPreview(v === "preview");
            }}
            className="rounded-lg bg-muted p-0.5"
          >
            {(["write", "preview"] as const).map((v) => (
              <ToggleGroupItem
                key={v}
                value={v}
                aria-keyshortcuts="Meta+Alt+P Control+Alt+P"
                className="h-7 rounded-md! border-0 px-2.5 text-xs text-muted-foreground pointer-coarse:h-11 data-[state=on]:bg-background data-[state=on]:text-foreground data-[state=on]:shadow-(--shadow-1)"
              >
                {v === "write" ? t("preview.write") : t("preview.show")}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        )}
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
              disabled={
                doc.readOnly !== null ||
                doc.restoring ||
                saving ||
                (doc.item !== null && !doc.dirty)
              }
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
  fieldsCard,
}: {
  doc: EditorDocument;
  typeLabel: string;
  focusMode: boolean;
  /** The restore banner, when there is one. */
  banner: ReactNode;
  /** The custom fields as a card under the body (wide screens, more than 4 fields). */
  fieldsCard: boolean;
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

  // A new item opens with the cursor in the title, unless a dialog (the command palette
  // opened while this screen loaded) already holds the focus.
  const [focusTitle] = useState(() => doc.initialFocus === "title");
  useEffect(() => {
    if (!focusTitle) return;
    if (document.activeElement?.closest("[role=dialog], [role=alertdialog]")) return;
    title.current?.focus();
  }, [focusTitle]);

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
        value={doc.title}
        readOnly={!editable || doc.restoring}
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
      {fieldsCard ? <FieldsSection doc={doc} card /> : null}
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
