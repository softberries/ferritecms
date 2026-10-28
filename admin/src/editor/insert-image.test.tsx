// Image files dropped on the body (M2b-2 final review, parked items): a finished upload
// is inserted once, also under StrictMode's double effects, and one that finishes in
// the same tick as the `/new` → `/$id` hand-over still lands where its placeholder was.

import { EditorContent, useEditor, type Editor } from "@tiptap/react";
import { act, render, waitFor } from "@testing-library/react";
import { StrictMode, useEffect } from "react";
import { afterEach, describe, expect, test } from "vitest";
import type { MediaDto } from "@/api/media";
import type { UploadEntry, UploadQueue } from "@/media/upload-queue";
import type { CarriedUploads } from "./body-uploads";
import { ImageUploads, useImageDrops } from "./insert-image";
import { editorExtensions } from "./schema";

const MEDIA_ID = "0190f1c0-0000-7000-8000-0000000000cc";
const media = { id: MEDIA_ID, alt: "A lake" } as MediaDto;
const file = new File(["x"], "lake.png", { type: "image/png" });

const BODY = {
  type: "doc",
  content: [
    { type: "paragraph", content: [{ type: "text", text: "One" }] },
    { type: "paragraph", content: [{ type: "text", text: "Two" }] },
  ],
};

/** A queue the test drives by hand (`set`), with the members the hook uses. */
function fakeQueue(initial: UploadEntry[]) {
  let rows = initial;
  const listeners = new Set<() => void>();
  const emit = () => {
    for (const fn of listeners) fn();
  };
  const queue: UploadQueue = {
    add: () => undefined,
    retry: () => undefined,
    cancel: (id) => {
      rows = rows.filter((r) => r.id !== id);
      emit();
    },
    clearDone: () => {
      rows = rows.filter((r) => r.state === "queued" || r.state === "uploading");
      emit();
    },
    subscribe: (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    entries: () => rows,
    active: () => rows.some((r) => r.state === "queued" || r.state === "uploading"),
    dispose: () => undefined,
  };
  return {
    queue,
    set(next: UploadEntry[]) {
      rows = next;
      emit();
    },
  };
}

const entry = (state: UploadEntry["state"]): UploadEntry => ({
  id: "u1",
  file,
  key: "k1",
  state,
  loaded: 0,
  total: 1,
  ...(state === "done" ? { media } : {}),
});

let current: { editor: Editor; handOver: () => CarriedUploads } | null = null;

function Screen({
  queue,
  carried,
}: {
  queue: UploadQueue;
  carried: Record<string, number | null> | null;
}) {
  const editor = useEditor({
    extensions: [
      ...editorExtensions({ placeholder: "", resolveMedia: () => Promise.resolve("/x.png") }),
      ImageUploads,
    ],
    content: BODY,
    immediatelyRender: true,
    shouldRerenderOnTransaction: false,
    injectCSS: false,
  });
  const drops = useImageDrops(editor, {
    queue,
    carried,
    canUpload: true,
    onForbidden: () => undefined,
  });
  useEffect(() => {
    current = { editor, handOver: drops.handOver };
  });
  return <EditorContent editor={editor} />;
}

const blocks = () => (current?.editor.getJSON().content ?? []).map((n) => n.type);

afterEach(() => {
  current = null;
});

describe("dropped images", () => {
  test("a_finished_upload_is_inserted_once_under_strict_mode", async () => {
    // A screen that mounts after the upload finished (a remount meanwhile) inserts it:
    // StrictMode runs the reconcile effect twice with the same entries.
    const { queue } = fakeQueue([entry("done")]);
    render(
      <StrictMode>
        <Screen queue={queue} carried={null} />
      </StrictMode>,
    );
    await waitFor(() => expect(blocks()).toContain("image"));
    expect(blocks().filter((b) => b === "image")).toHaveLength(1);
  });

  test("an_upload_done_in_the_hand_over_tick_lands_at_its_placeholder", async () => {
    // `/new`: the placeholder sits before the first paragraph.
    const q = fakeQueue([entry("uploading")]);
    const first = render(<Screen queue={q.queue} carried={{ u1: 0 }} />);
    await waitFor(() =>
      expect(document.querySelector("[data-slot=editor-upload-placeholder]")).not.toBeNull(),
    );
    // The upload finishes and the first save hands over in the same tick, before this
    // screen's effects ran.
    let carried: CarriedUploads | null = null;
    act(() => {
      q.set([entry("done")]);
      carried = current?.handOver() ?? null;
    });
    expect((carried as CarriedUploads | null)?.at).toEqual({ u1: 0 });
    first.unmount();

    // `/$id` takes the same body and puts the image where the placeholder was.
    render(<Screen queue={q.queue} carried={(carried as CarriedUploads | null)?.at ?? null} />);
    await waitFor(() => expect(blocks()).toContain("image"));
    expect(blocks()).toEqual(["image", "paragraph", "paragraph"]);
  });
});
