// The writing surface under the real server (plan Task 14; 07 §3, 11 §1, §6): write a
// new post with the slash menu and save it with `mod+s` (the address gets the id and a
// reload shows the same document), the link field refusing `javascript:`, focus mode,
// and the review screens. Every item is created by the test itself (E2E data hygiene).

import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { ContentApi } from "./support/content-api";
import { png } from "./support/files";
import { mod } from "./support/keys";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const fixture = (name: string): unknown =>
  JSON.parse(readFileSync(join(import.meta.dirname, "../src/editor/fixtures", name), "utf8"));

const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });
const body = (page: Page) => page.getByRole("textbox", { name: "Body" });
const slashMenu = (page: Page) => page.getByRole("listbox", { name: "Insert a block" });
const bubble = (page: Page) => page.getByRole("toolbar", { name: "Format" });
const UUID = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";

async function openNew(page: Page, type = "post"): Promise<void> {
  await page.goto(`/admin/content/${type}/new`);
  await expect(title(page)).toBeFocused();
  await expect(body(page)).toHaveAttribute("contenteditable", "true");
}

async function openItem(page: Page, type: string, id: string): Promise<void> {
  await page.goto(`/admin/content/${type}/${id}`);
  await expect(page.getByRole("textbox", { name: "Title", exact: true })).toBeVisible();
}

/**
 * Selects `text` and waits for the bubble menu, selecting again while it has not shown:
 * a selection made while the editor is still settling after the load can be dropped.
 */
async function selectForBubble(page: Page, text: string): Promise<void> {
  await expect(async () => {
    await selectText(page, text);
    await expect(bubble(page)).toBeVisible({ timeout: 1_000 });
  }).toPass({ timeout: 10_000 });
}

/** Selects `text` inside the body (a mouse drag over its characters' box). */
async function selectText(page: Page, text: string): Promise<void> {
  await body(page)
    .getByText(text, { exact: false })
    .first()
    .evaluate((el, wanted) => {
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const at = n.textContent?.indexOf(wanted) ?? -1;
        if (at >= 0) {
          const range = document.createRange();
          range.setStart(n, at);
          range.setEnd(n, at + wanted.length);
          const sel = window.getSelection();
          sel?.removeAllRanges();
          sel?.addRange(range);
          return;
        }
      }
      throw new Error(`text not found: ${wanted}`);
    }, text);
}

