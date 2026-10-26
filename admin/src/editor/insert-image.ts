// Images from the media library in a body (07 §3 Editor): the slash menu's "Image" and
// the editor bar's button open the media picker (images only) and insert
// `image { src: "ferrite:media/<id>", alt: <the item's alt text>, title: null }`, which
// the site resolves to a variant URL. Image files dropped on the body upload through the
// same queue as the library (at most 3 at once, one `Idempotency-Key` per file); each
// shows a placeholder where it was dropped, which follows the edits made meanwhile, and
// becomes the image when its upload is done. A failed upload says why in a toast and its
// placeholder goes. External `https:` images already in a body are left as they are.

import { Extension, type Editor, type JSONContent } from "@tiptap/core";
import { Plugin, PluginKey, type EditorState } from "@tiptap/pm/state";
import { Decoration, DecorationSet, type EditorView } from "@tiptap/pm/view";
import { useCallback, useEffect, useRef } from "react";
import { toast } from "sonner";
import type { MediaDto } from "@/api/media";
import { i18n } from "@/i18n";
import { openMediaPicker } from "@/media/media-picker";
import { uploadFailure } from "@/media/upload-errors";
import { useUploadQueue } from "@/media/use-upload-queue";

export const MEDIA_SRC_PREFIX = "ferrite:media/";

/** The body's reference to a media item. */
export const mediaSrc = (id: string) => `${MEDIA_SRC_PREFIX}${id}`;

/** The image node for `media` (its alt text; an empty one marks the image decorative). */
export function imageNodeOf(media: Pick<MediaDto, "id" | "alt">): JSONContent {
  return { type: "image", attrs: { src: mediaSrc(media.id), alt: media.alt, title: null } };
}

/** Inserts `media` at `pos`, else at the selection. */
export function insertMediaImage(editor: Editor, media: MediaDto, pos?: number): boolean {
  if (editor.isDestroyed || !editor.isEditable) return false;
  const node = imageNodeOf(media);
  const chain = editor.chain().focus();
  return (pos === undefined ? chain.insertContent(node) : chain.insertContentAt(pos, node)).run();
}

/** Opens the picker for an image and inserts the chosen one at the selection. */
export async function pickAndInsertImage(editor: Editor): Promise<boolean> {
  const media = await openMediaPicker({
    kind: "image",
    title: i18n.t("editor:image.pickTitle"),
  });
  if (!media) {
    if (!editor.isDestroyed) editor.commands.focus();
    return false;
  }
  return insertMediaImage(editor, media);
}

// --- Dropped image files ---------------------------------------------------------------

type Action = { add: { id: string; pos: number; name: string } } | { remove: string };

const uploadsKey = new PluginKey<DecorationSet>("imageUploads");

function placeholder(name: string): HTMLElement {
  const el = document.createElement("div");
  el.className = "editor-upload-placeholder";
  el.dataset.slot = "editor-upload-placeholder";
  el.setAttribute("role", "status");
  // A file name is untrusted text: set as text, never as markup.
  el.textContent = i18n.t("editor:image.uploading", { name });
  return el;
}

/** Where a block dropped at `pos` goes: before the block under it (a block boundary). */
export function blockPos(state: EditorState, pos: number): number {
  const $pos = state.doc.resolve(Math.max(0, Math.min(pos, state.doc.content.size)));
  return $pos.depth === 0 ? $pos.pos : $pos.before(1);
}

/** Shows where a dropped file will go (it follows later edits). */
export function addUploadPlaceholder(view: EditorView, id: string, pos: number, name: string) {
  view.dispatch(view.state.tr.setMeta(uploadsKey, { add: { id, pos, name } } satisfies Action));
}

export function removeUploadPlaceholder(view: EditorView, id: string) {
  view.dispatch(view.state.tr.setMeta(uploadsKey, { remove: id } satisfies Action));
}

/** The placeholder's position now, or `null` when it is gone. */
export function placeholderPos(state: EditorState, id: string): number | null {
  const set = uploadsKey.getState(state);
  const found = set?.find(undefined, undefined, (spec: { id?: string }) => spec.id === id);
  return found?.[0]?.from ?? null;
}

/** The DOM event the body sends for dropped image files (`useImageDrops` takes it). */
export const IMAGE_DROP_EVENT = "ferrite:image-drop";

export interface ImageDropDetail {
  files: File[];
  /** The block position the images go to. */
  pos: number;
}

