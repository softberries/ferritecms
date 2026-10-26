// Words and reading time of the body, live: the editor footer (from 1024 px) and the
// phone publish bar (below it, Task 15 carry) both show them.

import type { Editor } from "@tiptap/react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { uiLocale } from "@/lib/locale";
import { blockText } from "./text";
import { readingMinutes, wordCount } from "./word-count";

/** The body's words, recounted at most once a frame while it changes. */
function useWordCount(editor: Editor): number {
  const [words, setWords] = useState(() => wordCount(blockText(editor.getJSON())));
  useEffect(() => {
    let frame = 0;
    const onUpdate = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setWords(wordCount(blockText(editor.getJSON()))));
    };
    editor.on("update", onUpdate);
    return () => {
      cancelAnimationFrame(frame);
      editor.off("update", onUpdate);
    };
  }, [editor]);
  return words;
}

/** "312 words · 2 min read"; `fixedText` counts a body the editor does not show. */
export function WordStats({ editor, fixedText }: { editor: Editor; fixedText?: string }) {
  const { t } = useTranslation("editor");
  const live = useWordCount(editor);
  const words = fixedText === undefined ? live : wordCount(fixedText);
  const minutes = readingMinutes(words);
  const n = new Intl.NumberFormat(uiLocale());
  return (
    <>
      <span>{t("footer.words", { count: words, formatted: n.format(words) })}</span>
      <span aria-hidden="true">·</span>
      <span>{t("footer.minutes", { count: minutes, formatted: n.format(minutes) })}</span>
    </>
  );
}
