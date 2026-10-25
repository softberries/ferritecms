// The selection bubble (11 §1, after Notion): bold, italic, strike, code, link, H2/H3
// and quote over a text selection. The link button turns the bubble into a URL field
// that accepts only what the site will emit (`checkHref`): `javascript:` and friends are
// refused with a message, never applied.

import type { Editor } from "@tiptap/core";
import type { EditorState } from "@tiptap/pm/state";
import { useEditorState } from "@tiptap/react";
import { BubbleMenu } from "@tiptap/react/menus";
import {
  BoldIcon,
  CheckIcon,
  CodeIcon,
  Heading2Icon,
  Heading3Icon,
  ItalicIcon,
  Link2OffIcon,
  LinkIcon,
  StrikethroughIcon,
  TextQuoteIcon,
  type LucideIcon,
} from "lucide-react";
import { cn } from "cn";
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { useTranslation } from "react-i18next";
import { checkHref } from "./schema";

interface Active {
  bold: boolean;
  italic: boolean;
  strike: boolean;
  code: boolean;
  link: boolean;
  h2: boolean;
  h3: boolean;
  quote: boolean;
}

function Tool({
  icon: Icon,
  label,
  pressed,
  onClick,
}: {
  icon: LucideIcon;
  label: string;
  pressed: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      onClick={onClick}
      className={cn(
        "inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground pointer-coarse:size-11",
        pressed && "bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary",
      )}
    >
      <Icon aria-hidden="true" className="size-4" />
    </button>
  );
}

function Divider() {
  return <span aria-hidden="true" className="mx-0.5 h-5 w-px bg-border" />;
}

/** The bubble's plugin key (its `updatePosition` meta goes to it). */
const BUBBLE_KEY = "editorBubble";

/**
 * `layoutKey` changes when the page around the editor moves without a scroll or resize
 * (focus mode hides the top bar): the bubble is then placed again, once at once and once
 * after the shell's transition.
 */
