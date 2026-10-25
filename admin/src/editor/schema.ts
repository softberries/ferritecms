// The editor's document model: exactly the nodes and marks the core renderer handles
// (`crates/ferrite-core/src/blocks/render.rs`; M2b P19), so every body the editor saves
// renders on the site as it looked here, and a body with anything else opens read-only
// instead of being rewritten (Review Focus 3). Body headings are levels 2–6; the title is
// the page's h1 (P18). No underline (the renderer has none).
//
// TipTap defaults that would rewrite stored content are switched off (pre-flight F32):
// the link mark's `target`, `rel`, `class` and `title` have no default (a stored link
// without `target` stays without one, so the site never starts opening it in a new tab),
// and StarterKit's `trailingNode` is off (it appends an empty paragraph after a final
// non-paragraph block).

import type { Extensions } from "@tiptap/core";
import { Link as TiptapLink } from "@tiptap/extension-link";
import { TableKit } from "@tiptap/extension-table";
import { Placeholder } from "@tiptap/extensions";
import { StarterKit } from "@tiptap/starter-kit";
import { HtmlBlock } from "./html-node";
import { MediaImage } from "./image-node";
import { SlashCommand } from "./slash-menu";

/** Every node type the core renderer knows (`render_object`). */
export const SUPPORTED_NODES = [
  "doc",
  "paragraph",
  "heading",
  "bulletList",
  "orderedList",
  "listItem",
  "blockquote",
  "codeBlock",
  "horizontalRule",
  "hardBreak",
  "image",
  "table",
  "tableRow",
  "tableHeader",
  "tableCell",
  "html",
  "text",
] as const;

/** Every mark the core renderer knows (`render_text`). */
export const SUPPORTED_MARKS = ["bold", "italic", "strike", "code", "link"] as const;

/** Heading levels of a body; level 1 is the title (P18). */
export const HEADING_LEVELS = [2, 3, 4, 5, 6] as const;

/** Schemes a link may use (the site's `SAFE_URL_SCHEMES`). */
export const LINK_PROTOCOLS = ["http", "https", "mailto"];

const NODE_SET = new Set<string>(SUPPORTED_NODES);
const MARK_SET = new Set<string>(SUPPORTED_MARKS);

export interface EditorExtensionOptions {
  /** Shown in an empty body. */
  placeholder: string;
  /** `ferrite:media/<uuid>` → the URL to show, or `null` when the media is gone. */
  resolveMedia: (id: string) => Promise<string | null>;
}

/** No default: an absent attribute stays absent in `getJSON()` (F32). */
const NO_DEFAULT = { default: undefined };

export function editorExtensions(opts: EditorExtensionOptions): Extensions {
  return [
    StarterKit.configure({
      underline: false,
      trailingNode: false,
      heading: { levels: [...HEADING_LEVELS] },
      // The link mark comes from `LinkMark` below (F32 attribute defaults).
      link: false,
    }),
    LinkMark.configure({
      openOnClick: false,
      autolink: true,
      protocols: LINK_PROTOCOLS,
      HTMLAttributes: { target: null, rel: null, class: null },
      isAllowedUri: (url) => checkHref(url).ok,
      shouldAutoLink: (url) => checkHref(url).ok,
    }),
    TableKit.configure({ table: { resizable: false } }),
    MediaImage.configure({ inline: false, allowBase64: false, resolveMedia: opts.resolveMedia }),
    HtmlBlock,
    Placeholder.configure({
      placeholder: ({ editor }) => (editor.isEmpty ? opts.placeholder : ""),
    }),
    SlashCommand,
  ];
}

// `@tiptap/extension-link` (StarterKit's own, pinned directly at the same version) is
// extended here so its extra attributes have no default (F32); StarterKit's copy is off.
const LinkMark = TiptapLink.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      target: NO_DEFAULT,
      rel: NO_DEFAULT,
      class: NO_DEFAULT,
      title: NO_DEFAULT,
    };
  },
});

