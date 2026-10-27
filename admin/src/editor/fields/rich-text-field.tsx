// A `rich_text` field: a compact editor with the body's own schema (`editorExtensions`,
// P19), so what it saves renders on the site as it looks here; the value is the `doc`
// JSON (`null` when empty). The selection bubble formats it; `/` inserts blocks. A stored
// value the schema cannot represent is shown as its text and kept as stored (Review Focus
// 3 of M2b-1: never rewritten by opening it). The editor follows its value: one that
// changes from outside (a row moved under it) replaces what it shows; its own edits,
// coming back as the value, do not.

import { EditorContent, useEditor, type JSONContent } from "@tiptap/react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "cn";
import { resolveMediaUrl } from "@/api/media";
import { EditorBubbleMenu } from "../bubble-menu";
import { editorExtensions, validateBody } from "../schema";
import { blockText } from "../text";
import type { ControlIds } from "./fields-context";
import { isObject, sameValue } from "./fields-model";

/** What the editor opens a value with (`undefined`: its own empty document). */
const contentOf = (value: unknown): JSONContent | undefined =>
  isObject(value) && Array.isArray(value.content) && value.content.length > 0 ? value : undefined;

export function RichTextField({
  ids,
  value,
  onChange,
  disabled,
}: {
  ids: ControlIds;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation("editor");
  const [supported] = useState(() => value == null || (isObject(value) && validateBody(value).ok));
  if (!supported) {
    return (
      <div
        id={ids.id}
        role="note"
        aria-describedby={ids.describedBy}
        className="rounded-lg border border-dashed px-3 py-2 text-sm text-muted-foreground"
      >
        <p className="mb-1 font-medium text-foreground">{t("fields.richUnsupported")}</p>
        <p className="whitespace-pre-wrap">{blockText(value)}</p>
      </div>
    );
  }
  return <RichEditor ids={ids} value={value} onChange={onChange} disabled={disabled} />;
}

function RichEditor({
  ids,
  value,
  onChange,
  disabled,
}: {
  ids: ControlIds;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled: boolean;
}) {
  const latest = useRef(onChange);
  useEffect(() => {
    latest.current = onChange;
  });
  const [extensions] = useState(() =>
    editorExtensions({ placeholder: "", resolveMedia: resolveMediaUrl }),
  );
  const content = contentOf(value);
  // The value the editor shows: set by its own edits and by values from outside.
  const shown = useRef<unknown>(value);
  const editor = useEditor({
    extensions,
    ...(content ? { content } : {}),
    editable: !disabled,
    immediatelyRender: true,
    shouldRerenderOnTransaction: false,
    injectCSS: false,
    editorProps: {
      attributes: {
        id: ids.id,
        class: "editor-body editor-body-compact",
        role: "textbox",
        "aria-multiline": "true",
        "aria-labelledby": ids.labelId,
        "aria-describedby": ids.describedBy,
        ...(ids.invalid ? { "aria-invalid": "true" } : {}),
      },
    },
    onUpdate: ({ editor: e }) => {
      const next = e.isEmpty ? null : e.getJSON();
      shown.current = next;
      latest.current(next);
    },
  });
  useEffect(() => {
    if (editor.isDestroyed || value === shown.current) return;
    shown.current = value;
    if (sameValue(value, editor.isEmpty ? null : editor.getJSON())) return;
    // A value this editor cannot represent is left alone (never rewritten by showing it).
    if (value != null && !(isObject(value) && validateBody(value).ok)) return;
    const next = contentOf(value);
    if (next) editor.commands.setContent(next, { emitUpdate: false });
    else editor.commands.clearContent(false);
  }, [editor, value]);
  useEffect(() => {
    if (!editor.isDestroyed && editor.isEditable === disabled) editor.setEditable(!disabled, false);
  }, [editor, disabled]);
  useEffect(() => {
    if (editor.isDestroyed) return;
    const dom = editor.view.dom;
    if (ids.invalid) dom.setAttribute("aria-invalid", "true");
    else dom.removeAttribute("aria-invalid");
    dom.setAttribute("aria-describedby", ids.describedBy);
  }, [editor, ids.invalid, ids.describedBy]);
  return (
    <div
      data-slot="rich-text-field"
      className={cn(
        "rounded-lg border border-input bg-transparent px-3 py-1 transition-colors focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50 dark:bg-input/30",
        ids.invalid && "border-destructive ring-3 ring-destructive/20",
        disabled && "opacity-50",
      )}
    >
      <EditorContent editor={editor} />
      {disabled ? null : <EditorBubbleMenu editor={editor} layoutKey={false} />}
    </div>
  );
}
