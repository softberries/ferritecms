// The `/` insert menu (11 §1, after Notion and Ghost): typing `/` at the start of a line
// or after a space opens a list of blocks, filtered by what follows; ↑/↓ move, Enter or
// Tab inserts, Esc closes. Built on `@tiptap/suggestion` with managed positioning
// (`props.mount`). Focus stays in the editor, which points at the highlighted option
// through `aria-activedescendant`.

import { Extension, type Editor, type Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import { ReactRenderer } from "@tiptap/react";
import {
  Suggestion,
  exitSuggestion,
  type SuggestionKeyDownProps,
  type SuggestionProps,
} from "@tiptap/suggestion";
import {
  CodeXmlIcon,
  Heading2Icon,
  Heading3Icon,
  Heading4Icon,
  ListIcon,
  ListOrderedIcon,
  SeparatorHorizontalIcon,
  SquareCodeIcon,
  TableIcon,
  TextQuoteIcon,
  type LucideIcon,
} from "lucide-react";
import { cn } from "cn";
import { useEffect, useImperativeHandle, useState, type Ref } from "react";
import { useTranslation } from "react-i18next";
import { i18n } from "@/i18n";

export const slashMenuKey = new PluginKey("slashMenu");

type SlashId =
  | "heading2"
  | "heading3"
  | "heading4"
  | "bulletList"
  | "orderedList"
  | "quote"
  | "codeBlock"
  | "table"
  | "divider"
  | "html";

interface SlashItem {
  id: SlashId;
  icon: LucideIcon;
  run: (editor: Editor, range: Range) => void;
}

const chain = (editor: Editor, range: Range) => editor.chain().focus().deleteRange(range);

const ITEMS: SlashItem[] = [
  {
    id: "heading2",
    icon: Heading2Icon,
    run: (e, r) => chain(e, r).setNode("heading", { level: 2 }).run(),
  },
  {
    id: "heading3",
    icon: Heading3Icon,
    run: (e, r) => chain(e, r).setNode("heading", { level: 3 }).run(),
  },
  {
    id: "heading4",
    icon: Heading4Icon,
    run: (e, r) => chain(e, r).setNode("heading", { level: 4 }).run(),
  },
  { id: "bulletList", icon: ListIcon, run: (e, r) => chain(e, r).toggleBulletList().run() },
  {
    id: "orderedList",
    icon: ListOrderedIcon,
    run: (e, r) => chain(e, r).toggleOrderedList().run(),
  },
  { id: "quote", icon: TextQuoteIcon, run: (e, r) => chain(e, r).toggleBlockquote().run() },
  { id: "codeBlock", icon: SquareCodeIcon, run: (e, r) => chain(e, r).setCodeBlock().run() },
  {
    id: "table",
    icon: TableIcon,
    run: (e, r) => chain(e, r).insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run(),
  },
  {
    id: "divider",
    icon: SeparatorHorizontalIcon,
    run: (e, r) => chain(e, r).setHorizontalRule().run(),
  },
  { id: "html", icon: CodeXmlIcon, run: (e, r) => chain(e, r).insertHtmlBlock().run() },
];

/** The items whose label or keywords contain every word of `query`. */
function filterItems(query: string): SlashItem[] {
  const words = query.toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return ITEMS;
  return ITEMS.filter((item) => {
    const hay =
      `${i18n.t(`editor:slash.${item.id}.label`)} ${i18n.t(`editor:slash.${item.id}.keywords`)}`.toLocaleLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

interface MenuHandle {
  onKeyDown: (props: SuggestionKeyDownProps) => boolean;
}

type MenuProps = SuggestionProps<SlashItem, SlashItem> & { ref?: Ref<MenuHandle> };

const optionId = (id: SlashId) => `slash-option-${id}`;

function SlashMenuList({ items, query, command, editor, ref }: MenuProps) {
  const { t } = useTranslation("editor");
  // The highlight resets when the query changes (no state update inside an effect).
  const [sel, setSel] = useState({ query, index: 0 });
  const index = sel.query === query ? Math.min(sel.index, Math.max(items.length - 1, 0)) : 0;
  const active = items[index];

  const move = (delta: number) => {
    if (items.length === 0) return;
    setSel({ query, index: (index + delta + items.length) % items.length });
  };

  useImperativeHandle(ref, () => ({
    onKeyDown: ({ event }) => {
      if (event.key === "ArrowDown") {
        move(1);
        return true;
      }
      if (event.key === "ArrowUp") {
        move(-1);
        return true;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        if (!active) return false;
        command(active);
        return true;
      }
      return false;
    },
  }));

  // The editor (which keeps focus) names the highlighted option.
  const activeId = active ? optionId(active.id) : null;
  useEffect(() => {
    const dom = editor.view.dom;
    if (activeId) dom.setAttribute("aria-activedescendant", activeId);
    else dom.removeAttribute("aria-activedescendant");
    return () => dom.removeAttribute("aria-activedescendant");
  }, [editor, activeId]);

  return (
    <div data-slot="slash-menu" className="editor-menu w-72 p-1">
      <p className="px-2 pt-1.5 pb-1 text-xs font-medium text-muted-foreground" aria-hidden="true">
        {t("slash.heading")}
      </p>
      {items.length === 0 ? (
        <p className="px-2 py-2 text-sm text-muted-foreground" role="status">
          {t("slash.empty")}
        </p>
      ) : (
        <div role="listbox" aria-label={t("slash.label")}>
          {items.map((item, i) => {
            const Icon = item.icon;
            const selected = i === index;
            return (
              <div
                key={item.id}
                id={optionId(item.id)}
                role="option"
                aria-selected={selected}
                data-selected={selected || undefined}
                className={cn(
                  "flex cursor-pointer items-center gap-3 rounded-md px-2 py-1.5 select-none",
                  selected && "bg-muted",
                )}
                onMouseDown={(e) => e.preventDefault()}
                onMouseMove={() => {
                  if (!selected) setSel({ query, index: i });
                }}
                onClick={() => command(item)}
              >
                <span className="flex size-8 shrink-0 items-center justify-center rounded-md border bg-card text-muted-foreground">
                  <Icon aria-hidden="true" className="size-4" />
                </span>
                <span className="min-w-0">
                  <span className="block text-sm font-medium text-foreground">
                    {t(`slash.${item.id}.label`)}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {t(`slash.${item.id}.hint`)}
                  </span>
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/** The `/` trigger: not inside code blocks, and only while the editor is editable. */
export const SlashCommand = Extension.create({
  name: "slashCommand",

  addProseMirrorPlugins() {
    return [
      Suggestion<SlashItem, SlashItem>({
        editor: this.editor,
        pluginKey: slashMenuKey,
        char: "/",
        placement: "bottom-start",
        offset: { mainAxis: 6 },
        allow: ({ editor, state, range }) =>
          editor.isEditable && state.doc.resolve(range.from).parent.type.name !== "codeBlock",
        items: ({ query }) => filterItems(query),
        command: ({ editor, range, props }) => props.run(editor, range),
        render: () => {
          let component: ReactRenderer<MenuHandle, MenuProps> | null = null;
          let unmount: (() => void) | null = null;
          return {
            onStart: (props) => {
              component = new ReactRenderer(SlashMenuList, { props, editor: props.editor });
              component.element.classList.add("editor-menu-host");
              unmount = props.mount(component.element);
            },
            onUpdate: (props) => {
              component?.updateProps(props);
            },
            onKeyDown: (props) => {
              if (props.event.key === "Escape") {
                exitSuggestion(props.view, slashMenuKey);
                return true;
              }
              return component?.ref?.onKeyDown(props) ?? false;
            },
            onExit: () => {
              unmount?.();
              component?.destroy();
              unmount = null;
              component = null;
            },
          };
        },
      }),
    ];
  },
});