// --- validateBody ---------------------------------------------------------------------

export type BodyCheck = { ok: true } | { ok: false; unknown: string[] };

/** The label of a level-1 (or missing, or out-of-range) heading in `unknown`. */
export function headingLabel(level: unknown): string {
  return `heading (level ${typeof level === "number" ? String(level) : "none"})`;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function headingLevelOk(node: Record<string, unknown>): boolean {
  const attrs = node.attrs;
  const level = isObject(attrs) ? attrs.level : undefined;
  return typeof level === "number" && (HEADING_LEVELS as readonly number[]).includes(level);
}

/**
 * A root that is not `{ type: "doc" }` is unknown too (`(root: paragraph)`).
 * Walks `doc` for any node type or mark outside the supported sets, and for a heading
 * whose level is not 2–6 (level 1 included, P18; a missing level too, F32). `null`, an
 * empty object and a document without content are an empty body. Iterative: any depth.
 */
export function validateBody(doc: unknown): BodyCheck {
  if (doc === null || doc === undefined || (isObject(doc) && Object.keys(doc).length === 0)) {
    return { ok: true };
  }
  // The root must be a document: the renderer renders any node, so a body stored as a
  // bare paragraph (an API client's) shows on the site and must not be replaced.
  if (!isObject(doc))
    return { ok: false, unknown: [`(root: ${Array.isArray(doc) ? "list" : typeof doc})`] };
  if (doc.type !== "doc") {
    return {
      ok: false,
      unknown: [`(root: ${typeof doc.type === "string" ? doc.type : "no type"})`],
    };
  }
  const unknown = new Set<string>();
  const stack: unknown[] = [doc];
  while (stack.length > 0) {
    const v = stack.pop();
    if (!isObject(v)) {
      unknown.add(Array.isArray(v) ? "(list)" : "(not a node)");
      continue;
    }
    const type = typeof v.type === "string" ? v.type : "";
    if (!NODE_SET.has(type)) unknown.add(type || "(no type)");
    else if (type === "heading" && !headingLevelOk(v)) {
      unknown.add(headingLabel(isObject(v.attrs) ? v.attrs.level : undefined));
    }
    if (v.marks !== undefined) {
      if (!Array.isArray(v.marks)) unknown.add("(marks)");
      else {
        for (const m of v.marks as unknown[]) {
          const mt = isObject(m) && typeof m.type === "string" ? m.type : "";
          if (!MARK_SET.has(mt)) unknown.add(mt ? `${mt} (mark)` : "(mark)");
        }
      }
    }
    if (v.content !== undefined) {
      if (!Array.isArray(v.content)) unknown.add("(content)");
      else for (let i = v.content.length - 1; i >= 0; i--) stack.push(v.content[i]);
    }
  }
  return unknown.size === 0 ? { ok: true } : { ok: false, unknown: [...unknown] };
}

// --- displayBody ----------------------------------------------------------------------

const TEXTBLOCKS = new Set(["paragraph", "heading", "codeBlock"]);
const BLOCKS = new Set<string>(
  SUPPORTED_NODES.filter((n) => n !== "doc" && n !== "text" && n !== "hardBreak"),
);

function textOf(node: unknown): string {
  let out = "";
  const stack: unknown[] = [node];
  while (stack.length > 0) {
    const v = stack.pop();
    if (Array.isArray(v)) {
      for (let i = v.length - 1; i >= 0; i--) stack.push(v[i]);
    } else if (isObject(v)) {
      if (v.type === "text" && typeof v.text === "string") out += v.text;
      else if (Array.isArray(v.content)) stack.push(v.content);
    }
  }
  return out;
}

function cleanMarks(marks: unknown): unknown[] | undefined {
  if (!Array.isArray(marks)) return undefined;
  const kept = marks.filter((m) => isObject(m) && MARK_SET.has(String(m.type)));
  return kept.length > 0 ? kept : undefined;
}

/**
 * A copy of a body the editor cannot represent, made displayable for the read-only view
 * only (it is never saved): an unknown block becomes its known children or a paragraph
 * of its text (as the site renders it), an unknown inline node its text, an unknown
 * mark is dropped and a heading outside 2–6 shows as level 2.
 */
export function displayBody(doc: unknown): unknown {
  const convert = (node: unknown, inline: boolean): unknown[] => {
    if (!isObject(node)) return [];
    const type = typeof node.type === "string" ? node.type : "";
    if (!NODE_SET.has(type) || type === "doc") {
      const text = textOf(node);
      if (inline) return text ? [{ type: "text", text }] : [];
      const kids = Array.isArray(node.content) ? (node.content as unknown[]) : [];
      if (kids.length > 0 && kids.every((k) => isObject(k) && BLOCKS.has(String(k.type)))) {
        return kids.flatMap((k) => convert(k, false));
      }
      return text ? [{ type: "paragraph", content: [{ type: "text", text }] }] : [];
    }
    if (!inline && (type === "text" || type === "hardBreak")) {
      const text = textOf(node);
      return text ? [{ type: "paragraph", content: [{ type: "text", text }] }] : [];
    }
    const out: Record<string, unknown> = { ...node };
    if (type === "heading" && !headingLevelOk(node)) {
      out.attrs = { ...(isObject(node.attrs) ? node.attrs : {}), level: 2 };
    }
    const marks = cleanMarks(node.marks);
    if (marks) out.marks = marks;
    else delete out.marks;
    if (Array.isArray(node.content)) {
      const childInline = TEXTBLOCKS.has(type);
      out.content = (node.content as unknown[]).flatMap((c) => convert(c, childInline));
    }
    return [out];
  };
  // A root that is not a document is shown as the document's only block.
  const content =
    isObject(doc) && doc.type === "doc"
      ? Array.isArray(doc.content)
        ? (doc.content as unknown[])
        : []
      : [doc];
  return { type: "doc", content: content.flatMap((c) => convert(c, false)) };
}

// --- links ----------------------------------------------------------------------------

export type HrefCheck = { ok: true; href: string } | { ok: false };

/** C0 controls and DEL (`char::is_control` for ASCII). */
function isControl(c: string): boolean {
  const code = c.codePointAt(0) ?? 0;
  return code < 0x20 || (code >= 0x7f && code <= 0x9f);
}

/** The scheme the way browsers read it (whitespace and control characters ignored). */
function schemeOf(s: string): string | null {
  let scheme = "";
  for (const c of s) {
    if (c === ":") return scheme.toLowerCase();
    if (c === "/" || c === "?" || c === "#") return null;
    if (/\s/.test(c) || isControl(c)) continue;
    scheme += c;
  }
  return null;
}

/**
 * A link target the site will emit (`blocks/url.rs` `safe_href`, stricter): `http`,
 * `https` and `mailto` URLs, `#fragments` and paths on this site. A bare domain
 * (`example.com/x`) becomes `https://…`. `javascript:`, `data:`, protocol-relative
 * `//host` and anything with a backslash or control character are refused.
 */
export function checkHref(raw: string): HrefCheck {
  const s = raw.trim();
  if (s === "" || s.includes("\\") || [...s].some(isControl)) return { ok: false };
  if (s.startsWith("#")) return { ok: true, href: s };
  if (s.startsWith("/")) return s.startsWith("//") ? { ok: false } : { ok: true, href: s };
  const scheme = schemeOf(s);
  if (scheme !== null) {
    return LINK_PROTOCOLS.includes(scheme) && /^[a-z]+:/i.test(s)
      ? { ok: true, href: s }
      : { ok: false };
  }
  if (s.startsWith("&") || /\s/.test(s)) return { ok: false };
  if (/^[^/?#]+\.[a-z]{2,}([/?#]|$)/i.test(s)) return { ok: true, href: `https://${s}` };
  return { ok: true, href: s };
}
