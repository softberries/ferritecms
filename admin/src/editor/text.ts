// Plain text of a block document: the TypeScript counterpart of core
// `blocks::extract_text` (`crates/ferrite-core/src/blocks/text.rs`), for the word count
// and the revision diff. Paragraphs, headings, list items, code blocks, table rows and
// cells end a line; hard breaks and horizontal rules break one; nodes deeper than
// `MAX_DEPTH` contribute their text only; trailing newlines are trimmed. Iterative.

import { wordCount } from "./word-count";

export { wordCount };

/** `blocks::MAX_DEPTH`. */
export const MAX_DEPTH = 64;

const LINE_ENDING = new Set([
  "paragraph",
  "heading",
  "listItem",
  "codeBlock",
  "tableRow",
  "tableCell",
  "tableHeader",
]);

type Item = { node: unknown; depth: number } | "break";

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function children(node: Record<string, unknown>): unknown[] {
  return Array.isArray(node.content) ? (node.content as unknown[]) : [];
}

/** All text under `root`, concatenated without separators (`collect_text`). */
function collectText(root: unknown): string {
  let out = "";
  const stack: unknown[] = [root];
  while (stack.length > 0) {
    const v = stack.pop();
    if (Array.isArray(v)) {
      for (let i = v.length - 1; i >= 0; i--) stack.push(v[i]);
    } else if (isObject(v)) {
      if (v.type === "text") {
        if (typeof v.text === "string") out += v.text;
      } else {
        const kids = children(v);
        for (let i = kids.length - 1; i >= 0; i--) stack.push(kids[i]);
      }
    }
  }
  return out;
}

export function blockText(doc: unknown): string {
  let out = "";
  const pushBreak = () => {
    if (out !== "" && !out.endsWith("\n")) out += "\n";
  };
  const stack: Item[] = [{ node: doc, depth: 0 }];
  while (stack.length > 0) {
    const item = stack.pop();
    if (item === undefined) break;
    if (item === "break") {
      pushBreak();
      continue;
    }
    const { node, depth } = item;
    if (depth > MAX_DEPTH) {
      out += collectText(node);
      continue;
    }
    if (Array.isArray(node)) {
      for (let i = node.length - 1; i >= 0; i--) stack.push({ node: node[i], depth: depth + 1 });
    } else if (isObject(node)) {
      const type = typeof node.type === "string" ? node.type : "";
      if (type === "text") {
        if (typeof node.text === "string") out += node.text;
      } else if (type === "hardBreak" || type === "horizontalRule") {
        pushBreak();
      } else {
        if (LINE_ENDING.has(type)) stack.push("break");
        const kids = children(node);
        for (let i = kids.length - 1; i >= 0; i--) stack.push({ node: kids[i], depth: depth + 1 });
      }
    }
  }
  return out.replace(/\n+$/, "");
}
