import { i18n } from "@/i18n";
import { expect, test } from "vitest";
import { entryText } from "./entry-text";
import type { MenuEntry, MenuNode } from "./menu-model";

const t = i18n.getFixedT("en", "structure");
const node = (data: Partial<MenuEntry> & Pick<MenuEntry, "kind">): MenuNode => ({
  id: "n1",
  parentId: null,
  data: { targetId: "t1", url: null, label: "", ...data },
});
const about = { state: "found", title: "About us", kind: "Pages" } as const;

test("empty_label_rows_do_not_repeat_the_title", () => {
  // No label: the title shows muted on the first line, the kind alone under it.
  expect(entryText(node({ kind: "item" }), about, t)).toEqual({
    title: "About us",
    fallback: true,
    line: "Pages",
  });
  expect(
    entryText(node({ kind: "url", targetId: null, url: "https://x.example/" }), undefined, t),
  ).toEqual({ title: "https://x.example/", fallback: true, line: "Custom link" });
  expect(entryText(node({ kind: "item" }), { state: "missing" }, t)).toEqual({
    title: t("menus.editor.targetMissing"),
    fallback: true,
    line: "Content",
  });
  // An own label: the target under it, as before.
  expect(entryText(node({ kind: "item", label: "Team" }), about, t)).toEqual({
    title: "Team",
    fallback: false,
    line: "Pages · About us",
  });
  expect(
    entryText(
      node({ kind: "url", targetId: null, url: "https://x.example/", label: "X" }),
      undefined,
      t,
    ),
  ).toEqual({ title: "X", fallback: false, line: "Custom link · https://x.example/" });
});
