// The `embed` node (M3 P11): an atom block naming a YouTube or Vimeo video by `provider`
// and its watch `url`. The site renders a privacy-mode player `<iframe>`; the editor
// never does (the admin CSP has no `frame-src`, and it keeps none), it shows a card with
// the provider and URL. The URL is changed in an inline form that accepts only video links
// the site can play (`embed.ts`, the twin of the core's `blocks::embed_id`) and stores
// their canonical form. An embed whose stored URL does not parse (an API client's) says
// that the site shows it as a link.

import { Node, mergeAttributes } from "@tiptap/core";
import { NodeViewWrapper, ReactNodeViewRenderer, type ReactNodeViewProps } from "@tiptap/react";
import { PencilIcon, SquarePlayIcon } from "lucide-react";
import { cn } from "cn";
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { embedFromUrl, parseEmbed } from "./embed";

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    embedBlock: {
      /** Inserts an empty embed and opens its URL form. */
      insertEmbed: () => ReturnType;
    };
  }
}

/** Set by `insertEmbed` so the next embed view to mount opens its form. */
let openNext = false;

function EmbedView({ node, selected, editor, updateAttributes, deleteNode }: ReactNodeViewProps) {
  const { t } = useTranslation("editor");
  const url = typeof node.attrs.url === "string" ? node.attrs.url : "";
  const parsed = url ? parseEmbed(url) : null;
  const playable = parsed !== null && parsed.provider === node.attrs.provider;
  const editable = editor.isEditable;
  const [editing, setEditing] = useState(() => {
    const wanted = openNext && editable;
    openNext = false;
    return wanted;
  });
  const [draft, setDraft] = useState(url);
  const [invalid, setInvalid] = useState(false);
  const field = useRef<HTMLInputElement>(null);
  // Focus moves to the field one frame later: TipTap's `focus()` command (the slash menu
  // runs one before inserting) focuses the editor in a frame of its own, scheduled first.
  useEffect(() => {
    if (!editing) return;
    const frame = requestAnimationFrame(() => field.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [editing]);
  const fieldId = useId();
  const hintId = useId();
  const errorId = useId();

  const open = () => {
    setDraft(url);
    setInvalid(false);
    setEditing(true);
  };
  const close = () => {
    setEditing(false);
    editor.commands.focus();
  };
  const apply = () => {
    const next = embedFromUrl(draft);
    if (!next) {
      setInvalid(true);
      return;
    }
    if (next.url !== url || next.provider !== node.attrs.provider) updateAttributes(next);
    close();
  };
  const cancel = () => {
    // A fresh embed without a URL has nothing to keep.
    if (url === "") deleteNode();
    close();
  };

  const label = playable
    ? t(parsed.provider === "youtube" ? "embed.youtube" : "embed.vimeo")
    : t("embed.link");

  return (
    <NodeViewWrapper
      data-slot="editor-embed"
      className={cn("editor-html editor-embed", selected && "is-selected")}
    >
      <div className="editor-html-head" contentEditable={false}>
        <span className="inline-flex items-center gap-1.5">
          <SquarePlayIcon aria-hidden="true" className="size-3.5" />
          {label}
        </span>
        {editable && !editing ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 gap-1.5 px-2 text-xs"
            onClick={open}
          >
            <PencilIcon data-icon="inline-start" />
            {t("embed.change")}
          </Button>
        ) : null}
      </div>
      <div className="editor-embed-body" contentEditable={false}>
        {editing ? (
          <div className="flex flex-col gap-2" data-slot="embed-form">
            <Label htmlFor={fieldId}>{t("embed.urlLabel")}</Label>
            <Input
              id={fieldId}
              type="url"
              ref={field}
              value={draft}
              spellCheck={false}
              autoComplete="off"
              placeholder="https://www.youtube.com/watch?v=…"
              aria-invalid={invalid || undefined}
              aria-describedby={invalid ? `${errorId} ${hintId}` : hintId}
              onChange={(e) => {
                setDraft(e.target.value);
                setInvalid(false);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  apply();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  cancel();
                }
              }}
            />
            {invalid ? (
              <p id={errorId} className="text-sm text-destructive">
                {t("embed.invalid")}
              </p>
            ) : null}
            <p id={hintId} className="text-xs text-muted-foreground">
              {t("embed.hint")}
            </p>
            <div className="flex justify-end gap-2">
              <Button type="button" variant="outline" size="sm" onClick={cancel}>
                {t("embed.cancel")}
              </Button>
              <Button type="button" size="sm" onClick={apply}>
                {t("embed.apply")}
              </Button>
            </div>
          </div>
        ) : url === "" ? (
          <p className="text-sm text-muted-foreground">{t("embed.empty")}</p>
        ) : (
          <>
            <p className="truncate font-mono text-sm" title={url}>
              {url}
            </p>
            {playable ? null : (
              <p className="mt-1 text-xs text-muted-foreground">{t("embed.shownAsLink")}</p>
            )}
          </>
        )}
      </div>
    </NodeViewWrapper>
  );
}

export const EmbedBlock = Node.create({
  name: "embed",
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addAttributes() {
    return {
      provider: {
        default: null,
        parseHTML: (el: HTMLElement) => el.getAttribute("data-provider"),
        renderHTML: (attrs: { provider?: unknown }) =>
          typeof attrs.provider === "string" ? { "data-provider": attrs.provider } : {},
      },
      url: {
        default: "",
        parseHTML: (el: HTMLElement) => el.getAttribute("data-url") ?? "",
        renderHTML: (attrs: { url?: unknown }) => ({
          "data-url": typeof attrs.url === "string" ? attrs.url : "",
        }),
      },
    };
  },

  parseHTML() {
    return [{ tag: "div[data-ferrite-embed]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-ferrite-embed": "" }, HTMLAttributes)];
  },

  addCommands() {
    return {
      insertEmbed:
        () =>
        ({ commands, dispatch }) => {
          // Only a real insert opens the form, never a `can()` dry run (no `dispatch`).
          if (dispatch) openNext = true;
          const done = commands.insertContent({
            type: this.name,
            attrs: { provider: null, url: "" },
          });
          if (!done) openNext = false;
          return done;
        },
    };
  },

  addNodeView() {
    return ReactNodeViewRenderer(EmbedView);
  },
});
