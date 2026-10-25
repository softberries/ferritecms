// The `html` node: an atom block whose `html` attribute holds raw markup. The site
// sanitises it with ammonia when it renders the page (06 §5); the editor never renders
// it, it shows the source as a read-only code preview, edited in a sheet.

import { Node, mergeAttributes } from "@tiptap/core";
import { NodeViewWrapper, ReactNodeViewRenderer, type ReactNodeViewProps } from "@tiptap/react";
import { CodeXmlIcon, PencilIcon } from "lucide-react";
import { cn } from "cn";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    htmlBlock: {
      /** Inserts an empty HTML block and opens its source sheet. */
      insertHtmlBlock: () => ReturnType;
    };
  }
}

/** Set by `insertHtmlBlock` so the next HTML block view to mount opens its sheet. */
let openNext = false;

function HtmlView({ node, selected, editor, updateAttributes }: ReactNodeViewProps) {
  const { t } = useTranslation("editor");
  const html = typeof node.attrs.html === "string" ? node.attrs.html : "";
  const [open, setOpen] = useState(() => {
    const wanted = openNext && editor.isEditable;
    openNext = false;
    return wanted;
  });
  const [draft, setDraft] = useState(html);
  const editable = editor.isEditable;

  const openSheet = () => {
    setDraft(html);
    setOpen(true);
  };

  return (
    <NodeViewWrapper
      data-slot="editor-html"
      className={cn("editor-html", selected && "is-selected")}
    >
      <div className="editor-html-head" contentEditable={false}>
        <span className="inline-flex items-center gap-1.5">
          <CodeXmlIcon aria-hidden="true" className="size-3.5" />
          {t("html.label")}
        </span>
        {editable ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 gap-1.5 px-2 text-xs"
            onClick={openSheet}
          >
            <PencilIcon data-icon="inline-start" />
            {t("html.edit")}
          </Button>
        ) : null}
      </div>
      <pre className="editor-html-code" contentEditable={false}>
        {html ? (
          <code>{html}</code>
        ) : (
          <span className="text-muted-foreground">{t("html.empty")}</span>
        )}
      </pre>
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent
          side="right"
          className="w-full gap-0 sm:max-w-xl"
          onCloseAutoFocus={(e) => {
            // Back to the writing surface, not to <body> (a fresh block has no trigger).
            e.preventDefault();
            editor.commands.focus();
          }}
        >
          <SheetHeader>
            <SheetTitle>{t("html.sheetTitle")}</SheetTitle>
            <SheetDescription>{t("html.sheetDescription")}</SheetDescription>
          </SheetHeader>
          <div className="flex min-h-0 flex-1 flex-col px-4">
            <Textarea
              aria-label={t("html.sourceLabel")}
              value={draft}
              spellCheck={false}
              autoCapitalize="off"
              autoComplete="off"
              onChange={(e) => setDraft(e.target.value)}
              className="min-h-0 flex-1 resize-none font-mono text-sm leading-6"
            />
          </div>
          <SheetFooter className="flex-row justify-end">
            <Button type="button" variant="outline" onClick={() => setOpen(false)}>
              {t("html.cancel")}
            </Button>
            <Button
              type="button"
              onClick={() => {
                updateAttributes({ html: draft });
                setOpen(false);
              }}
            >
              {t("html.apply")}
            </Button>
          </SheetFooter>
        </SheetContent>
      </Sheet>
    </NodeViewWrapper>
  );
}

export const HtmlBlock = Node.create({
  name: "html",
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addAttributes() {
    return {
      html: {
        default: "",
        parseHTML: (el: HTMLElement) => el.getAttribute("data-html") ?? "",
        renderHTML: (attrs: { html?: unknown }) => ({
          "data-html": typeof attrs.html === "string" ? attrs.html : "",
        }),
      },
    };
  },

  parseHTML() {
    return [{ tag: "div[data-ferrite-html]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-ferrite-html": "" }, HTMLAttributes)];
  },

  addCommands() {
    return {
      insertHtmlBlock:
        () =>
        ({ commands }) => {
          openNext = true;
          const done = commands.insertContent({ type: this.name, attrs: { html: "" } });
          if (!done) openNext = false;
          return done;
        },
    };
  },

  addNodeView() {
    return ReactNodeViewRenderer(HtmlView);
  },
});