test("write_and_save_new_post", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const page = await as("admin");
  const name = `Written ${uid()}`;
  await openNew(page);

  await page.keyboard.type(name);
  await page.keyboard.press("Enter");
  await expect(body(page)).toBeFocused();
  await page.keyboard.type("An opening paragraph.");
  await page.keyboard.press("Enter");

  // `/` opens the block menu; typing filters it; Enter inserts.
  await page.keyboard.type("/");
  await expect(slashMenu(page)).toBeVisible();
  await page.keyboard.type("h2");
  await expect(slashMenu(page).getByRole("option")).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect(slashMenu(page)).toBeHidden();
  await page.keyboard.type("A section");
  await page.keyboard.press("Enter");
  await page.keyboard.type("/bullet");
  await expect(slashMenu(page).getByRole("option", { name: /Bulleted list/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await page.keyboard.press("Enter");
  await page.keyboard.type("first point");
  await page.keyboard.press("Enter");
  await page.keyboard.type("second point");

  await expect(body(page).locator("h2")).toHaveText("A section");
  await expect(body(page).locator("ul > li")).toHaveCount(2);

  await page.keyboard.press(`${await mod(page)}+s`);
  await expect(page).toHaveURL(new RegExp(`/admin/content/post/${UUID}$`));
  const id = new URL(page.url()).pathname.split("/").at(-1) ?? "";
  // Still in the body after the hand-over to the item's own address.
  await expect(body(page)).toBeFocused();
  await expect(page.locator("[data-slot=save-state]")).toHaveAttribute("data-state", "saved");

  const expected = {
    type: "doc",
    content: [
      { type: "paragraph", content: [{ type: "text", text: "An opening paragraph." }] },
      { type: "heading", attrs: { level: 2 }, content: [{ type: "text", text: "A section" }] },
      {
        type: "bulletList",
        content: ["first point", "second point"].map((text) => ({
          type: "listItem",
          content: [{ type: "paragraph", content: [{ type: "text", text }] }],
        })),
      },
    ],
  };
  const stored = await api.get("post", id);
  expect(stored.title).toBe(name);
  expect(stored.body).toEqual(expected);

  // A reload shows the same document, and saving it again sends nothing new.
  await page.reload();
  await expect(title(page)).toHaveValue(name);
  await expect(body(page).locator("h2")).toHaveText("A section");
  await expect(body(page).locator("ul > li")).toHaveText(["first point", "second point"]);

  // An edit saves as a PATCH with the version as `If-Match`.
  await body(page).locator("h2").click();
  await page.keyboard.press("End");
  await page.keyboard.type(" again");
  await expect(page.locator("[data-slot=save-state]")).toHaveAttribute("data-state", "dirty");
  const patch = page.waitForRequest(
    (r) => r.method() === "PATCH" && r.url().endsWith(`/api/v1/content/post/${id}`),
  );
  await page.keyboard.press(`${await mod(page)}+s`);
  expect((await patch).headers()["if-match"]).toBe(`"${stored.version}"`);
  await expect(page.locator("[data-slot=save-state]")).toHaveAttribute("data-state", "saved");
  expect((await api.get("post", id)).version).toBeGreaterThan(stored.version);
  await api.dispose();
});

test("bubble_menu_link_refuses_javascript", async ({ as, site, siteKey }) => {
  const api = await ContentApi.as(siteKey, site.url, "admin");
  const item = await api.create("post", `Links ${uid()}`, {
    body: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "Read the manual before you start." }],
        },
      ],
    },
  });
  const page = await as("admin");
  await openItem(page, "post", item.id);
  await body(page).click();
  await selectForBubble(page, "manual");

  await bubble(page).getByRole("button", { name: "Link" }).click();
  const field = page.getByRole("textbox", { name: "Link address" });
  await expect(field).toBeFocused();
  await field.fill("javascript:alert(1)");
  await field.press("Enter");
  await expect(page.getByRole("alert")).toHaveText(
    "Use a web address (https://…), a mailto: address or a path on this site.",
  );
  await expect(field).toHaveAttribute("aria-invalid", "true");
  await expect(body(page).locator("a")).toHaveCount(0);

  await field.fill("JaVaScRiPt:alert(1)");
  await field.press("Enter");
  await expect(body(page).locator("a")).toHaveCount(0);

  await field.fill("https://example.com/manual");
  await field.press("Enter");
  await expect(body(page).locator("a")).toHaveAttribute("href", "https://example.com/manual");
  await expect(body(page).locator("a")).toHaveText("manual");
  await api.dispose();
});

test("focus_mode", async ({ as }) => {
  const page = await as("admin");
  await openNew(page);
  const topBar = page.locator("[data-slot=top-bar]");
  const sidebar = page.locator("[data-slot=sidebar]").first();
  await expect(topBar).toBeVisible();
  await expect(sidebar).toHaveAttribute("data-state", "expanded");

  await page.keyboard.press(`${await mod(page)}+Shift+f`);
  await expect(topBar).toHaveAttribute("data-hidden", "true");
  await expect(topBar).toHaveCSS("opacity", "0");
  await expect(sidebar).toHaveAttribute("data-state", "collapsed");
  await expect(sidebar).toHaveAttribute("data-collapsible", "offcanvas");
  // Typing still goes to the title.
  await page.keyboard.type("Quiet");
  await expect(title(page)).toHaveValue("Quiet");

  // Esc leaves it; the shell comes back as it was.
  await page.keyboard.press("Escape");
  await expect(topBar).not.toHaveAttribute("data-hidden");
  await expect(sidebar).toHaveAttribute("data-state", "expanded");

  // The bar's button does the same.
  await page.getByRole("button", { name: "Focus mode" }).click();
  await expect(topBar).toHaveAttribute("data-hidden", "true");
  await page.getByRole("button", { name: "Leave focus mode" }).click();
  await expect(topBar).not.toHaveAttribute("data-hidden");

  // Leaving the editor leaves focus mode.
  await page.keyboard.press(`${await mod(page)}+Shift+f`);
  await expect(topBar).toHaveAttribute("data-hidden", "true");
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press("g");
  await page.keyboard.press("d");
  // The typed title is unsaved: the leave guard asks first (Task 16); discard it.
  await page
    .locator("[data-slot=leave-dialog]")
    .getByRole("button", { name: "Discard changes" })
    .click();
  await expect(page).toHaveURL(/\/admin\/$/);
  await expect(topBar).not.toHaveAttribute("data-hidden");
});

