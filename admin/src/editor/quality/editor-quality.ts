// The editor screen's checks (design 16 §11.1), in one hook so the screen only passes
// what it returns on: runs the checks on the TipTap document, keeps the inline marks in
// step, and gives the panel and the publish dialog what they show.

import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { suppress, unsuppress, type CheckRequest, type FindingDto } from "@/api/quality";
import type { EditorDocument } from "../editor-state";
import type { QualitySectionProps } from "../panel/quality-section";
import { qualityKey, qualityPlugin } from "./decorations";
import { findingPos, messageFor } from "./findings";
import { useQualityCheck, type QualityState } from "./use-quality-check";

/** `?check=<rule>:<key>` of the current address (the health report's links). */
function focusFromUrl(): string | null {
  if (typeof window === "undefined") return null;
  return new URLSearchParams(window.location.search).get("check");
}

export interface EditorQuality {
  state: QualityState;
  panel: QualitySectionProps;
}

export function useEditorQuality(
  doc: EditorDocument,
  type: string,
  onShowPanel: () => void,
): EditorQuality {
  const { t } = useTranslation("quality");
  const queryClient = useQueryClient();
  const editor = doc.editor;
  const docRef = useRef(doc);
  useEffect(() => {
    docRef.current = doc;
  });
  const state = useQualityCheck({
    type,
    enabled: type !== "media",
    draft: () => {
      const d = docRef.current.draft();
      return {
        ...(docRef.current.item ? { id: docRef.current.item.id } : {}),
        title: d.title,
        // The generated type of a free-form object (`Record<string, never>`).
        body: d.body as CheckRequest["body"],
        fields: (d.fields ?? {}) as NonNullable<CheckRequest["fields"]>,
      };
    },
    subscribe: (onChange) => {
      editor.on("update", onChange);
      return () => {
        editor.off("update", onChange);
      };
    },
  });

  const show = (f: FindingDto) => {
    if (f.location.kind !== "body" || !f.location.path || editor.isDestroyed) return;
    const view = editor.view;
    const pos = findingPos(view.state.doc, f.location.path);
    if (pos === null) return;
    const node = view.state.doc.nodeAt(pos);
    if (!node) return;
    const sel = node.isText
      ? TextSelection.create(view.state.doc, pos, pos + node.nodeSize)
      : NodeSelection.create(view.state.doc, pos);
    view.dispatch(view.state.tr.setSelection(sel).scrollIntoView());
    view.focus();
    if (f.rule.startsWith("image-alt")) {
      // The image's alt button opens its popover.
      window.setTimeout(() => {
        const dom = view.nodeDOM(pos);
        const button =
          dom instanceof HTMLElement
            ? dom.querySelector<HTMLButtonElement>("[data-slot=image-alt] button")
            : null;
        button?.click();
      }, 50);
    }
  };

  // The marks: the plugin once per editor, the findings with every report.
  useEffect(() => {
    if (editor.isDestroyed) return;
    const plugin = qualityPlugin({
      label: (f) =>
        t("mark.label", { severity: t(`severity.${f.severity}`), message: messageFor(t, f) }),
      onActivate: () => onShowPanel(),
    });
    editor.registerPlugin(plugin);
    return () => {
      if (!editor.isDestroyed) editor.unregisterPlugin(qualityKey);
    };
    // `t` and `onShowPanel` are stable enough: the labels follow the next report.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor]);
  useEffect(() => {
    if (editor.isDestroyed || !state.report) return;
    const tr = editor.view.state.tr.setMeta(qualityKey, state.report.findings);
    tr.setMeta("addToHistory", false);
    editor.view.dispatch(tr);
  }, [editor, state.report]);

  const item = doc.item;
  const panel: QualitySectionProps = {
    report: state.report,
    failed: state.failed,
    canSuppress: item !== null && item.allowed.edit === true,
    focus: focusFromUrl(),
    onShow: show,
    onSuppress: async (f, input) => {
      if (!item) return;
      await suppress(type, item.id, { rule: f.rule, ...input });
      await state.checkNow();
      void queryClient.invalidateQueries({ queryKey: ["health"] });
    },
    onUnsuppress: async (f) => {
      if (!item || !f.suppressed) return;
      await unsuppress(type, item.id, f.rule, f.suppressed.key);
      await state.checkNow();
      void queryClient.invalidateQueries({ queryKey: ["health"] });
    },
  };
  return { state, panel };
}
