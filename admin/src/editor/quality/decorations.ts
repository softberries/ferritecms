// The inline marks of the editor's findings (design 16 §11.1): a ProseMirror plugin that
// maps each body finding's `path` to its node and decorates it (a wavy underline for
// text, an outline for images and tables) with a small button whose `aria-label` says
// what is wrong, so the cue is never colour alone (WCAG 1.4.1). Findings arrive through
// a transaction meta; the decorations follow edits until the next check replaces them.

import { Plugin, PluginKey, type EditorState } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import type { FindingDto } from "@/api/quality";
import { findingId, findingPos } from "./findings";

export const qualityKey = new PluginKey<DecorationSet>("ferrite-quality");

export interface QualityMarkOptions {
  /** The widget's accessible name for a finding. */
  label: (f: FindingDto) => string;
  /** The widget was pressed: show the finding in the panel. */
  onActivate: (f: FindingDto) => void;
}

/** The decorations of `findings` (body ones, not suppressed) in `state`'s document. */
export function decorationsFor(
  state: EditorState,
  findings: readonly FindingDto[],
  options: QualityMarkOptions,
): DecorationSet {
  const decos: Decoration[] = [];
  for (const f of findings) {
    if (f.suppressed || f.location.kind !== "body" || !f.location.path) continue;
    const pos = findingPos(state.doc, f.location.path);
    if (pos === null) continue;
    const node = state.doc.nodeAt(pos);
    if (!node) continue;
    const tone = f.severity === "error" ? "is-error" : "is-warning";
    const attrs = { class: `quality-mark ${tone}`, "data-finding": findingId(f) };
    if (node.isText) {
      decos.push(Decoration.inline(pos, pos + node.nodeSize, attrs));
    } else {
      decos.push(Decoration.node(pos, pos + node.nodeSize, attrs));
    }
    decos.push(
      Decoration.widget(
        pos,
        () => {
          const b = document.createElement("button");
          b.type = "button";
          b.className = `quality-widget ${tone}`;
          b.setAttribute("aria-label", options.label(f));
          b.setAttribute("data-slot", "quality-widget");
          b.contentEditable = "false";
          b.textContent = "!";
          b.addEventListener("mousedown", (e) => e.preventDefault());
          b.addEventListener("click", (e) => {
            e.preventDefault();
            options.onActivate(f);
          });
          return b;
        },
        { side: -1, key: `q-${findingId(f)}`, ignoreSelection: true },
      ),
    );
  }
  return DecorationSet.create(state.doc, decos);
}

/** The plugin; set findings with `tr.setMeta(qualityKey, findings)`. */
export function qualityPlugin(options: QualityMarkOptions): Plugin<DecorationSet> {
  return new Plugin<DecorationSet>({
    key: qualityKey,
    state: {
      init: () => DecorationSet.empty,
      apply(tr, set, _old, state) {
        const next = tr.getMeta(qualityKey) as readonly FindingDto[] | undefined;
        if (next) return decorationsFor(state, next, options);
        return set.map(tr.mapping, tr.doc);
      },
    },
    props: {
      decorations: (state) => qualityKey.getState(state) ?? DecorationSet.empty,
    },
  });
}