/** A long post as a writer would have it: prose, headings, lists, code, a table. */
function longBody(mediaId: string | null): unknown {
  const t = (text: string) => ({ type: "text", text });
  const p = (...parts: unknown[]) => ({
    type: "paragraph",
    content: parts.map((x) => (typeof x === "string" ? t(x) : x)),
  });
  const h = (level: number, text: string) => ({
    type: "heading",
    attrs: { level },
    content: [t(text)],
  });
  const li = (text: string) => ({ type: "listItem", content: [p(text)] });
  const bold = (text: string) => ({ type: "text", text, marks: [{ type: "bold" }] });
  const em = (text: string) => ({ type: "text", text, marks: [{ type: "italic" }] });
  const code = (text: string) => ({ type: "text", text, marks: [{ type: "code" }] });
  const link = (text: string, href: string) => ({
    type: "text",
    text,
    marks: [{ type: "link", attrs: { href } }],
  });
  const cell = (type: "tableCell" | "tableHeader", text: string) => ({
    type,
    attrs: { colspan: 1, rowspan: 1 },
    content: [p(text)],
  });
  return {
    type: "doc",
    content: [
      p(
        "Every page on this site is rendered from structured blocks, not stored HTML. ",
        "That one decision is why the editor can promise that ",
        bold("what you write is what the site shows"),
        ", and why a theme can never smuggle a script into a post.",
      ),
      p(
        "This tour walks through the blocks the editor knows, how they render, and what ",
        em("happens to the ones it does not"),
        ".",
      ),
      h(2, "Paragraphs and marks"),
      p(
        "Text can be ",
        bold("bold"),
        ", ",
        em("italic"),
        ", ",
        { type: "text", text: "struck through", marks: [{ type: "strike" }] },
        " or ",
        code("inline code"),
        ". Links such as ",
        link("the permalink guide", "/guides/permalinks/"),
        " keep their address exactly as typed.",
      ),
      h(3, "Lists"),
      {
        type: "bulletList",
        content: [
          li("A single binary and a single data folder"),
          li("Plugins in a capability-scoped sandbox"),
          li("Themes that are templates, not programs"),
        ],
      },
      {
        type: "orderedList",
        attrs: { start: 1 },
        content: [li("Write the draft"), li("Ask for a review"), li("Schedule it for Monday")],
      },
      {
        type: "blockquote",
        content: [p("Plugins are untrusted. They get only what their manifest declares.")],
      },
      ...(mediaId
        ? [
            {
              type: "image",
              attrs: { src: `ferrite:media/${mediaId}`, alt: "A soft gradient", title: null },
            },
          ]
        : []),
      h(2, "Code and tables"),
      p("Code blocks keep their language for the theme's highlighter:"),
      {
        type: "codeBlock",
        attrs: { language: "sh" },
        content: [
          t("ferrite init --data ./site --url http://localhost:8080\nferrite serve --data ./site"),
        ],
      },
      {
        type: "table",
        content: [
          {
            type: "tableRow",
            content: [
              cell("tableHeader", "Profile"),
              cell("tableHeader", "Database"),
              cell("tableHeader", "Media"),
            ],
          },
          {
            type: "tableRow",
            content: [
              cell("tableCell", "Solo"),
              cell("tableCell", "SQLite"),
              cell("tableCell", "Local folder"),
            ],
          },
          {
            type: "tableRow",
            content: [
              cell("tableCell", "Cluster"),
              cell("tableCell", "Postgres"),
              cell("tableCell", "S3-compatible"),
            ],
          },
        ],
      },
      { type: "horizontalRule" },
      h(2, "Raw HTML, when you need it"),
      {
        type: "html",
        attrs: { html: '<aside class="note">\n  <p>Sanitised on the way out.</p>\n</aside>' },
      },
      p(
        "Anything the editor cannot represent opens read-only, so nothing is ever rewritten ",
        "behind your back. The site renders it from the stored blocks either way.",
      ),
    ],
  };
}