export function EditorBubbleMenu({ editor, layoutKey }: { editor: Editor; layoutKey?: unknown }) {
  const { t } = useTranslation("editor");
  const active = useEditorState({
    editor,
    selector: ({ editor: e }): Active => ({
      bold: e.isActive("bold"),
      italic: e.isActive("italic"),
      strike: e.isActive("strike"),
      code: e.isActive("code"),
      link: e.isActive("link"),
      h2: e.isActive("heading", { level: 2 }),
      h3: e.isActive("heading", { level: 3 }),
      quote: e.isActive("blockquote"),
    }),
  });
  useEffect(() => {
    const place = () => {
      if (!editor.isDestroyed) {
        editor.view.dispatch(editor.state.tr.setMeta(BUBBLE_KEY, "updatePosition"));
      }
    };
    const frame = requestAnimationFrame(place);
    const later = setTimeout(place, 260);
    return () => {
      cancelAnimationFrame(frame);
      clearTimeout(later);
    };
  }, [editor, layoutKey]);
  const [linking, setLinking] = useState(false);
  const [url, setUrl] = useState("");
  const [refused, setRefused] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const errorId = useId();

  const c = () => editor.chain().focus();
  const shouldShow = useCallback(
    ({
      editor: e,
      from,
      to,
      state,
    }: {
      editor: Editor;
      from: number;
      to: number;
      state: EditorState;
    }) => {
      if (!e.isEditable || from === to) return false;
      if (e.isActive("codeBlock") || e.isActive("image") || e.isActive("html")) return false;
      return state.doc.textBetween(from, to).trim().length > 0;
    },
    [],
  );
  // Stable, so the menu plugin is not sent new options on every render.
  const options = useMemo(
    () => ({
      placement: "top" as const,
      offset: 8,
      flip: true,
      shift: { padding: 8 },
      onHide: () => {
        setLinking(false);
        setRefused(false);
      },
    }),
    [],
  );

  const startLink = () => {
    const href: unknown = editor.getAttributes("link").href;
    setUrl(typeof href === "string" ? href : "");
    setRefused(false);
    setLinking(true);
    requestAnimationFrame(() => input.current?.focus());
  };
  const closeLink = () => {
    setLinking(false);
    setRefused(false);
    editor.commands.focus();
  };
  const applyLink = () => {
    const check = checkHref(url);
    if (!check.ok) {
      setRefused(true);
      input.current?.focus();
      return;
    }
    c().extendMarkRange("link").setLink({ href: check.href }).run();
    setLinking(false);
    setRefused(false);
  };
  const removeLink = () => {
    c().extendMarkRange("link").unsetLink().run();
    setLinking(false);
  };
  const onLinkKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      applyLink();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeLink();
    }
  };

  return (
    <BubbleMenu
      editor={editor}
      pluginKey={BUBBLE_KEY}
      options={options}
      shouldShow={shouldShow}
      className="editor-menu z-40"
      data-slot="bubble-menu"
    >
      {linking ? (
        <div className="flex w-[min(22rem,calc(100vw-2rem))] flex-col gap-1 p-1">
          <div className="flex items-center gap-1">
            <LinkIcon aria-hidden="true" className="ml-1.5 size-4 shrink-0 text-muted-foreground" />
            <input
              ref={input}
              type="text"
              inputMode="url"
              autoComplete="off"
              spellCheck={false}
              aria-label={t("bubble.linkInput")}
              aria-invalid={refused || undefined}
              aria-describedby={refused ? errorId : undefined}
              placeholder={t("bubble.linkPlaceholder")}
              value={url}
              onChange={(e) => {
                setUrl(e.target.value);
                setRefused(false);
              }}
              onKeyDown={onLinkKey}
              className="h-8 min-w-0 flex-1 rounded-md bg-transparent px-1.5 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none pointer-coarse:h-11"
            />
            <Tool
              icon={CheckIcon}
              label={t("bubble.linkApply")}
              pressed={false}
              onClick={applyLink}
            />
            {active.link ? (
              <Tool
                icon={Link2OffIcon}
                label={t("bubble.linkRemove")}
                pressed={false}
                onClick={removeLink}
              />
            ) : null}
          </div>
          {refused ? (
            <p id={errorId} role="alert" className="px-2 pb-1 text-xs text-destructive">
              {t("bubble.linkRefused")}
            </p>
          ) : null}
        </div>
      ) : (
        <div
          role="toolbar"
          aria-label={t("bubble.label")}
          className="flex items-center gap-0.5 p-1"
        >
          <Tool
            icon={BoldIcon}
            label={t("bubble.bold")}
            pressed={active.bold}
            onClick={() => c().toggleBold().run()}
          />
          <Tool
            icon={ItalicIcon}
            label={t("bubble.italic")}
            pressed={active.italic}
            onClick={() => c().toggleItalic().run()}
          />
          <Tool
            icon={StrikethroughIcon}
            label={t("bubble.strike")}
            pressed={active.strike}
            onClick={() => c().toggleStrike().run()}
          />
          <Tool
            icon={CodeIcon}
            label={t("bubble.code")}
            pressed={active.code}
            onClick={() => c().toggleCode().run()}
          />
          <Tool
            icon={LinkIcon}
            label={t("bubble.link")}
            pressed={active.link}
            onClick={startLink}
          />
          <Divider />
          <Tool
            icon={Heading2Icon}
            label={t("bubble.h2")}
            pressed={active.h2}
            onClick={() => c().toggleHeading({ level: 2 }).run()}
          />
          <Tool
            icon={Heading3Icon}
            label={t("bubble.h3")}
            pressed={active.h3}
            onClick={() => c().toggleHeading({ level: 3 }).run()}
          />
          <Tool
            icon={TextQuoteIcon}
            label={t("bubble.quote")}
            pressed={active.quote}
            onClick={() => c().toggleBlockquote().run()}
          />
        </div>
      )}
    </BubbleMenu>
  );
}