/** Takes image files dropped on the body; shows the upload placeholders. */
export const ImageUploads = Extension.create({
  name: "imageUploads",
  addProseMirrorPlugins() {
    const editor = this.editor;
    return [
      new Plugin<DecorationSet>({
        key: uploadsKey,
        state: {
          init: () => DecorationSet.empty,
          apply(tr, set) {
            let next = set.map(tr.mapping, tr.doc);
            const action = tr.getMeta(uploadsKey) as Action | undefined;
            if (action && "add" in action) {
              const { id, pos, name } = action.add;
              const at = Math.max(0, Math.min(pos, tr.doc.content.size));
              next = next.add(tr.doc, [
                Decoration.widget(at, () => placeholder(name), { id, side: -1 }),
              ]);
            } else if (action && "remove" in action) {
              const id = action.remove;
              next = next.remove(
                next.find(undefined, undefined, (spec: { id?: string }) => spec.id === id),
              );
            }
            return next;
          },
        },
        props: {
          decorations: (state) => uploadsKey.getState(state),
          handleDrop: (view, event, _slice, moved) => {
            if (moved || !editor.isEditable) return false;
            const files = Array.from(event.dataTransfer?.files ?? []).filter((f) =>
              f.type.startsWith("image/"),
            );
            if (files.length === 0) return false;
            event.preventDefault();
            const at = view.posAtCoords({ left: event.clientX, top: event.clientY });
            const detail: ImageDropDetail = {
              files,
              pos: blockPos(view.state, at?.pos ?? view.state.selection.from),
            };
            view.dom.dispatchEvent(new CustomEvent(IMAGE_DROP_EVENT, { detail }));
            return true;
          },
        },
      }),
    ];
  },
});

/**
 * The upload side of image files dropped on `editor`'s body. Each file gets a
 * placeholder; a finished upload is inserted where its placeholder is now. The queue
 * lives as long as the screen; the number still running is returned for the editor's
 * leave guard, which asks before leaving cancels them (never silently). Without
 * `canUpload` a drop calls `onForbidden` instead (the server would refuse it, P30).
 */
export function useImageDrops(
  editor: Editor,
  { canUpload, onForbidden }: { canUpload: boolean; onForbidden: () => void },
): { uploading: number } {
  const tracked = useRef(new Set<string>());
  const noop = useCallback(() => undefined, []);
  const { queue, entries } = useUploadQueue(noop);
  useEffect(() => {
    if (editor.isDestroyed) return;
    for (const e of entries) {
      if (!tracked.current.has(e.id)) continue;
      if (e.state === "done" && e.media) {
        tracked.current.delete(e.id);
        const pos = placeholderPos(editor.state, e.id);
        removeUploadPlaceholder(editor.view, e.id);
        if (pos !== null) insertMediaImage(editor, e.media, pos);
      } else if (e.state === "failed" || e.state === "cancelled") {
        tracked.current.delete(e.id);
        removeUploadPlaceholder(editor.view, e.id);
        if (e.state === "failed") {
          const failure = uploadFailure(e.error);
          toast.error(i18n.t("editor:image.uploadFailed", { name: e.file.name }), {
            description: i18n.t(`media:tray.reason.${failure.kind}`),
          });
          queue.cancel(e.id);
        }
      }
    }
  }, [entries, editor, queue]);
  const forbidden = useRef(onForbidden);
  useEffect(() => {
    forbidden.current = onForbidden;
  });
  useEffect(() => {
    if (editor.isDestroyed) return;
    const dom = editor.view.dom;
    const onDrop = (event: Event) => {
      const { files, pos } = (event as CustomEvent<ImageDropDetail>).detail;
      if (!canUpload) {
        forbidden.current();
        return;
      }
      const before = new Set(queue.entries().map((e) => e.id));
      queue.add(files);
      for (const e of queue.entries()) {
        if (before.has(e.id)) continue;
        tracked.current.add(e.id);
        addUploadPlaceholder(editor.view, e.id, pos, e.file.name);
      }
    };
    dom.addEventListener(IMAGE_DROP_EVENT, onDrop);
    return () => dom.removeEventListener(IMAGE_DROP_EVENT, onDrop);
  }, [editor, queue, canUpload]);
  return {
    uploading: entries.filter((e) => e.state === "queued" || e.state === "uploading").length,
  };
}