test("screens", async ({ as, site, siteKey, snap, expectAccessible }) => {
  test.setTimeout(300_000);
  const api = await ContentApi.as(siteKey, site.url, "admin");
  // An image of its own: one another test uploaded may be deleted while this one runs.
  const image = await api.upload(png(`${uid("tour")}.png`, 960, 540));
  const long = await api.create("post", "A tour of the block format", {
    body: longBody(image.id),
  });
  const imported = await api.create("post", "Imported from another editor", {
    body: fixture("unknown-node.json"),
  });
  const broken = await api.create("post", "Structure the editor refuses", {
    body: {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "Written by an API client." }] },
        {
          type: "listItem",
          content: [{ type: "paragraph", content: [{ type: "text", text: "A stray list item." }] }],
        },
      ],
    },
  });
  const page = await as("admin");

  // An empty new post.
  await openNew(page);
  await snap(page, "editor-new");
  await expectAccessible(page);

  // A long post.
  await openItem(page, "post", long.id);
  await expect(body(page).locator("h2").first()).toHaveText("Paragraphs and marks");
  await snap(page, "editor-long-post");
  await expectAccessible(page);

  // The bubble menu over a selection (the top of the post, unscrolled).
  await selectForBubble(page, "what you write is what the site shows");
  await snap(page, "editor-bubble-menu");
  await expectAccessible(page);

  // Focus mode.
  await page.keyboard.press(`${await mod(page)}+Shift+f`);
  await expect(page.locator("[data-slot=top-bar]")).toHaveAttribute("data-hidden", "true");
  await snap(page, "editor-focus-mode");
  await expectAccessible(page);
  await page.keyboard.press("Escape");

  // The slash menu, while writing a short new post.
  await openNew(page);
  await page.keyboard.type("Weekend notes");
  await page.keyboard.press("Enter");
  await page.keyboard.type("A few things worth writing down before Monday.");
  await page.keyboard.press("Enter");
  await page.keyboard.type("/");
  await expect(slashMenu(page)).toBeVisible();
  await snap(page, "editor-slash-menu");
  await expectAccessible(page);
  await page.keyboard.press("Escape");
  await expect(slashMenu(page)).toBeHidden();

  // A body with a node the editor does not know: read-only, with the banner.
  await openItem(page, "post", imported.id);
  await expect(page.getByRole("note")).toContainText("callout");
  await expect(body(page)).toHaveAttribute("contenteditable", "false");
  await snap(page, "editor-read-only");
  await expectAccessible(page);

  // A body whose structure the schema refuses: its text, read-only, and the stored JSON
  // behind a disclosure (admins and editors).
  await openItem(page, "post", broken.id);
  const stored = page.locator("[data-slot=stored-text]");
  await expect(stored).toContainText("A stray list item.");
  await expect(page.locator('[contenteditable="true"]')).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Save" })).toBeDisabled();
  await stored.getByText("Show stored JSON").click();
  await expect(stored.locator("pre")).toContainText('"listItem"');
  await snap(page, "editor-content-error");
  await expectAccessible(page);
  await api.purgeMedia(image.id);
  await api.dispose();
});
